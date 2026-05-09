use crate::audio::{
    EngineTelemetry, InputParameterRingBufferProducer, InputParameters, MetersOutput,
    XrunEventsConsumer,
};
use ringbuf::traits::Consumer as RbConsumer;
use crate::dsp::echo::EchoParamKind;
use crate::dsp::mixer::MixerParam;
use crate::dsp::param::FloatParams;
use crate::dsp::reverb::ReverbParamKind;
use crate::persist::{AppState, PersistableState};

use anyhow::Result;
use ringbuf::traits::Producer;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

slint::include_modules!();

/// Meter ballistics — UI-side smoothing so the bars don't jitter every 33ms.
const METER_TICK_MS: u64 = 33;
/// Peak hold time before decay starts (ticks). 45 * 33ms ≈ 1.5s.
const PEAK_HOLD_TICKS: u32 = 45;
/// Per-tick multiplier once peak starts decaying. ~0.92 → ~22 dB/sec.
const PEAK_DECAY: f32 = 0.92;
/// RMS one-pole low-pass weight on the new sample. α=0.15 → ~200ms time const.
const RMS_NEW_WEIGHT: f32 = 0.15;

/// Bottom of the displayed dB range. 0 dB is always at the top.
const METER_MIN_DB: f32 = -60.0;

#[inline]
fn linear_to_pos(linear: f32) -> f32 {
    if linear <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * linear.log10();
    ((db - METER_MIN_DB) / -METER_MIN_DB).clamp(0.0, 1.0)
}

#[derive(Default, Clone, Copy)]
struct MeterDisplay {
    peak: f32,
    peak_hold: u32,
    rms: f32,
}

impl MeterDisplay {
    #[inline]
    fn step(&mut self, raw_peak: f32, raw_rms: f32) {
        if raw_peak >= self.peak {
            self.peak = raw_peak;
            self.peak_hold = PEAK_HOLD_TICKS;
        } else if self.peak_hold > 0 {
            self.peak_hold -= 1;
        } else {
            self.peak *= PEAK_DECAY;
            if self.peak < 1e-4 {
                self.peak = 0.0;
            }
        }
        self.rms = self.rms * (1.0 - RMS_NEW_WEIGHT) + raw_rms * RMS_NEW_WEIGHT;
    }
}

struct MeterDisplays {
    inputs: Vec<MeterDisplay>,
    reverb: MeterDisplay,
    echo: MeterDisplay,
    master_l: MeterDisplay,
    master_r: MeterDisplay,
}

/// Build the slint-side model for one effect's parameter list, sourcing the
/// initial slider value from the loaded `AppState`.
fn build_float_param_model<K: FloatParams>(state: &K::State) -> Vec<FloatParam> {
    K::all()
        .iter()
        .map(|&id| {
            let (min, max) = id.default_curve().range();
            FloatParam {
                label: id.name().into(),
                minimum: min,
                maximum: max,
                value: id.read(state) as f32,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    producer: InputParameterRingBufferProducer,
    running: Arc<AtomicBool>,
    num_inputs: usize,
    meters: Arc<MetersOutput>,
    telemetry: Arc<EngineTelemetry>,
    xrun_consumer: XrunEventsConsumer,
    persisted: Arc<Mutex<PersistableState>>,
    loaded: AppState,
) -> Result<()> {
    let ui = MainWindow::new()?;

    let labels: Vec<SharedString> = (0..num_inputs)
        .map(|i| format!("IN {}", i + 1).into())
        .collect();
    ui.set_input_labels(ModelRc::new(VecModel::from(labels)));

    let input_peaks_model: Rc<VecModel<f32>> =
        Rc::new(VecModel::from(vec![0.0_f32; num_inputs]));
    let input_rms_model: Rc<VecModel<f32>> =
        Rc::new(VecModel::from(vec![0.0_f32; num_inputs]));
    ui.set_input_peaks(ModelRc::from(input_peaks_model.clone()));
    ui.set_input_rms(ModelRc::from(input_rms_model.clone()));

    // ----- Seed initial slider positions from the loaded state -----

    // Reverb / echo float-param models.
    ui.set_reverb_params(ModelRc::new(VecModel::from(
        build_float_param_model::<ReverbParamKind>(&loaded.reverb),
    )));
    ui.set_echo_params(ModelRc::new(VecModel::from(
        build_float_param_model::<EchoParamKind>(&loaded.echo),
    )));

    // Input strip initial values (one entry per strip).
    let input_init: Vec<InputStripInit> = loaded
        .mixer
        .inputs
        .iter()
        .map(|s| InputStripInit {
            gain_db: s.gain_db,
            pan: s.pan,
            mute: s.mute,
            send_reverb: s.send_reverb,
            send_echo: s.send_echo,
            send_pre_fader: s.send_pre_fader,
        })
        .collect();
    ui.set_input_init(ModelRc::new(VecModel::from(input_init)));

    // FX returns + master initial values.
    ui.set_reverb_return_init(FxReturnInit {
        gain_db: loaded.mixer.reverb_return.gain_db,
        pan: loaded.mixer.reverb_return.pan,
        mute: loaded.mixer.reverb_return.mute,
    });
    ui.set_echo_return_init(FxReturnInit {
        gain_db: loaded.mixer.echo_return.gain_db,
        pan: loaded.mixer.echo_return.pan,
        mute: loaded.mixer.echo_return.mute,
    });
    ui.set_master_init_gain_db(loaded.mixer.master_gain_db);

    // ----- Event fanout: every UI event updates both audio + persisted state -----
    let producer = Rc::new(RefCell::new(producer));
    let push = {
        let producer = producer.clone();
        let persisted = persisted.clone();
        move |msg: InputParameters| {
            // 1. Send to audio thread.
            if producer.borrow_mut().try_push(msg).is_err() {
                tracing::warn!("Parameter channel full, dropping update");
            }
            // 2. Mirror in the persisted state (mark dirty for the saver).
            if let Ok(mut g) = persisted.lock() {
                g.apply_and_mark_dirty(msg);
            }
        }
    };

    let make_mixer = |variant: fn(f64) -> MixerParam| {
        let push = push.clone();
        move |v: f32| push(InputParameters::Mixer(variant(v as f64)))
    };

    let make_mixer_bool = |variant: fn(bool) -> MixerParam| {
        let push = push.clone();
        move |b: bool| push(InputParameters::Mixer(variant(b)))
    };

    let make_indexed_mixer_f = |variant: fn(usize, f64) -> MixerParam| {
        let push = push.clone();
        move |idx: i32, v: f32| push(InputParameters::Mixer(variant(idx as usize, v as f64)))
    };

    let make_indexed_mixer_b = |variant: fn(usize, bool) -> MixerParam| {
        let push = push.clone();
        move |idx: i32, b: bool| push(InputParameters::Mixer(variant(idx as usize, b)))
    };

    {
        let push = push.clone();
        ui.on_reverb_param_changed(move |idx: i32, v: f32| {
            if let Some(&kind) = ReverbParamKind::all().get(idx as usize) {
                push(InputParameters::Reverb(kind.build(v as f64)));
            }
        });
    }
    {
        let push = push.clone();
        ui.on_echo_param_changed(move |idx: i32, v: f32| {
            if let Some(&kind) = EchoParamKind::all().get(idx as usize) {
                push(InputParameters::Echo(kind.build(v as f64)));
            }
        });
    }

    ui.on_input_gain_changed(make_indexed_mixer_f(MixerParam::InputGainDb));
    ui.on_input_pan_changed(make_indexed_mixer_f(MixerParam::InputPan));
    ui.on_input_mute_changed(make_indexed_mixer_b(MixerParam::InputMute));
    ui.on_input_send_reverb_changed(make_indexed_mixer_f(MixerParam::InputSendReverb));
    ui.on_input_send_echo_changed(make_indexed_mixer_f(MixerParam::InputSendEcho));
    ui.on_input_send_pre_fader_changed(make_indexed_mixer_b(MixerParam::InputSendPreFader));

    ui.on_reverb_return_gain_changed(make_mixer(MixerParam::ReverbReturnGainDb));
    ui.on_reverb_return_pan_changed(make_mixer(MixerParam::ReverbReturnPan));
    ui.on_reverb_return_mute_changed(make_mixer_bool(MixerParam::ReverbReturnMute));
    ui.on_echo_return_gain_changed(make_mixer(MixerParam::EchoReturnGainDb));
    ui.on_echo_return_pan_changed(make_mixer(MixerParam::EchoReturnPan));
    ui.on_echo_return_mute_changed(make_mixer_bool(MixerParam::EchoReturnMute));

    ui.on_master_gain_changed(make_mixer(MixerParam::MasterGainDb));

    // ----- Shutdown timer (existing) -----
    let shutdown_timer = slint::Timer::default();
    {
        let running = running.clone();
        shutdown_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                if !running.load(Ordering::Relaxed) {
                    let _ = slint::quit_event_loop();
                }
            },
        );
    }

    // ----- Meter timer -----
    let displays: Rc<RefCell<MeterDisplays>> = Rc::new(RefCell::new(MeterDisplays {
        inputs: vec![MeterDisplay::default(); num_inputs],
        reverb: MeterDisplay::default(),
        echo: MeterDisplay::default(),
        master_l: MeterDisplay::default(),
        master_r: MeterDisplay::default(),
    }));

    let xrun_consumer = Rc::new(RefCell::new(xrun_consumer));
    let meter_timer = slint::Timer::default();
    {
        let ui_weak = ui.as_weak();
        let meters = meters.clone();
        let telemetry = telemetry.clone();
        let input_peaks = input_peaks_model.clone();
        let input_rms = input_rms_model.clone();
        let displays = displays.clone();
        let xrun_consumer = xrun_consumer.clone();
        meter_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(METER_TICK_MS),
            move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let mut d = displays.borrow_mut();
                ui.set_dsp_load_pct(telemetry.dsp_load_peak_pct());

                // Drain xrun events. For now, log via tracing — a visible UI
                // indicator can come later if needed.
                {
                    let mut c = xrun_consumer.borrow_mut();
                    while let Some(ev) = c.try_pop() {
                        tracing::warn!("xrun: {:?} at t={}us", ev.kind, ev.timestamp_us);
                    }
                }

                let n = meters.num_inputs().min(input_peaks.row_count());
                for i in 0..n {
                    let (raw_peak, raw_rms) = meters.load_input(i);
                    d.inputs[i].step(raw_peak, raw_rms);
                    input_peaks.set_row_data(i, linear_to_pos(d.inputs[i].peak));
                    input_rms.set_row_data(i, linear_to_pos(d.inputs[i].rms));
                }

                let (rp, rr) = meters.load_reverb();
                d.reverb.step(rp, rr);
                ui.set_reverb_peak(linear_to_pos(d.reverb.peak));
                ui.set_reverb_rms(linear_to_pos(d.reverb.rms));

                let (ep, er) = meters.load_echo();
                d.echo.step(ep, er);
                ui.set_echo_peak(linear_to_pos(d.echo.peak));
                ui.set_echo_rms(linear_to_pos(d.echo.rms));

                let ((mlp, mlr), (mrp, mrr)) = meters.load_master();
                d.master_l.step(mlp, mlr);
                d.master_r.step(mrp, mrr);
                ui.set_master_peak_l(linear_to_pos(d.master_l.peak));
                ui.set_master_rms_l(linear_to_pos(d.master_l.rms));
                ui.set_master_peak_r(linear_to_pos(d.master_r.peak));
                ui.set_master_rms_r(linear_to_pos(d.master_r.rms));
            },
        );
    }

    ui.run()?;

    running.store(false, Ordering::SeqCst);
    Ok(())
}

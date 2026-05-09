use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::audio::InputParameters;
use crate::dsp::compressor::{CompressorParam, CompressorParams};
use crate::dsp::mixer::MixerParam;
use crate::dsp::param::FloatParams;
use crate::dsp::reverb::{ReverbParam, ReverbParams};
use crate::dsp::stereo_delay::{StereoDelayParam, StereoDelayParams};
use crate::dsp::tape_delay::{TapeDelayParam, TapeDelayParams};

/// Bumped when the serialised format becomes incompatible. Future versions
/// can match on this and migrate. Missing fields always fall back to defaults
/// thanks to `#[serde(default)]`, so additive changes don't need a bump.
/// Bumped when the serialised format becomes incompatible. v2: renamed
/// `echo` → `stereo_delay` (and equivalents in MixerSnapshot / InputStripState).
/// Old v1 files still load via `#[serde(alias = "echo")]` shims.
const SCHEMA_VERSION: u32 = 2;

/// Top-level user-state mirror. Lives in `Arc<Mutex<...>>` on the main
/// thread; UI and MIDI threads call `apply()` after pushing each event.
/// The audio thread never touches this.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct AppState {
    pub version: u32,
    pub mixer: MixerSnapshot,
    pub reverb: ReverbParams,
    #[serde(alias = "echo")]
    pub stereo_delay: StereoDelayParams,
    pub tape_delay: TapeDelayParams,
    pub compressor: CompressorParams,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            mixer: MixerSnapshot::default(),
            reverb: ReverbParams::default(),
            stereo_delay: StereoDelayParams::default(),
            tape_delay: TapeDelayParams::default(),
            compressor: CompressorParams::default(),
        }
    }
}

impl AppState {
    /// Update the mirror to reflect an event that was just pushed to the
    /// audio thread. Call this from UI and MIDI fanouts.
    pub fn apply(&mut self, event: InputParameters) {
        match event {
            InputParameters::Reverb(p) => apply_reverb(p, &mut self.reverb),
            InputParameters::StereoDelay(p) => apply_stereo_delay(p, &mut self.stereo_delay),
            InputParameters::TapeDelay(p) => apply_tape_delay(p, &mut self.tape_delay),
            InputParameters::Mixer(p) => self.mixer.apply(p),
            InputParameters::Compressor(p) => apply_compressor(p, &mut self.compressor),
        }
    }

    /// Force `mixer.inputs` to exactly `num_inputs` entries by truncating or
    /// padding with defaults. Call after load and before sliders are built.
    pub fn align_inputs(&mut self, num_inputs: usize) {
        self.mixer
            .inputs
            .resize_with(num_inputs, InputStripState::default);
    }

    /// Generate the full sequence of parameter events that, when consumed by
    /// the audio thread, will reproduce this state. Used at startup.
    pub fn replay_events(&self) -> Vec<InputParameters> {
        use crate::dsp::compressor::CompressorParamKind;
        use crate::dsp::reverb::ReverbParamKind;
        use crate::dsp::stereo_delay::StereoDelayParamKind;
        use crate::dsp::tape_delay::TapeDelayParamKind;

        let mut out = Vec::new();

        for &id in ReverbParamKind::all() {
            let v = id.read(&self.reverb);
            out.push(InputParameters::Reverb(id.build(v)));
        }
        for &id in StereoDelayParamKind::all() {
            let v = id.read(&self.stereo_delay);
            out.push(InputParameters::StereoDelay(id.build(v)));
        }
        for &id in TapeDelayParamKind::all() {
            let v = id.read(&self.tape_delay);
            out.push(InputParameters::TapeDelay(id.build(v)));
        }
        for &id in CompressorParamKind::all() {
            let v = id.read(&self.compressor);
            out.push(InputParameters::Compressor(id.build(v)));
        }

        for (i, s) in self.mixer.inputs.iter().enumerate() {
            out.push(InputParameters::Mixer(MixerParam::InputGainDb(
                i,
                s.gain_db as f64,
            )));
            out.push(InputParameters::Mixer(MixerParam::InputPan(
                i,
                s.pan as f64,
            )));
            out.push(InputParameters::Mixer(MixerParam::InputMute(i, s.mute)));
            out.push(InputParameters::Mixer(MixerParam::InputSendReverb(
                i,
                s.send_reverb as f64,
            )));
            out.push(InputParameters::Mixer(MixerParam::InputSendStereoDelay(
                i,
                s.send_stereo_delay as f64,
            )));
            out.push(InputParameters::Mixer(MixerParam::InputSendTapeDelay(
                i,
                s.send_tape_delay as f64,
            )));
            out.push(InputParameters::Mixer(MixerParam::InputSendPreFader(
                i,
                s.send_pre_fader,
            )));
        }

        out.push(InputParameters::Mixer(MixerParam::ReverbReturnGainDb(
            self.mixer.reverb_return.gain_db as f64,
        )));
        out.push(InputParameters::Mixer(MixerParam::ReverbReturnPan(
            self.mixer.reverb_return.pan as f64,
        )));
        out.push(InputParameters::Mixer(MixerParam::ReverbReturnMute(
            self.mixer.reverb_return.mute,
        )));
        out.push(InputParameters::Mixer(MixerParam::StereoDelayReturnGainDb(
            self.mixer.stereo_delay_return.gain_db as f64,
        )));
        out.push(InputParameters::Mixer(MixerParam::StereoDelayReturnPan(
            self.mixer.stereo_delay_return.pan as f64,
        )));
        out.push(InputParameters::Mixer(MixerParam::StereoDelayReturnMute(
            self.mixer.stereo_delay_return.mute,
        )));
        out.push(InputParameters::Mixer(MixerParam::TapeDelayReturnGainDb(
            self.mixer.tape_delay_return.gain_db as f64,
        )));
        out.push(InputParameters::Mixer(MixerParam::TapeDelayReturnPan(
            self.mixer.tape_delay_return.pan as f64,
        )));
        out.push(InputParameters::Mixer(MixerParam::TapeDelayReturnMute(
            self.mixer.tape_delay_return.mute,
        )));
        out.push(InputParameters::Mixer(MixerParam::MasterGainDb(
            self.mixer.master_gain_db as f64,
        )));

        out
    }

    /// Load from `path`, falling back to `Default::default()` if the file is
    /// missing or malformed. Never fails — corrupt files cannot crash the app.
    pub fn load_or_default(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => match toml::from_str::<Self>(&content) {
                Ok(mut state) => {
                    if state.version != SCHEMA_VERSION {
                        tracing::warn!(
                            "State file {} has version {} (current {}); using as-is, missing fields will use defaults",
                            path.display(),
                            state.version,
                            SCHEMA_VERSION
                        );
                        state.version = SCHEMA_VERSION;
                    }
                    tracing::info!("Loaded state from {}", path.display());
                    state
                }
                Err(e) => {
                    tracing::warn!(
                        "State file {} is malformed: {}; using defaults",
                        path.display(),
                        e
                    );
                    Self::default()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                tracing::info!("No state file at {}; starting with defaults", path.display());
                Self::default()
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to read state file {}: {}; using defaults",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    /// Atomically write to `path` (write-temp + rename). Creates parent dir.
    pub fn save_atomic(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let toml_str = toml::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, toml_str)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct MixerSnapshot {
    pub inputs: Vec<InputStripState>,
    pub reverb_return: FxReturnState,
    #[serde(alias = "echo_return")]
    pub stereo_delay_return: FxReturnState,
    pub tape_delay_return: FxReturnState,
    pub master_gain_db: f32,
}

impl Default for MixerSnapshot {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            reverb_return: FxReturnState::default(),
            stereo_delay_return: FxReturnState::default(),
            tape_delay_return: FxReturnState::default(),
            master_gain_db: 0.0,
        }
    }
}

impl MixerSnapshot {
    pub fn apply(&mut self, event: MixerParam) {
        match event {
            MixerParam::InputGainDb(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.gain_db = v as f32;
                }
            }
            MixerParam::InputPan(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.pan = (v as f32).clamp(-1.0, 1.0);
                }
            }
            MixerParam::InputMute(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.mute = v;
                }
            }
            MixerParam::InputSendReverb(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.send_reverb = (v as f32).clamp(0.0, 1.0);
                }
            }
            MixerParam::InputSendStereoDelay(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.send_stereo_delay = (v as f32).clamp(0.0, 1.0);
                }
            }
            MixerParam::InputSendTapeDelay(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.send_tape_delay = (v as f32).clamp(0.0, 1.0);
                }
            }
            MixerParam::InputSendPreFader(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.send_pre_fader = v;
                }
            }
            MixerParam::ReverbReturnGainDb(v) => self.reverb_return.gain_db = v as f32,
            MixerParam::ReverbReturnPan(v) => {
                self.reverb_return.pan = (v as f32).clamp(-1.0, 1.0);
            }
            MixerParam::ReverbReturnMute(v) => self.reverb_return.mute = v,
            MixerParam::StereoDelayReturnGainDb(v) => self.stereo_delay_return.gain_db = v as f32,
            MixerParam::StereoDelayReturnPan(v) => {
                self.stereo_delay_return.pan = (v as f32).clamp(-1.0, 1.0);
            }
            MixerParam::StereoDelayReturnMute(v) => self.stereo_delay_return.mute = v,
            MixerParam::TapeDelayReturnGainDb(v) => self.tape_delay_return.gain_db = v as f32,
            MixerParam::TapeDelayReturnPan(v) => {
                self.tape_delay_return.pan = (v as f32).clamp(-1.0, 1.0);
            }
            MixerParam::TapeDelayReturnMute(v) => self.tape_delay_return.mute = v,
            MixerParam::MasterGainDb(v) => self.master_gain_db = v as f32,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
#[serde(default)]
pub struct InputStripState {
    pub gain_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub send_reverb: f32,
    #[serde(alias = "send_echo")]
    pub send_stereo_delay: f32,
    pub send_tape_delay: f32,
    pub send_pre_fader: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
#[serde(default)]
pub struct FxReturnState {
    pub gain_db: f32,
    pub pan: f32,
    pub mute: bool,
}

fn apply_reverb(p: ReverbParam, s: &mut ReverbParams) {
    match p {
        ReverbParam::Size(v) => s.size = v as f32,
        ReverbParam::Feedback(v) => s.feedback = v as f32,
        ReverbParam::Balance(v) => s.balance = v as f32,
        ReverbParam::PreDelayMs(v) => s.pre_delay_ms = v as f32,
        ReverbParam::HpfHz(v) => s.hpf_hz = v as f32,
        ReverbParam::LpfHz(v) => s.lpf_hz = v as f32,
        ReverbParam::Chorus(v) => s.chorus = v as f32,
        ReverbParam::Send(v) => s.send = v as f32,
    }
}

fn apply_stereo_delay(p: StereoDelayParam, s: &mut StereoDelayParams) {
    match p {
        StereoDelayParam::Send(v) => s.send = v as f32,
        StereoDelayParam::FbLocal(v) => s.fb_local = v as f32,
        StereoDelayParam::FbCross(v) => s.fb_cross = v as f32,
        StereoDelayParam::TimeLMs(v) => s.time_l_ms = v as f32,
        StereoDelayParam::TimeRMs(v) => s.time_r_ms = v as f32,
        StereoDelayParam::LpfHz(v) => s.lpf_hz = v as f32,
    }
}

fn apply_tape_delay(p: TapeDelayParam, s: &mut TapeDelayParams) {
    match p {
        TapeDelayParam::Send(v) => s.send = v as f32,
        TapeDelayParam::RepeatRateMs(v) => s.repeat_rate_ms = v as f32,
        TapeDelayParam::Intensity(v) => s.intensity = v as f32,
        TapeDelayParam::H1Level(v) => s.h1_level = v as f32,
        TapeDelayParam::H2Level(v) => s.h2_level = v as f32,
        TapeDelayParam::H3Level(v) => s.h3_level = v as f32,
        TapeDelayParam::SaturationDrive(v) => s.saturation_drive = v as f32,
        TapeDelayParam::HfRolloffHz(v) => s.hf_rolloff_hz = v as f32,
        TapeDelayParam::WowDepth(v) => s.wow_depth = v as f32,
        TapeDelayParam::WowRateHz(v) => s.wow_rate_hz = v as f32,
        TapeDelayParam::FlutterDepth(v) => s.flutter_depth = v as f32,
        TapeDelayParam::FlutterRateHz(v) => s.flutter_rate_hz = v as f32,
    }
}

fn apply_compressor(p: CompressorParam, s: &mut CompressorParams) {
    match p {
        CompressorParam::ThresholdDb(v) => s.threshold_db = v as f32,
        CompressorParam::Ratio(v) => s.ratio = v as f32,
        CompressorParam::AttackMs(v) => s.attack_ms = v as f32,
        CompressorParam::ReleaseMs(v) => s.release_ms = v as f32,
        CompressorParam::KneeDb(v) => s.knee_db = v as f32,
        CompressorParam::MakeupDb(v) => s.makeup_db = v as f32,
    }
}

/// `$XDG_CONFIG_HOME/neosynth/state.toml`, falling back to
/// `~/.config/neosynth/state.toml`. Returns None on environments without
/// either env var (rare on Linux).
pub fn default_state_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(dir);
        if !p.as_os_str().is_empty() {
            return Some(p.join("neosynth").join("state.toml"));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Some(
            PathBuf::from(home)
                .join(".config")
                .join("neosynth")
                .join("state.toml"),
        );
    }
    None
}

/// Persistable state + dirty flag. Wrap in `Arc<Mutex<...>>`.
pub struct PersistableState {
    pub state: AppState,
    pub dirty: bool,
}

impl PersistableState {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            dirty: false,
        }
    }

    pub fn apply_and_mark_dirty(&mut self, event: InputParameters) {
        self.state.apply(event);
        self.dirty = true;
    }
}

/// Spawn a persister thread that periodically writes dirty state to disk.
/// Polls `running` every 100ms so it shuts down promptly when the app exits.
/// Used in both UI and headless modes — the slint event loop is no longer
/// involved in persistence timing.
pub fn spawn_persister(
    running: Arc<AtomicBool>,
    persisted: Arc<Mutex<PersistableState>>,
    path: Option<PathBuf>,
    interval: Duration,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let tick = Duration::from_millis(100);
        let mut elapsed = Duration::ZERO;
        while running.load(Ordering::Relaxed) {
            std::thread::sleep(tick);
            elapsed += tick;
            if elapsed < interval {
                continue;
            }
            elapsed = Duration::ZERO;

            let Some(p) = path.as_ref() else {
                continue;
            };
            let Ok(mut g) = persisted.lock() else {
                continue;
            };
            if !g.dirty {
                continue;
            }
            match g.state.save_atomic(p) {
                Ok(()) => {
                    g.dirty = false;
                    tracing::debug!("Persisted state to {}", p.display());
                }
                Err(e) => tracing::warn!("Periodic state save failed: {}", e),
            }
        }
        tracing::info!("Persister thread stopped");
    })
}

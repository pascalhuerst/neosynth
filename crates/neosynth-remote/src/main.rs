use anyhow::{Context, Result};
use clap::Parser;
use rosc::{OscMessage, OscPacket, OscType, decoder, encoder};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

slint::include_modules!();

#[derive(Parser, Debug)]
#[command(version, about = "neosynth-remote — OSC GUI for the neosynth engine")]
struct Cli {
    /// Host:port the engine's OSC server is listening on.
    #[arg(long, default_value = "127.0.0.1:9000")]
    target: SocketAddr,

    /// UDP port we bind locally to receive replies and broadcast bundles.
    /// `0` lets the OS pick an ephemeral port.
    #[arg(long, default_value_t = 0)]
    listen_port: u16,
}

/// Schema entry for one parameter, parsed from a `/list/item` reply.
#[derive(Debug, Clone)]
enum ParamKind {
    Float { min: f32, max: f32, default: f32 },
    Bool { default: bool },
}

#[derive(Debug, Clone)]
struct Param {
    path: String,
    kind: ParamKind,
    label: String,
    /// One of "Reverb", "Stereo Delay", "Master", "Reverb Return", "Stereo Delay Return",
    /// "Input N". Used to bucket controls in the UI.
    group: String,
    /// Sort order within group; lower = earlier.
    order: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let socket = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], cli.listen_port)))
        .with_context(|| format!("binding local UDP socket on port {}", cli.listen_port))?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    let local_addr = socket.local_addr()?;
    tracing::info!("Bound local socket on {}", local_addr);
    tracing::info!("Engine target: {}", cli.target);

    // ----- Discovery: /list ----- ----- ----- ----- ----- ----- ----- -----
    send_message(&socket, &cli.target, "/list", &[])?;
    let schema = receive_until(&socket, "/list/end", Duration::from_secs(3))?
        .into_iter()
        .filter_map(|msg| {
            if msg.addr != "/list/item" {
                return None;
            }
            parse_list_item(&msg)
        })
        .collect::<Vec<Param>>();
    if schema.is_empty() {
        anyhow::bail!("No parameters returned by /list — is the engine running with --osc-listen?");
    }
    tracing::info!("Received {} parameters from engine", schema.len());

    // ----- Initial values: /get_all ----- ----- ----- ----- ----- -----
    send_message(&socket, &cli.target, "/get_all", &[])?;
    let initial_values = receive_until(&socket, "/state/end", Duration::from_secs(3))?
        .into_iter()
        .filter_map(|msg| {
            if !msg.addr.starts_with("/state") {
                return None;
            }
            let path = msg.addr.strip_prefix("/state")?.to_string();
            let arg = msg.args.into_iter().next()?;
            Some((path, arg))
        })
        .collect::<BTreeMap<String, OscType>>();
    tracing::info!("Received initial values for {} paths", initial_values.len());

    // ----- Subscribe for periodic broadcasts ----- ----- ----- ----- -----
    send_message(&socket, &cli.target, "/subscribe", &[])?;
    tracing::info!("Subscribed for meter/telemetry broadcasts");

    // ----- Build the slint model ----- ----- ----- ----- ----- -----
    let groups_model = build_groups_model(&schema, &initial_values);

    let ui = MainWindow::new()?;
    ui.set_groups(ModelRc::from(groups_model.clone()));
    ui.set_connection_status(format!("Connected to {}", cli.target).into());

    // path → (group_idx, param_idx) so push-from-engine can update the right row
    let path_index = build_path_index(&groups_model);

    // ----- User interactions: send OSC out -----
    {
        let socket = socket.try_clone()?;
        let target = cli.target;
        ui.on_float_changed(move |path, v| {
            let _ = send_message(&socket, &target, &path, &[OscType::Float(v)]);
        });
    }
    {
        let socket = socket.try_clone()?;
        let target = cli.target;
        ui.on_bool_changed(move |path, b| {
            let _ = send_message(&socket, &target, &path, &[OscType::Bool(b)]);
        });
    }

    // ----- Background OSC reader → mpsc → slint Timer drain -----
    // OSC reader runs in a thread, parses packets to `UiUpdate`s, sends them
    // over an mpsc. A slint Timer in the UI thread drains the channel and
    // applies updates to the (non-Send) Rc-bearing models.
    let (tx, rx) = channel::<Vec<UiUpdate>>();
    let running = Arc::new(AtomicBool::new(true));
    let reader_handle = spawn_osc_reader(socket, tx, running.clone());

    let drain_timer = slint::Timer::default();
    {
        let ui_weak = ui.as_weak();
        let groups_model = groups_model.clone();
        let path_index = path_index.clone();
        let rx = rx;
        drain_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(33),
            move || {
                let Some(ui) = ui_weak.upgrade() else { return; };
                while let Ok(updates) = rx.try_recv() {
                    apply_updates(&ui, &groups_model, &path_index, updates);
                }
            },
        );
    }

    {
        let running = running.clone();
        ctrlc::set_handler(move || {
            running.store(false, Ordering::SeqCst);
            let _ = slint::quit_event_loop();
        })?;
    }

    ui.run()?;

    running.store(false, Ordering::SeqCst);
    let _ = reader_handle.join();
    Ok(())
}

/// True for floats whose range is symmetric around zero (e.g. pan, balance).
/// We detect by `min == -max` modulo a small relative tolerance, which means
/// gain_db sliders (-60..+12) deliberately don't qualify — those are
/// asymmetric attenuators and the linear slider feel is correct for them.
fn is_bipolar(min: f32, max: f32) -> bool {
    if min >= 0.0 || max <= 0.0 {
        return false;
    }
    let range = max - min;
    (min + max).abs() < 0.01 * range
}

fn parse_list_item(msg: &OscMessage) -> Option<Param> {
    // args: [path:str, type:"f"|"b", min:f, max:f, default:f]
    let mut it = msg.args.iter();
    let path = match it.next()? {
        OscType::String(s) => s.clone(),
        _ => return None,
    };
    let ty = match it.next()? {
        OscType::String(s) => s.clone(),
        _ => return None,
    };
    let min = match it.next()? {
        OscType::Float(f) => *f,
        _ => return None,
    };
    let max = match it.next()? {
        OscType::Float(f) => *f,
        _ => return None,
    };
    let default = match it.next()? {
        OscType::Float(f) => *f,
        _ => return None,
    };

    let kind = match ty.as_str() {
        "f" => ParamKind::Float { min, max, default },
        "b" => ParamKind::Bool {
            default: default >= 0.5,
        },
        _ => return None,
    };

    let (group, order) = group_and_order(&path);
    let label = label_for(&path);
    Some(Param {
        path,
        kind,
        label,
        group,
        order,
    })
}

/// Bucket a path into a UI group, with a stable per-group sort key.
fn group_and_order(path: &str) -> (String, usize) {
    if let Some(rest) = path.strip_prefix("/reverb/") {
        return (
            "Reverb".into(),
            reverb_echo_order(rest, &["size", "feedback", "balance", "pre_delay_ms", "hpf_hz", "lpf_hz", "chorus", "send"]),
        );
    }
    if let Some(rest) = path.strip_prefix("/stereo_delay/") {
        return (
            "Stereo Delay".into(),
            reverb_echo_order(rest, &["send", "fb_local", "fb_cross", "time_l_ms", "time_r_ms", "lpf_hz"]),
        );
    }
    if let Some(rest) = path.strip_prefix("/tape_delay/") {
        return (
            "Tape Delay".into(),
            reverb_echo_order(
                rest,
                &[
                    "send",
                    "repeat_rate_ms",
                    "intensity",
                    "h1_level",
                    "h2_level",
                    "h3_level",
                    "saturation_drive",
                    "hf_rolloff_hz",
                    "wow_depth",
                    "wow_rate_hz",
                    "flutter_depth",
                    "flutter_rate_hz",
                ],
            ),
        );
    }
    if let Some(rest) = path.strip_prefix("/compressor/") {
        return (
            "Compressor".into(),
            reverb_echo_order(
                rest,
                &["threshold_db", "ratio", "attack_ms", "release_ms", "knee_db", "makeup_db"],
            ),
        );
    }
    if let Some(rest) = path.strip_prefix("/mixer/master/") {
        return ("Master".into(), mixer_segment_order(rest));
    }
    if let Some(rest) = path.strip_prefix("/mixer/reverb_return/") {
        return ("Reverb Return".into(), mixer_segment_order(rest));
    }
    if let Some(rest) = path.strip_prefix("/mixer/stereo_delay_return/") {
        return ("Stereo Delay Return".into(), mixer_segment_order(rest));
    }
    if let Some(rest) = path.strip_prefix("/mixer/tape_delay_return/") {
        return ("Tape Delay Return".into(), mixer_segment_order(rest));
    }
    if let Some(rest) = path.strip_prefix("/mixer/input/") {
        // /mixer/input/N/segment
        if let Some((n, seg)) = rest.split_once('/') {
            if let Ok(idx) = n.parse::<usize>() {
                return (format!("Input {}", idx + 1), mixer_segment_order(seg));
            }
        }
    }
    ("Other".into(), 999)
}

fn reverb_echo_order(seg: &str, ordered: &[&str]) -> usize {
    ordered.iter().position(|x| *x == seg).unwrap_or(ordered.len())
}

fn mixer_segment_order(seg: &str) -> usize {
    match seg {
        "gain_db" => 0,
        "pan" => 1,
        "mute" => 2,
        "send_reverb" => 3,
        "send_stereo_delay" => 4,
        "send_tape_delay" => 5,
        "send_pre_fader" => 6,
        _ => 99,
    }
}

fn label_for(path: &str) -> String {
    // Take the last segment, replace underscores with spaces, capitalize.
    let last = path.rsplit('/').next().unwrap_or(path);
    let mut out = String::with_capacity(last.len());
    let mut next_upper = true;
    for ch in last.chars() {
        if ch == '_' {
            out.push(' ');
            next_upper = true;
        } else if next_upper {
            for u in ch.to_uppercase() {
                out.push(u);
            }
            next_upper = false;
        } else {
            out.push(ch);
        }
    }
    // Light cleanup for the common units we use.
    out = out.replace(" Db", " (dB)").replace(" Hz", " (Hz)").replace(" Ms", " (ms)");
    out
}

fn build_groups_model(
    schema: &[Param],
    initial: &BTreeMap<String, OscType>,
) -> Rc<VecModel<GroupRow>> {
    use std::collections::BTreeMap as Map;
    // Group → ordered Vec<Param>
    let mut by_group: Map<String, Vec<Param>> = Map::new();
    for p in schema {
        by_group.entry(p.group.clone()).or_default().push(p.clone());
    }
    for v in by_group.values_mut() {
        v.sort_by_key(|p| p.order);
    }

    // Effect groups first, then Inputs in numeric order.
    let group_priority = |name: &str| -> usize {
        match name {
            "Reverb" => 0,
            "Stereo Delay" => 1,
            "Tape Delay" => 2,
            "Compressor" => 3,
            "Master" => 4,
            "Reverb Return" => 5,
            "Stereo Delay Return" => 6,
            "Tape Delay Return" => 7,
            n if n.starts_with("Input ") => {
                10 + n.trim_start_matches("Input ").parse::<usize>().unwrap_or(0)
            }
            _ => 100,
        }
    };

    let mut groups: Vec<(String, Vec<Param>)> = by_group.into_iter().collect();
    groups.sort_by_key(|(name, _)| group_priority(name));

    let group_rows: Vec<GroupRow> = groups
        .into_iter()
        .map(|(title, params)| {
            let rows: Vec<ParamRow> = params
                .into_iter()
                .map(|p| {
                    let init = initial.get(&p.path);
                    match p.kind {
                        ParamKind::Float { min, max, default } => {
                            let value = match init {
                                Some(OscType::Float(v)) => *v,
                                Some(OscType::Int(i)) => *i as f32,
                                _ => default,
                            };
                            ParamRow {
                                path: p.path.into(),
                                kind: 0,
                                label: p.label.into(),
                                min,
                                max,
                                float_value: value,
                                bool_value: false,
                                bipolar: is_bipolar(min, max),
                            }
                        }
                        ParamKind::Bool { default } => {
                            let value = match init {
                                Some(OscType::Bool(b)) => *b,
                                Some(OscType::Int(i)) => *i != 0,
                                Some(OscType::Float(f)) => *f >= 0.5,
                                _ => default,
                            };
                            ParamRow {
                                path: p.path.into(),
                                kind: 1,
                                label: p.label.into(),
                                min: 0.0,
                                max: 1.0,
                                float_value: 0.0,
                                bool_value: value,
                                bipolar: false,
                            }
                        }
                    }
                })
                .collect();
            GroupRow {
                title: title.into(),
                params: ModelRc::new(VecModel::from(rows)),
            }
        })
        .collect();

    Rc::new(VecModel::from(group_rows))
}

/// path → (group_index, param_index) for fast lookup when applying broadcasts.
fn build_path_index(groups: &Rc<VecModel<GroupRow>>) -> Rc<RefCell<BTreeMap<String, (usize, usize)>>> {
    let mut idx: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (g, group) in groups.iter().enumerate() {
        for (p, param) in group.params.iter().enumerate() {
            idx.insert(param.path.to_string(), (g, p));
        }
    }
    Rc::new(RefCell::new(idx))
}

fn send_message(
    socket: &UdpSocket,
    dest: &SocketAddr,
    addr: &str,
    args: &[OscType],
) -> Result<()> {
    let msg = OscMessage {
        addr: addr.into(),
        args: args.to_vec(),
    };
    let bytes = encoder::encode(&OscPacket::Message(msg))
        .map_err(|e| anyhow::anyhow!("OSC encode: {e:?}"))?;
    socket.send_to(&bytes, dest)?;
    Ok(())
}

/// Receive messages until one with `terminator` address arrives, or timeout.
/// Discards any bundles received in the meantime (broadcast tick, before we
/// subscribed nothing should be coming, but this is defensive).
fn receive_until(
    socket: &UdpSocket,
    terminator: &str,
    overall_timeout: Duration,
) -> Result<Vec<OscMessage>> {
    let mut buf = [0u8; 65536];
    let deadline = Instant::now() + overall_timeout;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, _)) => {
                let Ok((_, packet)) = decoder::decode_udp(&buf[..n]) else {
                    continue;
                };
                let mut done = false;
                collect_messages(&packet, &mut out, &mut done, terminator);
                if done {
                    return Ok(out);
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("Timeout waiting for {}", terminator)
}

fn collect_messages(packet: &OscPacket, out: &mut Vec<OscMessage>, done: &mut bool, terminator: &str) {
    match packet {
        OscPacket::Message(m) => {
            if m.addr == terminator {
                *done = true;
            } else {
                out.push(m.clone());
            }
        }
        OscPacket::Bundle(b) => {
            for inner in &b.content {
                collect_messages(inner, out, done, terminator);
            }
        }
    }
}

/// Spawn a thread that reads OSC packets and forwards parsed updates over the
/// given channel. The slint side drains the channel from a Timer.
fn spawn_osc_reader(
    socket: UdpSocket,
    tx: Sender<Vec<UiUpdate>>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok();

    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        while running.load(Ordering::Relaxed) {
            let n = match socket.recv_from(&mut buf) {
                Ok((n, _)) => n,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => {
                    tracing::warn!("OSC recv error: {}", e);
                    continue;
                }
            };
            let Ok((_, packet)) = decoder::decode_udp(&buf[..n]) else {
                continue;
            };
            let mut updates = Vec::<UiUpdate>::new();
            extract_updates(&packet, &mut updates);
            if updates.is_empty() {
                continue;
            }
            // Receiver gone → UI exited. Stop.
            if tx.send(updates).is_err() {
                break;
            }
        }
    })
}

#[derive(Debug)]
enum UiUpdate {
    DspLoad(f32),
    OverrunCount(i32),
    UnderrunCount(i32),
    /// Any `/<path> f` not in the meter/telemetry namespace — applied as a
    /// state update to the matching slider.
    StateFloat { path: String, value: f32 },
    StateBool { path: String, value: bool },
}

fn extract_updates(packet: &OscPacket, out: &mut Vec<UiUpdate>) {
    match packet {
        OscPacket::Message(m) => {
            match m.addr.as_str() {
                "/telemetry/dsp_load" => {
                    if let Some(OscType::Float(v)) = m.args.first() {
                        out.push(UiUpdate::DspLoad(*v));
                    }
                }
                "/telemetry/xrun_overrun" => {
                    if let Some(OscType::Int(v)) = m.args.first() {
                        out.push(UiUpdate::OverrunCount(*v));
                    }
                }
                "/telemetry/xrun_underrun" => {
                    if let Some(OscType::Int(v)) = m.args.first() {
                        out.push(UiUpdate::UnderrunCount(*v));
                    }
                }
                addr if addr.starts_with("/meters/") => {
                    // Meters not yet rendered in the UI; ignore for now.
                }
                addr if addr.starts_with("/state") => {
                    let path = addr.strip_prefix("/state").unwrap_or("").to_string();
                    match m.args.first() {
                        Some(OscType::Float(v)) => out.push(UiUpdate::StateFloat { path, value: *v }),
                        Some(OscType::Bool(b)) => out.push(UiUpdate::StateBool { path, value: *b }),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        OscPacket::Bundle(b) => {
            for inner in &b.content {
                extract_updates(inner, out);
            }
        }
    }
}

fn apply_updates(
    ui: &MainWindow,
    groups: &Rc<VecModel<GroupRow>>,
    path_index: &Rc<RefCell<BTreeMap<String, (usize, usize)>>>,
    updates: Vec<UiUpdate>,
) {
    let idx = path_index.borrow();
    for u in updates {
        match u {
            UiUpdate::DspLoad(v) => ui.set_dsp_load_pct(v),
            UiUpdate::OverrunCount(v) => ui.set_overrun_count(v),
            UiUpdate::UnderrunCount(v) => ui.set_underrun_count(v),
            UiUpdate::StateFloat { path, value } => {
                if let Some(&(g, p)) = idx.get(&path) {
                    if let Some(group) = groups.row_data(g) {
                        if let Some(mut row) = group.params.row_data(p) {
                            row.float_value = value;
                            group.params.set_row_data(p, row);
                        }
                    }
                }
            }
            UiUpdate::StateBool { path, value } => {
                if let Some(&(g, p)) = idx.get(&path) {
                    if let Some(group) = groups.row_data(g) {
                        if let Some(mut row) = group.params.row_data(p) {
                            row.bool_value = value;
                            group.params.set_row_data(p, row);
                        }
                    }
                }
            }
        }
    }
}

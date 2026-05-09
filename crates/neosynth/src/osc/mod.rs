mod router;

use router::{OscParamKind, OscRouter, OscValue};

use crate::audio::{EngineTelemetry, InputParameterRingBufferProducer, MetersOutput};
use crate::persist::PersistableState;

use anyhow::{Context, Result};
use ringbuf::traits::Producer;
use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, decoder, encoder};
use std::collections::HashSet;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often we broadcast meter / telemetry bundles to subscribers (~30 Hz).
const BROADCAST_INTERVAL: Duration = Duration::from_millis(33);
/// Socket recv timeout. Loop wakes at this rate so we can both serve incoming
/// requests promptly and tick the broadcast cadence.
const RECV_TIMEOUT: Duration = Duration::from_millis(20);

/// Spawn the OSC thread.
///
/// Binds a UDP socket to `listen_addr` (e.g. `0.0.0.0:9000`), parses incoming
/// OSC packets, dispatches Control messages through the router into
/// `InputParameters`, and pushes onto `producer`. Each event also fanouts to
/// `persisted` so MIDI/UI/OSC all converge on the same state.
///
/// # Discovery / sync protocol (server-side)
///
/// * `/list`           — server replies one `/list/item` per parameter, then
///                        `/list/end`. Static metadata only (path, type, range,
///                        default).
/// * `/get_all`        — server replies one `/state/<path>` per parameter with
///                        the *current* value, then `/state/end`. Use this for
///                        initial UI sync after `/list`.
/// * `/subscribe`      — registers the source `addr:port` for periodic meter
///                        and telemetry bundles (~30 Hz). No args.
/// * `/unsubscribe`    — removes the source from the broadcast list.
///
/// # /list/item argument layout
///
/// * args[0]  string  full OSC path (e.g. `/reverb/size`)
/// * args[1]  string  type tag, `"f"` for float or `"b"` for bool
/// * args[2]  float   minimum
/// * args[3]  float   maximum
/// * args[4]  float   default value (booleans encode 0.0 or 1.0)
///
/// # /state/<path> argument layout
///
/// One float arg for floats, one bool arg for booleans. Path matches the
/// addresses returned by `/list/item`.
///
/// # Broadcast bundle (sent to subscribers)
///
/// A single OSC bundle per tick containing:
///
/// * `/meters/input/{idx}/peak`, `/meters/input/{idx}/rms` — per input strip
/// * `/meters/reverb/peak`, `/meters/reverb/rms`
/// * `/meters/echo/peak`, `/meters/echo/rms`
/// * `/meters/master/l/peak`, `/meters/master/l/rms`,
///    `/meters/master/r/peak`, `/meters/master/r/rms`
/// * `/meters/compressor/gr_db`     f  (master compressor peak gain reduction)
/// * `/telemetry/dsp_load`           f  (peak-hold percentage)
/// * `/telemetry/xrun_overrun`       i  (cumulative count)
/// * `/telemetry/xrun_underrun`      i  (cumulative count)
#[allow(clippy::too_many_arguments)]
pub fn run(
    running: Arc<AtomicBool>,
    producer: InputParameterRingBufferProducer,
    persisted: Arc<Mutex<PersistableState>>,
    meters: Arc<MetersOutput>,
    telemetry: Arc<EngineTelemetry>,
    listen_addr: SocketAddr,
    num_inputs: usize,
) -> Result<JoinHandle<()>> {
    let socket = UdpSocket::bind(listen_addr)
        .with_context(|| format!("binding OSC UDP socket to {listen_addr}"))?;
    socket
        .set_read_timeout(Some(RECV_TIMEOUT))
        .context("setting OSC read timeout")?;
    let local_addr = socket.local_addr()?;
    tracing::info!("OSC: listening on {}", local_addr);

    let router = OscRouter::new(num_inputs);
    let mut producer = producer;

    let handle = std::thread::spawn(move || {
        let mut subs: HashSet<SocketAddr> = HashSet::new();
        let mut last_broadcast = Instant::now();
        if let Err(e) = run_loop(
            &socket,
            &router,
            &mut producer,
            &persisted,
            &meters,
            &telemetry,
            &mut subs,
            &mut last_broadcast,
            &running,
        ) {
            tracing::error!("OSC thread error: {}", e);
        }
        tracing::info!("OSC thread stopped");
    });
    Ok(handle)
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    socket: &UdpSocket,
    router: &OscRouter,
    producer: &mut InputParameterRingBufferProducer,
    persisted: &Arc<Mutex<PersistableState>>,
    meters: &Arc<MetersOutput>,
    telemetry: &Arc<EngineTelemetry>,
    subs: &mut HashSet<SocketAddr>,
    last_broadcast: &mut Instant,
    running: &AtomicBool,
) -> Result<()> {
    let mut buf = [0u8; 8192];
    while running.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                handle_packet(&buf[..n], src, socket, router, producer, persisted, subs);
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // poll timeout — fall through to the broadcast tick check
            }
            Err(e) => return Err(e.into()),
        }

        // Broadcast tick. Cheap when there are no subscribers.
        if !subs.is_empty() && last_broadcast.elapsed() >= BROADCAST_INTERVAL {
            *last_broadcast = Instant::now();
            if let Err(e) = broadcast(socket, subs, meters, telemetry) {
                tracing::warn!("OSC: broadcast failed: {}", e);
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_packet(
    bytes: &[u8],
    src: SocketAddr,
    socket: &UdpSocket,
    router: &OscRouter,
    producer: &mut InputParameterRingBufferProducer,
    persisted: &Arc<Mutex<PersistableState>>,
    subs: &mut HashSet<SocketAddr>,
) {
    let packet = match decoder::decode_udp(bytes) {
        Ok((_, p)) => p,
        Err(e) => {
            tracing::warn!("OSC: malformed packet from {}: {:?}", src, e);
            return;
        }
    };
    visit(&packet, src, socket, router, producer, persisted, subs);
}

#[allow(clippy::too_many_arguments)]
fn visit(
    packet: &OscPacket,
    src: SocketAddr,
    socket: &UdpSocket,
    router: &OscRouter,
    producer: &mut InputParameterRingBufferProducer,
    persisted: &Arc<Mutex<PersistableState>>,
    subs: &mut HashSet<SocketAddr>,
) {
    match packet {
        OscPacket::Message(msg) => {
            match msg.addr.as_str() {
                "/list" => {
                    if let Err(e) = send_introspection(socket, src, router) {
                        tracing::warn!("OSC: introspection reply to {} failed: {}", src, e);
                    }
                    return;
                }
                "/get_all" => {
                    if let Err(e) = send_get_all(socket, src, router, persisted) {
                        tracing::warn!("OSC: get_all reply to {} failed: {}", src, e);
                    }
                    return;
                }
                "/subscribe" => {
                    if subs.insert(src) {
                        tracing::info!("OSC: subscriber added: {}", src);
                    }
                    return;
                }
                "/unsubscribe" => {
                    if subs.remove(&src) {
                        tracing::info!("OSC: subscriber removed: {}", src);
                    }
                    return;
                }
                _ => {}
            }

            if let Some(param) = router.route(&msg.addr, &msg.args) {
                if producer.try_push(param).is_err() {
                    tracing::warn!("OSC: parameter channel full, dropping update");
                }
                if let Ok(mut g) = persisted.lock() {
                    g.apply_and_mark_dirty(param);
                }
            } else {
                tracing::trace!("OSC: ignored message {} from {}", msg.addr, src);
            }
        }
        OscPacket::Bundle(b) => {
            for inner in &b.content {
                visit(inner, src, socket, router, producer, persisted, subs);
            }
        }
    }
}

fn send_introspection(
    socket: &UdpSocket,
    dest: SocketAddr,
    router: &OscRouter,
) -> std::io::Result<()> {
    for p in router.introspection() {
        let msg = match p.kind {
            OscParamKind::Float { min, max, default } => OscMessage {
                addr: "/list/item".into(),
                args: vec![
                    OscType::String(p.path.clone()),
                    OscType::String("f".into()),
                    OscType::Float(min),
                    OscType::Float(max),
                    OscType::Float(default),
                ],
            },
            OscParamKind::Bool { default } => OscMessage {
                addr: "/list/item".into(),
                args: vec![
                    OscType::String(p.path.clone()),
                    OscType::String("b".into()),
                    OscType::Float(0.0),
                    OscType::Float(1.0),
                    OscType::Float(if default { 1.0 } else { 0.0 }),
                ],
            },
        };
        let bytes = encoder::encode(&OscPacket::Message(msg))
            .map_err(|e| std::io::Error::other(format!("OSC encode: {e:?}")))?;
        socket.send_to(&bytes, dest)?;
    }
    let end = OscMessage {
        addr: "/list/end".into(),
        args: vec![],
    };
    let end_bytes = encoder::encode(&OscPacket::Message(end))
        .map_err(|e| std::io::Error::other(format!("OSC encode: {e:?}")))?;
    socket.send_to(&end_bytes, dest)?;
    tracing::info!(
        "OSC: replied with {} parameters to {}",
        router.introspection().len(),
        dest
    );
    Ok(())
}

fn send_get_all(
    socket: &UdpSocket,
    dest: SocketAddr,
    router: &OscRouter,
    persisted: &Arc<Mutex<PersistableState>>,
) -> std::io::Result<()> {
    // Take a single snapshot under the lock so values are consistent.
    let snap = match persisted.lock() {
        Ok(g) => g.state.clone(),
        Err(_) => {
            tracing::warn!("OSC: get_all could not lock persisted state");
            return Ok(());
        }
    };

    let mut count = 0;
    for p in router.introspection() {
        let Some(value) = router.current_value(&p.path, &snap) else {
            continue;
        };
        let arg = match value {
            OscValue::Float(v) => OscType::Float(v),
            OscValue::Bool(v) => OscType::Bool(v),
        };
        let msg = OscMessage {
            addr: format!("/state{}", p.path),
            args: vec![arg],
        };
        let bytes = encoder::encode(&OscPacket::Message(msg))
            .map_err(|e| std::io::Error::other(format!("OSC encode: {e:?}")))?;
        socket.send_to(&bytes, dest)?;
        count += 1;
    }
    let end = OscMessage {
        addr: "/state/end".into(),
        args: vec![],
    };
    let end_bytes = encoder::encode(&OscPacket::Message(end))
        .map_err(|e| std::io::Error::other(format!("OSC encode: {e:?}")))?;
    socket.send_to(&end_bytes, dest)?;
    tracing::info!("OSC: replied with {} state values to {}", count, dest);
    Ok(())
}

fn broadcast(
    socket: &UdpSocket,
    subs: &HashSet<SocketAddr>,
    meters: &Arc<MetersOutput>,
    telemetry: &Arc<EngineTelemetry>,
) -> std::io::Result<()> {
    let mut content: Vec<OscPacket> = Vec::with_capacity(meters.num_inputs() * 2 + 12);

    // Per-input strips
    for i in 0..meters.num_inputs() {
        let (peak, rms) = meters.load_input(i);
        content.push(make_msg(format!("/meters/input/{i}/peak"), peak));
        content.push(make_msg(format!("/meters/input/{i}/rms"), rms));
    }

    let (rp, rr) = meters.load_reverb();
    content.push(make_msg("/meters/reverb/peak".into(), rp));
    content.push(make_msg("/meters/reverb/rms".into(), rr));

    let (ep, er) = meters.load_echo();
    content.push(make_msg("/meters/echo/peak".into(), ep));
    content.push(make_msg("/meters/echo/rms".into(), er));

    let ((mlp, mlr), (mrp, mrr)) = meters.load_master();
    content.push(make_msg("/meters/master/l/peak".into(), mlp));
    content.push(make_msg("/meters/master/l/rms".into(), mlr));
    content.push(make_msg("/meters/master/r/peak".into(), mrp));
    content.push(make_msg("/meters/master/r/rms".into(), mrr));

    // Master compressor gain reduction in dB (≥ 0; 0 = no compression).
    content.push(make_msg(
        "/meters/compressor/gr_db".into(),
        meters.load_compressor_gr_db(),
    ));

    content.push(make_msg(
        "/telemetry/dsp_load".into(),
        telemetry.dsp_load_peak_pct(),
    ));
    content.push(make_int_msg(
        "/telemetry/xrun_overrun".into(),
        telemetry.overrun_count() as i32,
    ));
    content.push(make_int_msg(
        "/telemetry/xrun_underrun".into(),
        telemetry.underrun_count() as i32,
    ));

    let bundle = OscBundle {
        timetag: OscTime { seconds: 0, fractional: 0 },
        content,
    };
    let bytes = encoder::encode(&OscPacket::Bundle(bundle))
        .map_err(|e| std::io::Error::other(format!("OSC encode: {e:?}")))?;
    for dest in subs {
        socket.send_to(&bytes, dest)?;
    }
    Ok(())
}

#[inline]
fn make_msg(addr: String, v: f32) -> OscPacket {
    OscPacket::Message(OscMessage {
        addr,
        args: vec![OscType::Float(v)],
    })
}

#[inline]
fn make_int_msg(addr: String, v: i32) -> OscPacket {
    OscPacket::Message(OscMessage {
        addr,
        args: vec![OscType::Int(v)],
    })
}

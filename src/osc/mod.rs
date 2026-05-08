mod router;

pub use router::{OscParamKind, OscParameter, OscRouter};

use crate::audio::InputParameterRingBufferProducer;
use crate::persist::PersistableState;

use anyhow::{Context, Result};
use ringbuf::traits::Producer;
use rosc::{OscMessage, OscPacket, OscType, decoder, encoder};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Spawn the OSC thread.
///
/// Binds a UDP socket to `listen_addr` (e.g. `0.0.0.0:9000`), parses incoming
/// OSC packets, dispatches Control messages through the router into
/// `InputParameters`, and pushes onto `producer`. Each event also fanouts to
/// `persisted` so MIDI/UI/OSC all converge on the same state.
///
/// Introspection: external apps can send a single message addressed
/// `/list` (no args) — the server responds with one `/list/item` message per
/// known parameter, followed by a `/list/end` message.
///
/// `/list/item` argument layout:
///   args[0]  string  full OSC path (e.g. `/reverb/size`)
///   args[1]  string  type tag, `"f"` for float or `"b"` for bool
///   args[2]  float   minimum
///   args[3]  float   maximum
///   args[4]  float   default value (booleans encode 0.0 or 1.0)
pub fn run(
    running: Arc<AtomicBool>,
    producer: InputParameterRingBufferProducer,
    persisted: Arc<Mutex<PersistableState>>,
    listen_addr: SocketAddr,
    num_inputs: usize,
) -> Result<JoinHandle<()>> {
    let socket = UdpSocket::bind(listen_addr).with_context(|| {
        format!("binding OSC UDP socket to {listen_addr}")
    })?;
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .context("setting OSC read timeout")?;
    let local_addr = socket.local_addr()?;
    tracing::info!("OSC: listening on {}", local_addr);

    let router = OscRouter::new(num_inputs);
    let mut producer = producer;

    let handle = std::thread::spawn(move || {
        if let Err(e) = run_loop(socket, &router, &mut producer, &persisted, &running) {
            tracing::error!("OSC thread error: {}", e);
        }
        tracing::info!("OSC thread stopped");
    });
    Ok(handle)
}

fn run_loop(
    socket: UdpSocket,
    router: &OscRouter,
    producer: &mut InputParameterRingBufferProducer,
    persisted: &Arc<Mutex<PersistableState>>,
    running: &AtomicBool,
) -> Result<()> {
    let mut buf = [0u8; 8192];
    while running.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                handle_packet(&buf[..n], src, &socket, router, producer, persisted);
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // poll timeout — loop and re-check `running`
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn handle_packet(
    bytes: &[u8],
    src: SocketAddr,
    socket: &UdpSocket,
    router: &OscRouter,
    producer: &mut InputParameterRingBufferProducer,
    persisted: &Arc<Mutex<PersistableState>>,
) {
    let packet = match decoder::decode_udp(bytes) {
        Ok((_, p)) => p,
        Err(e) => {
            tracing::warn!("OSC: malformed packet from {}: {:?}", src, e);
            return;
        }
    };
    visit(&packet, src, socket, router, producer, persisted);
}

fn visit(
    packet: &OscPacket,
    src: SocketAddr,
    socket: &UdpSocket,
    router: &OscRouter,
    producer: &mut InputParameterRingBufferProducer,
    persisted: &Arc<Mutex<PersistableState>>,
) {
    match packet {
        OscPacket::Message(msg) => {
            if msg.addr == "/list" {
                if let Err(e) = send_introspection(socket, src, router) {
                    tracing::warn!("OSC: introspection reply to {} failed: {}", src, e);
                }
                return;
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
                visit(inner, src, socket, router, producer, persisted);
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

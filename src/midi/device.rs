use alsa::Direction;
use alsa::poll::Descriptors;
use alsa::rawmidi::Rawmidi;
use anyhow::{Context, Result};
use std::io::Read;

const READ_BUFFER_SIZE: usize = 256;

pub struct RawMidiInput {
    rawmidi: Rawmidi,
    parser: MidiParser,
    buf: [u8; READ_BUFFER_SIZE],
}

impl RawMidiInput {
    /// Open an ALSA raw MIDI capture device by name (e.g. `"hw:1,0,0"`).
    /// Set non-blocking so reads return `EAGAIN` instead of stalling when the
    /// device has no pending bytes.
    pub fn open(device: &str) -> Result<Self> {
        let rawmidi = Rawmidi::new(device, Direction::Capture, /* nonblock = */ true)
            .with_context(|| format!("opening MIDI device '{device}'"))?;
        tracing::info!("MIDI: opened raw device '{}'", device);
        Ok(Self {
            rawmidi,
            parser: MidiParser::new(),
            buf: [0; READ_BUFFER_SIZE],
        })
    }

    /// Block up to `timeout_ms` waiting for pending bytes. Returns immediately
    /// if data is already available. Used to keep the MIDI thread responsive
    /// to shutdown without busy-looping.
    pub fn poll_with_timeout(&self, timeout_ms: i32) -> Result<()> {
        let count = Descriptors::count(&self.rawmidi);
        if count == 0 {
            return Ok(());
        }
        let mut fds: Vec<libc::pollfd> = (0..count)
            .map(|_| libc::pollfd {
                fd: 0,
                events: 0,
                revents: 0,
            })
            .collect();
        Descriptors::fill(&self.rawmidi, &mut fds).context("filling poll descriptors")?;
        let _ = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
        Ok(())
    }

    /// Drain all currently available bytes, parse them, and call `f` for each
    /// Control Change event with `(channel, cc, value)`.
    pub fn drain_cc<F: FnMut(u8, u8, u8)>(&mut self, mut f: F) -> Result<()> {
        loop {
            match self.rawmidi.io().read(&mut self.buf) {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    for &b in &self.buf[..n] {
                        if let Some((channel, cc, value)) = self.parser.feed(b) {
                            f(channel, cc, value);
                        }
                    }
                }
                Err(e) => {
                    // alsa-rs surfaces ALSA's negative errno through io::Error.
                    // EAGAIN (no more data, non-blocking) appears as either
                    // ErrorKind::WouldBlock or a raw_os_error of ±EAGAIN.
                    let errno_abs = e.raw_os_error().map(|c| c.abs()).unwrap_or(0);
                    if e.kind() == std::io::ErrorKind::WouldBlock || errno_abs == libc::EAGAIN {
                        return Ok(());
                    }
                    return Err(e.into());
                }
            }
        }
    }
}

/// Streaming MIDI byte parser. Tracks running status, swallows interleaved
/// System Real-Time bytes (clocks etc), keeps the byte cursor in sync for
/// 2-byte messages we don't care about, and emits only Control Change events.
struct MidiParser {
    status: u8,
    pending_data: Option<u8>,
}

impl MidiParser {
    fn new() -> Self {
        Self {
            status: 0,
            pending_data: None,
        }
    }

    /// Feed one byte. Returns `Some((channel, cc, value))` when the byte
    /// completes a Control Change message; `None` otherwise.
    fn feed(&mut self, byte: u8) -> Option<(u8, u8, u8)> {
        // System Real-Time messages (0xF8..=0xFF): single byte, may be
        // interleaved anywhere — must NOT disturb running status.
        if byte >= 0xF8 {
            return None;
        }

        // Status byte (high bit set)
        if byte & 0x80 != 0 {
            // System Common (0xF0..=0xF7) clears running status.
            if byte >= 0xF0 {
                self.status = 0;
                self.pending_data = None;
                return None;
            }
            // Channel-message status. Reset data buffer, set running status.
            self.status = byte;
            self.pending_data = None;
            return None;
        }

        // Data byte (high bit clear); ignored if no running status.
        if self.status == 0 {
            return None;
        }

        let msg_type = self.status & 0xF0;
        let channel = self.status & 0x0F;

        match msg_type {
            // Control Change — 2 data bytes; emit on the second.
            0xB0 => {
                if let Some(cc) = self.pending_data.take() {
                    Some((channel, cc, byte))
                } else {
                    self.pending_data = Some(byte);
                    None
                }
            }
            // Other 2-byte channel messages we ignore but must consume both
            // data bytes to keep the cursor in sync (running status preserved).
            0x80 | 0x90 | 0xA0 | 0xE0 => {
                if self.pending_data.is_some() {
                    self.pending_data = None;
                } else {
                    self.pending_data = Some(byte);
                }
                None
            }
            // 1-byte channel messages (Program Change, Channel Aftertouch).
            0xC0 | 0xD0 => None,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(p: &mut MidiParser, bytes: &[u8]) -> Vec<(u8, u8, u8)> {
        let mut out = Vec::new();
        for &b in bytes {
            if let Some(ev) = p.feed(b) {
                out.push(ev);
            }
        }
        out
    }

    #[test]
    fn single_cc_message() {
        let mut p = MidiParser::new();
        let evs = feed_all(&mut p, &[0xB0, 0x07, 0x40]);
        assert_eq!(evs, vec![(0, 7, 64)]);
    }

    #[test]
    fn cc_on_channel_5() {
        let mut p = MidiParser::new();
        let evs = feed_all(&mut p, &[0xB5, 0x0A, 0x7F]);
        assert_eq!(evs, vec![(5, 10, 127)]);
    }

    #[test]
    fn running_status_emits_repeated_cc() {
        let mut p = MidiParser::new();
        let evs = feed_all(&mut p, &[0xB0, 0x07, 0x40, 0x07, 0x42, 0x08, 0x01]);
        assert_eq!(evs, vec![(0, 7, 64), (0, 7, 66), (0, 8, 1)]);
    }

    #[test]
    fn realtime_bytes_do_not_disturb_running_status() {
        let mut p = MidiParser::new();
        // 0xF8 = MIDI clock — must be transparent.
        let evs = feed_all(&mut p, &[0xB0, 0xF8, 0x07, 0xFE, 0x40]);
        assert_eq!(evs, vec![(0, 7, 64)]);
    }

    #[test]
    fn note_on_consumed_but_not_emitted() {
        let mut p = MidiParser::new();
        // Note On + 2 data bytes should produce no CC events.
        let evs = feed_all(&mut p, &[0x90, 0x40, 0x7F]);
        assert!(evs.is_empty());
    }

    #[test]
    fn status_change_clears_data_buffer() {
        let mut p = MidiParser::new();
        // Start a CC, switch mid-message to Note On — first CC byte is dropped.
        let evs = feed_all(&mut p, &[0xB0, 0x07, 0x90, 0x40, 0x7F]);
        assert!(evs.is_empty());
    }

    #[test]
    fn system_common_clears_running_status() {
        let mut p = MidiParser::new();
        // 0xF6 = Tune Request (System Common, no data) — clears running status.
        let evs = feed_all(&mut p, &[0xB0, 0x07, 0x40, 0xF6, 0x07, 0x42]);
        assert_eq!(evs, vec![(0, 7, 64)]);
    }
}

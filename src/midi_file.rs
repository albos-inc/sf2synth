//! Standard MIDI Files (formats 0, 1 and 2).

use std::io::Read;

use crate::error::{Error, Result};

/// A channel message or System Exclusive message of a MIDI file, at its
/// time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiEvent {
    /// Seconds from the start, with the file's tempo changes applied.
    pub time: f64,
    /// Ticks from the start.
    pub tick: u64,
    /// Index of the track the event is in.
    pub track: u16,
    /// Status and data bytes; `len` of them are used. A System Exclusive
    /// message only has its status (`F0`) here: get the whole message with
    /// [`MidiFile::message`].
    pub bytes: [u8; 3],
    pub len: u8,
    /// A System Exclusive message's range in the file's SysEx data.
    sysex: Option<(u32, u32)>,
}

impl MidiEvent {
    /// The channel message. For the complete bytes of a System Exclusive
    /// message, use [`MidiFile::message`].
    pub fn message(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    pub fn is_sysex(&self) -> bool {
        self.sysex.is_some()
    }

    pub fn status(&self) -> u8 {
        self.bytes[0]
    }

    pub fn channel(&self) -> u8 {
        self.bytes[0] & 0x0f
    }

    /// Whether the event is a note-on with a non-zero velocity.
    pub fn is_note_on(&self) -> bool {
        self.bytes[0] & 0xf0 == 0x90 && self.bytes[2] > 0
    }
}

/// A tempo change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoChange {
    pub tick: u64,
    pub time: f64,
    pub microseconds_per_quarter: u32,
}

/// A parsed Standard MIDI File: its channel and System Exclusive messages
/// merged into one list sorted by time.
#[derive(Clone, Debug)]
pub struct MidiFile {
    format: u16,
    track_count: u16,
    division: u16,
    events: Vec<MidiEvent>,
    /// The System Exclusive messages' bytes, `F0` to `F7`, one after another.
    sysex_data: Vec<u8>,
    tempo_changes: Vec<TempoChange>,
    duration: f64,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidMidiFile(message.into())
}

struct TrackReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl TrackReader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn variable_length(&mut self) -> Option<u32> {
        let mut value: u32 = 0;
        for _ in 0..4 {
            let b = self.byte()?;
            value = (value << 7) | (b & 0x7f) as u32;
            if b & 0x80 == 0 {
                return Some(value);
            }
        }
        Some(value)
    }

    fn skip(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }
}

/// An event of one track, before the tracks are merged.
enum Raw {
    Channel([u8; 3], u8),
    /// A System Exclusive message, `F0` to `F7`.
    SysEx(Vec<u8>),
    Tempo(u32),
    EndOfTrack,
}

impl MidiFile {
    /// Reads and parses a MIDI file.
    pub fn read<R: Read>(mut reader: R) -> Result<MidiFile> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes)
    }

    /// Parses a MIDI file. Truncated tracks are read as far as they go.
    pub fn from_bytes(data: &[u8]) -> Result<MidiFile> {
        // Some files wrap the SMF in a RIFF RMID container.
        let data = if data.len() >= 20 && &data[0..4] == b"RIFF" && &data[8..12] == b"RMID" {
            let start = data.windows(4).position(|w| w == b"MThd").ok_or_else(|| invalid("no MThd in RMID"))?;
            &data[start..]
        } else {
            data
        };
        if data.len() < 14 || &data[0..4] != b"MThd" {
            return Err(invalid("not a Standard MIDI File"));
        }
        let header_length = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        if header_length < 6 {
            return Err(invalid("short header"));
        }
        let format = u16::from_be_bytes([data[8], data[9]]);
        let declared_tracks = u16::from_be_bytes([data[10], data[11]]);
        let division = u16::from_be_bytes([data[12], data[13]]);
        if division == 0 {
            return Err(invalid("division is 0"));
        }
        if format > 2 {
            return Err(invalid(format!("unsupported format {format}")));
        }

        let mut raw: Vec<(u64, u16, u32, Raw)> = Vec::new();
        let mut pos = 8usize.saturating_add(header_length);
        let mut track: u16 = 0;
        while pos + 8 <= data.len() && track < declared_tracks.max(1).saturating_add(64) {
            let id = &data[pos..pos + 4];
            let length = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize;
            let start = pos + 8;
            let end = start.saturating_add(length).min(data.len());
            pos = start.saturating_add(length);
            if id != b"MTrk" {
                continue;
            }
            read_track(&data[start..end], track, &mut raw);
            track += 1;
        }
        if track == 0 {
            return Err(invalid("no tracks"));
        }

        // Format 2 tracks are independent songs; play them one after another.
        if format == 2 {
            let mut offset = 0;
            for t in 0..track {
                let mut last = 0;
                for (tick, event_track, _, _) in raw.iter_mut() {
                    if *event_track == t {
                        *tick += offset;
                        last = last.max(*tick);
                    }
                }
                offset = last;
            }
        }

        raw.sort_by_key(|&(tick, track, order, _)| (tick, track, order));

        let mut events = Vec::with_capacity(raw.len());
        let mut sysex_data = Vec::new();
        let mut tempo_changes = Vec::new();
        let mut tempo = 500_000u32;
        let mut last_tick = 0u64;
        let mut last_time = 0.0f64;
        let smpte = division & 0x8000 != 0;
        let seconds_per_tick = |tempo: u32| -> f64 {
            if smpte {
                let fps = match (division >> 8) as u8 as i8 {
                    -29 => 29.97,
                    fps => (-(fps as i32)) as f64,
                };
                let ticks_per_frame = (division & 0xff).max(1) as f64;
                1.0 / (fps * ticks_per_frame)
            } else {
                tempo as f64 / 1_000_000.0 / division as f64
            }
        };
        let mut max_tick = 0;
        for (tick, track, _, kind) in raw {
            let time = last_time + (tick - last_tick) as f64 * seconds_per_tick(tempo);
            last_tick = tick;
            last_time = time;
            max_tick = max_tick.max(tick);
            match kind {
                Raw::Tempo(value) => {
                    tempo = value.max(1);
                    tempo_changes.push(TempoChange { tick, time, microseconds_per_quarter: tempo });
                }
                Raw::Channel(bytes, len) => events.push(MidiEvent { time, tick, track, bytes, len, sysex: None }),
                Raw::SysEx(message) => {
                    let range = (sysex_data.len() as u32, message.len() as u32);
                    sysex_data.extend_from_slice(&message);
                    events.push(MidiEvent { time, tick, track, bytes: [0xf0, 0, 0], len: 1, sysex: Some(range) });
                }
                Raw::EndOfTrack => {}
            }
        }
        let duration = last_time + (max_tick - last_tick) as f64 * seconds_per_tick(tempo);

        Ok(MidiFile { format, track_count: track, division, events, sysex_data, tempo_changes, duration })
    }

    /// 0, 1 or 2.
    pub fn format(&self) -> u16 {
        self.format
    }

    pub fn track_count(&self) -> u16 {
        self.track_count
    }

    /// Ticks per quarter note, or SMPTE timing if the top bit is set.
    pub fn division(&self) -> u16 {
        self.division
    }

    /// Channel and System Exclusive messages sorted by time.
    pub fn events(&self) -> &[MidiEvent] {
        &self.events
    }

    /// The complete bytes of `event`: its channel message, or its System
    /// Exclusive message from `F0` to `F7`.
    pub fn message<'a>(&'a self, event: &'a MidiEvent) -> &'a [u8] {
        match event.sysex {
            Some((start, len)) => self.sysex_data.get(start as usize..(start + len) as usize).unwrap_or(&[]),
            None => event.message(),
        }
    }

    pub fn tempo_changes(&self) -> &[TempoChange] {
        &self.tempo_changes
    }

    /// Seconds to the last event (End of Track included).
    pub fn duration(&self) -> f64 {
        self.duration
    }
}

fn read_track(data: &[u8], track: u16, out: &mut Vec<(u64, u16, u32, Raw)>) {
    let mut reader = TrackReader { data, pos: 0 };
    let mut tick = 0u64;
    let mut running_status = 0u8;
    let mut order = 0u32;
    loop {
        let Some(delta) = reader.variable_length() else { return };
        tick += delta as u64;
        let Some(first) = reader.byte() else { return };
        let status = if first & 0x80 != 0 {
            first
        } else {
            if running_status == 0 {
                return;
            }
            reader.pos -= 1;
            running_status
        };
        match status {
            0xff => {
                let Some(kind) = reader.byte() else { return };
                let Some(length) = reader.variable_length() else { return };
                let Some(body) = reader.skip(length as usize) else { return };
                match kind {
                    0x51 if body.len() >= 3 => {
                        let tempo = ((body[0] as u32) << 16) | ((body[1] as u32) << 8) | body[2] as u32;
                        out.push((tick, track, order, Raw::Tempo(tempo)));
                        order += 1;
                    }
                    0x2f => {
                        // End of Track: keep its tick for the duration.
                        out.push((tick, track, order, Raw::EndOfTrack));
                        return;
                    }
                    _ => {}
                }
            }
            0xf0 => {
                let Some(length) = reader.variable_length() else { return };
                let Some(body) = reader.skip(length as usize) else { return };
                let mut message = Vec::with_capacity(body.len() + 2);
                message.push(0xf0);
                message.extend_from_slice(body);
                if message.last() != Some(&0xf7) {
                    // Continued in F7 packets, which aren't read: end it here.
                    message.push(0xf7);
                }
                out.push((tick, track, order, Raw::SysEx(message)));
                order += 1;
            }
            0xf7 => {
                // Escaped bytes or a SysEx continuation packet: skipped.
                let Some(length) = reader.variable_length() else { return };
                if reader.skip(length as usize).is_none() {
                    return;
                }
            }
            0x80..=0xef => {
                running_status = status;
                let data_bytes = if matches!(status & 0xf0, 0xc0 | 0xd0) { 1 } else { 2 };
                let mut bytes = [status, 0, 0];
                for byte in bytes.iter_mut().skip(1).take(data_bytes) {
                    let Some(b) = reader.byte() else { return };
                    *byte = b & 0x7f;
                }
                out.push((tick, track, order, Raw::Channel(bytes, 1 + data_bytes as u8)));
                order += 1;
            }
            _ => {
                // System common / real-time bytes have no place in a file;
                // skip the byte.
            }
        }
    }
}

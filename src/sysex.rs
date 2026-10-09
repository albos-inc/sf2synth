//! The System Exclusive messages the synthesizer understands.

/// A recognized System Exclusive message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SysEx {
    /// GM System On (or GM2 System On, `true`).
    GmOn { gm2: bool },
    /// GM System Off.
    GmOff,
    /// GS Reset (or the GS "System Mode Set").
    GsReset,
    /// XG System On.
    XgOn,
    /// Master volume, 0…1 (Universal, GS and XG).
    MasterVolume(f32),
    /// Master Fine Tuning, in cents (−100…+100).
    MasterFineTuning(f32),
    /// Master Coarse Tuning, in semitones (−64…+63).
    MasterCoarseTuning(f32),
    /// A channel turned into a drum part or back (GS "Use for Rhythm Part",
    /// XG "Part Mode").
    DrumPart { channel: u8, drum: bool },
    /// A GS reverb parameter (address 40 01 30…37) and its value.
    GsReverb { parameter: u8, value: u8 },
    /// A GS chorus parameter (address 40 01 38…3F) and its value.
    GsChorus { parameter: u8, value: u8 },
    /// GS Scale Tuning of a part (address 40 1x 40…4B): `count` values for
    /// the pitch classes from `start` (0 = C), each 0…127 for −64…+63 cents.
    GsScaleTuning { channel: u8, start: u8, values: [u8; 12], count: u8 },
    /// A part parameter that a control change also sets (GS and XG part
    /// level, pan, reverb and chorus send; XG bank select).
    PartControl { channel: u8, controller: u8, value: u8 },
    /// A part's program (GS tone number with its bank, XG program).
    PartProgram { channel: u8, bank: Option<u8>, program: u8 },
    /// A part's key shift, in semitones (GS Pitch Key Shift, XG Note Shift).
    PartKeyShift { channel: u8, semitones: i8 },
    /// Master key shift, in semitones (GS Master Key-Shift, XG Transpose).
    MasterKeyShift(i8),
}

const ROLAND: u8 = 0x41;
const YAMAHA: u8 = 0x43;
const NON_REAL_TIME: u8 = 0x7e;
const REAL_TIME: u8 = 0x7f;
const GS_MODEL: u8 = 0x42;
const XG_MODEL: u8 = 0x4c;

/// Recognizes a complete System Exclusive message (`F0 … F7`; the final F7
/// may be missing). Device IDs are not checked, nor are Roland checksums.
pub(crate) fn parse(message: &[u8]) -> Option<SysEx> {
    let body = message.strip_prefix(&[0xf0])?;
    let body = body.strip_suffix(&[0xf7]).unwrap_or(body);
    match *body {
        // Universal Non-Real Time: General MIDI (sub-ID 09).
        [NON_REAL_TIME, _, 0x09, 0x01, ..] => Some(SysEx::GmOn { gm2: false }),
        [NON_REAL_TIME, _, 0x09, 0x02, ..] => Some(SysEx::GmOff),
        [NON_REAL_TIME, _, 0x09, 0x03, ..] => Some(SysEx::GmOn { gm2: true }),
        // Universal Real Time: Device Control (sub-ID 04).
        [REAL_TIME, _, 0x04, 0x01, lsb, msb, ..] => Some(SysEx::MasterVolume(fourteen_bits(lsb, msb) as f32 / 16383.0)),
        [REAL_TIME, _, 0x04, 0x03, lsb, msb, ..] => {
            Some(SysEx::MasterFineTuning((fourteen_bits(lsb, msb) as f32 - 8192.0) / 8192.0 * 100.0))
        }
        [REAL_TIME, _, 0x04, 0x04, _, msb, ..] => Some(SysEx::MasterCoarseTuning(msb as f32 - 64.0)),
        // Roland GS: DT1 (12) to the GS model (42).
        [ROLAND, _, GS_MODEL, 0x12, high, middle, low, value, ref rest @ ..] => {
            // The data runs up to the checksum.
            let data = &body[7..7 + rest.len().max(1)];
            gs(high, middle, low, value & 0x7f, data)
        }
        // Yamaha XG: parameter change (1n) to the XG model (4C).
        [YAMAHA, device, XG_MODEL, high, middle, low, value, ..] if device & 0xf0 == 0x10 => {
            xg(high, middle, low, value & 0x7f)
        }
        _ => None,
    }
}

fn fourteen_bits(lsb: u8, msb: u8) -> u16 {
    (lsb & 0x7f) as u16 | (((msb & 0x7f) as u16) << 7)
}

fn gs(high: u8, middle: u8, low: u8, value: u8, data: &[u8]) -> Option<SysEx> {
    match (high, middle, low) {
        (0x40, 0x00, 0x7f) => Some(SysEx::GsReset),
        (0x00, 0x00, 0x7f) => Some(SysEx::GsReset),
        (0x40, 0x00, 0x04) => Some(SysEx::MasterVolume(value as f32 / 127.0)),
        (0x40, 0x00, 0x05) => Some(SysEx::MasterKeyShift(key_shift(value))),
        (0x40, 0x01, 0x30..=0x37) => Some(SysEx::GsReverb { parameter: low - 0x30, value }),
        (0x40, 0x01, 0x38..=0x3f) => Some(SysEx::GsChorus { parameter: low - 0x38, value }),
        (0x40, 0x10..=0x1f, 0x15) => {
            Some(SysEx::DrumPart { channel: gs_part_channel(middle & 0x0f), drum: value != 0 })
        }
        (0x40, 0x10..=0x1f, _) => gs_part(gs_part_channel(middle & 0x0f), low, value, data),
        _ => None,
    }
}

fn gs_part(channel: u8, address: u8, value: u8, data: &[u8]) -> Option<SysEx> {
    let control = |controller| Some(SysEx::PartControl { channel, controller, value });
    match address {
        // Tone number: bank select MSB, then program.
        0x00 => match *data {
            [bank, program, ..] => {
                Some(SysEx::PartProgram { channel, bank: Some(bank & 0x7f), program: program & 0x7f })
            }
            _ => Some(SysEx::PartControl { channel, controller: 0, value }),
        },
        0x01 => Some(SysEx::PartProgram { channel, bank: None, program: value }),
        0x16 => Some(SysEx::PartKeyShift { channel, semitones: key_shift(value) }),
        0x19 => control(7),
        // 0 is random pan, played centered.
        0x1c => Some(SysEx::PartControl { channel, controller: 10, value: if value == 0 { 64 } else { value } }),
        0x21 => control(93),
        0x22 => control(91),
        0x40..=0x4b => {
            let start = address - 0x40;
            let count = data.len().min(12 - start as usize);
            let mut values = [64; 12];
            for (slot, &byte) in values.iter_mut().zip(&data[..count]) {
                *slot = byte & 0x7f;
            }
            Some(SysEx::GsScaleTuning { channel, start, values, count: count as u8 })
        }
        _ => None,
    }
}

/// A key shift of 0x28…0x58 (−24…+24 semitones, 0x40 none).
fn key_shift(value: u8) -> i8 {
    (value.clamp(0x28, 0x58) as i8) - 0x40
}

/// The MIDI channel of GS part block `block` (40 1x …): block 0 is part 10,
/// blocks 1…9 are parts 1…9, blocks A…F parts 11…16.
fn gs_part_channel(block: u8) -> u8 {
    match block {
        0 => 9,
        1..=9 => block - 1,
        _ => block,
    }
}

fn xg(high: u8, middle: u8, low: u8, value: u8) -> Option<SysEx> {
    match (high, middle, low) {
        (0x00, 0x00, 0x7e) => Some(SysEx::XgOn),
        (0x00, 0x00, 0x7f) => Some(SysEx::XgOn), // All Parameter Reset
        (0x00, 0x00, 0x04) => Some(SysEx::MasterVolume(value as f32 / 127.0)),
        (0x00, 0x00, 0x06) => Some(SysEx::MasterKeyShift(key_shift(value))),
        // Multi Part (part n plays channel n).
        (0x08, channel @ 0x00..=0x0f, _) => {
            let control = |controller| Some(SysEx::PartControl { channel, controller, value });
            match low {
                0x01 => control(0),
                0x02 => control(32),
                0x03 => Some(SysEx::PartProgram { channel, bank: None, program: value }),
                // Part Mode: 0 normal, 1 drum, 2…5 drum setups.
                0x07 => Some(SysEx::DrumPart { channel, drum: value != 0 }),
                0x08 => Some(SysEx::PartKeyShift { channel, semitones: key_shift(value) }),
                0x0b => control(7),
                // 0 is random pan, played centered.
                0x0e => {
                    Some(SysEx::PartControl { channel, controller: 10, value: if value == 0 { 64 } else { value } })
                }
                0x12 => control(93),
                0x13 => control(91),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Collects a System Exclusive message that arrives in pieces (a MIDI byte
/// stream, UMP SysEx7 packets), in fixed storage.
pub(crate) struct SysExBuffer {
    bytes: [u8; SysExBuffer::CAPACITY],
    len: usize,
    overflowed: bool,
}

impl SysExBuffer {
    /// Longer messages are dropped (none the synthesizer reads is near it).
    pub const CAPACITY: usize = 256;

    pub fn new() -> SysExBuffer {
        SysExBuffer { bytes: [0; Self::CAPACITY], len: 0, overflowed: false }
    }

    /// Starts a message with `F0`.
    pub fn start(&mut self) {
        self.len = 0;
        self.overflowed = false;
        self.push(0xf0);
    }

    pub fn push(&mut self, byte: u8) {
        if self.len < Self::CAPACITY {
            self.bytes[self.len] = byte;
            self.len += 1;
        } else {
            self.overflowed = true;
        }
    }

    pub fn is_open(&self) -> bool {
        self.len > 0
    }

    /// Ends the message with `F7`: the complete message, unless it was
    /// too long.
    pub fn finish(&mut self) -> Option<&[u8]> {
        if !self.is_open() {
            return None;
        }
        self.push(0xf7);
        let len = std::mem::take(&mut self.len);
        if self.overflowed { None } else { Some(&self.bytes[..len]) }
    }

    pub fn cancel(&mut self) {
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_resets() {
        assert_eq!(parse(&[0xf0, 0x7e, 0x7f, 0x09, 0x01, 0xf7]), Some(SysEx::GmOn { gm2: false }));
        assert_eq!(parse(&[0xf0, 0x7e, 0x7f, 0x09, 0x03, 0xf7]), Some(SysEx::GmOn { gm2: true }));
        assert_eq!(parse(&[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7f, 0x00, 0x41, 0xf7]), Some(SysEx::GsReset));
        assert_eq!(parse(&[0xf0, 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7e, 0x00, 0xf7]), Some(SysEx::XgOn));
        assert_eq!(parse(&[0xf0, 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7e, 0x00]), Some(SysEx::XgOn));
        assert_eq!(parse(&[0xf0, 0x43, 0x20, 0x4c, 0x00, 0x00, 0x7e, 0x00, 0xf7]), None);
        assert_eq!(parse(&[0x90, 60, 100]), None);
    }

    #[test]
    fn reads_the_parameters() {
        assert_eq!(parse(&[0xf0, 0x7f, 0x7f, 0x04, 0x01, 0x7f, 0x7f, 0xf7]), Some(SysEx::MasterVolume(1.0)));
        assert_eq!(parse(&[0xf0, 0x7f, 0x7f, 0x04, 0x04, 0x00, 0x42, 0xf7]), Some(SysEx::MasterCoarseTuning(2.0)));
        assert_eq!(parse(&[0xf0, 0x7f, 0x7f, 0x04, 0x03, 0x00, 0x40, 0xf7]), Some(SysEx::MasterFineTuning(0.0)));
        // Part 10 (block 0), part 1 (block 1), part 11 (block A).
        for (block, channel) in [(0x10, 9), (0x11, 0), (0x1a, 10), (0x1f, 15)] {
            assert_eq!(
                parse(&[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, block, 0x15, 0x02, 0x00, 0xf7]),
                Some(SysEx::DrumPart { channel, drum: true })
            );
        }
        assert_eq!(
            parse(&[0xf0, 0x43, 0x10, 0x4c, 0x08, 0x03, 0x07, 0x01, 0xf7]),
            Some(SysEx::DrumPart { channel: 3, drum: true })
        );
        assert_eq!(
            parse(&[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x33, 0x50, 0x00, 0xf7]),
            Some(SysEx::GsReverb { parameter: 3, value: 0x50 })
        );
        let mut values = [64; 12];
        values[..3].copy_from_slice(&[0x36, 0x39, 0x3a]);
        assert_eq!(
            parse(&[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x11, 0x40, 0x36, 0x39, 0x3a, 0x00, 0xf7]),
            Some(SysEx::GsScaleTuning { channel: 0, start: 0, values, count: 3 })
        );
    }

    #[test]
    fn the_buffer_drops_long_messages() {
        let mut buffer = SysExBuffer::new();
        buffer.start();
        buffer.push(1);
        assert_eq!(buffer.finish(), Some(&[0xf0, 1, 0xf7][..]));
        assert_eq!(buffer.finish(), None);
        buffer.start();
        for _ in 0..SysExBuffer::CAPACITY {
            buffer.push(1);
        }
        assert_eq!(buffer.finish(), None);
    }
}

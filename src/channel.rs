//! The state of a MIDI channel.

const RPN_NONE: u16 = 0x3fff;

#[derive(Clone, Debug)]
pub(crate) struct Channel {
    /// Plays drum kits (bank 128).
    pub is_drum: bool,
    /// Made a drum part by an XG Part Mode message, whatever its bank.
    pub drum_part: bool,
    pub bank_msb: u8,
    pub bank_lsb: u8,
    pub program: u8,
    /// The preset playing, resolved when the program or bank changes.
    pub preset: Option<usize>,
    pub cc: [u8; 128],
    pub pitch_wheel: u16,
    pub channel_pressure: u8,
    rpn: u16,
    data_entry_is_rpn: bool,
    /// RPN 0, in semitones (MSB) plus cents (LSB) / 100.
    pub pitch_wheel_sensitivity: f32,
    /// RPN 1 (fine tuning) plus RPN 2 (coarse tuning) plus the master
    /// tuning, in cents.
    pub tuning: f32,
    /// Master Fine and Coarse Tuning (Universal SysEx), in cents.
    master_tuning: f32,
    /// GS Scale Tuning: cents added to each pitch class (0 = C).
    pub scale_tuning: [f32; 12],
    fine_tuning: u16,
    coarse_tuning: u8,
    /// Incremented on every change a voice's modulators may read.
    pub version: u32,
}

impl Channel {
    pub fn new(is_drum: bool) -> Channel {
        let mut channel = Channel {
            is_drum,
            drum_part: false,
            bank_msb: 0,
            bank_lsb: 0,
            program: 0,
            preset: None,
            cc: [0; 128],
            pitch_wheel: 8192,
            channel_pressure: 0,
            rpn: RPN_NONE,
            data_entry_is_rpn: false,
            pitch_wheel_sensitivity: 2.0,
            tuning: 0.0,
            fine_tuning: 8192,
            coarse_tuning: 64,
            master_tuning: 0.0,
            scale_tuning: [0.0; 12],
            version: 0,
        };
        channel.reset();
        channel
    }

    /// Everything back to power-on state.
    pub fn reset(&mut self) {
        self.bank_msb = 0;
        self.bank_lsb = 0;
        self.program = 0;
        self.cc = [0; 128];
        self.cc[7] = 100;
        self.cc[10] = 64;
        self.cc[11] = 127;
        self.pitch_wheel_sensitivity = 2.0;
        self.fine_tuning = 8192;
        self.coarse_tuning = 64;
        self.master_tuning = 0.0;
        self.update_tuning();
        self.reset_controllers();
    }

    /// Reset All Controllers (CC 121), as RP-015 defines it: volume, pan,
    /// bank and program are kept.
    pub fn reset_controllers(&mut self) {
        self.cc[1] = 0;
        self.cc[11] = 127;
        for pedal in 64..=69 {
            self.cc[pedal] = 0;
        }
        self.pitch_wheel = 8192;
        self.channel_pressure = 0;
        self.rpn = RPN_NONE;
        self.data_entry_is_rpn = false;
        self.version = self.version.wrapping_add(1);
    }

    pub fn sustain(&self) -> bool {
        self.cc[64] >= 64
    }

    pub fn sostenuto(&self) -> bool {
        self.cc[66] >= 64
    }

    /// Stores a control change and applies bank select and (N)RPN data
    /// entry. A bank change takes effect at the next program change.
    pub fn control_change(&mut self, controller: u8, value: u8) {
        let controller = controller & 0x7f;
        let value = value & 0x7f;
        self.cc[controller as usize] = value;
        match controller {
            0 => self.bank_msb = value,
            32 => self.bank_lsb = value,
            6 => self.data_entry(Some(value), None),
            38 => self.data_entry(None, Some(value)),
            96 => self.data_increment(1),
            97 => self.data_increment(-1),
            98 | 99 => self.data_entry_is_rpn = false,
            100 => {
                self.rpn = (self.rpn & 0x3f80) | value as u16;
                self.data_entry_is_rpn = true;
            }
            101 => {
                self.rpn = (self.rpn & 0x7f) | ((value as u16) << 7);
                self.data_entry_is_rpn = true;
            }
            _ => {}
        }
        self.version = self.version.wrapping_add(1);
    }

    fn data_entry(&mut self, msb: Option<u8>, lsb: Option<u8>) {
        if !self.data_entry_is_rpn {
            return;
        }
        match self.rpn {
            0 => {
                let semitones = msb.map(f32::from).unwrap_or(self.pitch_wheel_sensitivity.trunc());
                let cents = lsb.map(f32::from).unwrap_or(if msb.is_some() {
                    0.0
                } else {
                    (self.pitch_wheel_sensitivity.fract() * 100.0).round()
                });
                self.pitch_wheel_sensitivity = semitones + cents.min(99.0) / 100.0;
            }
            1 => {
                if let Some(msb) = msb {
                    self.fine_tuning = (self.fine_tuning & 0x7f) | ((msb as u16) << 7);
                }
                if let Some(lsb) = lsb {
                    self.fine_tuning = (self.fine_tuning & 0x3f80) | lsb as u16;
                }
                self.update_tuning();
            }
            2 => {
                if let Some(msb) = msb {
                    self.coarse_tuning = msb;
                    self.update_tuning();
                }
            }
            _ => {}
        }
    }

    fn data_increment(&mut self, step: i32) {
        if self.data_entry_is_rpn && self.rpn == 0 {
            self.pitch_wheel_sensitivity = (self.pitch_wheel_sensitivity + step as f32).clamp(0.0, 127.0);
        }
    }

    fn update_tuning(&mut self) {
        let fine = (self.fine_tuning as f32 - 8192.0) / 8192.0 * 100.0;
        let coarse = (self.coarse_tuning as f32 - 64.0) * 100.0;
        self.tuning = fine + coarse + self.master_tuning;
        self.version = self.version.wrapping_add(1);
    }

    pub fn set_master_tuning(&mut self, cents: f32) {
        self.master_tuning = cents;
        self.update_tuning();
    }

    pub fn set_pitch_wheel(&mut self, value: u16) {
        self.pitch_wheel = value.min(16383);
        self.version = self.version.wrapping_add(1);
    }

    pub fn set_channel_pressure(&mut self, value: u8) {
        self.channel_pressure = value & 0x7f;
        self.version = self.version.wrapping_add(1);
    }
}

//! MOS 6581 SID — audio synthesis (no analog filter yet).
//!
//! Three voices, each a 24-bit phase-accumulator oscillator with the four
//! waveforms (triangle, sawtooth, pulse, noise), ring mod, and an ADSR
//! envelope using the SID rate table + the exponential decay/release
//! approximation. Voices are summed and scaled by the master volume.
//!
//! `clock(cycles)` advances the chip by that many ~1 MHz SID clocks and pushes
//! 44.1 kHz mono samples into an internal buffer that the frontend drains.
//! The analog filter ($D415–$D418 routing) is not modelled: every voice goes
//! straight to the output, so music plays but filtered timbres differ.

const SID_CLOCK: f64 = 985_248.0; // PAL
pub const SAMPLE_RATE: u32 = 44_100;

/// Envelope rate-counter periods (SID clocks) indexed by the 4-bit ADSR value.
const RATE: [u32; 16] = [
    9, 32, 63, 95, 149, 220, 267, 313, 392, 977, 1954, 3126, 3907, 11720, 19532, 31251,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvState {
    Attack,
    Decay,
    Release,
}

#[derive(Clone, Copy)]
struct Voice {
    freq: u16,
    pw: u16, // 12-bit pulse width
    control: u8,
    ad: u8,
    sr: u8,

    acc: u32, // 24-bit phase accumulator
    lfsr: u32, // 23-bit noise shift register

    env: u16, // 0..255
    state: EnvState,
    rate_counter: u32,
    exp_counter: u32,
    gate: bool,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            freq: 0,
            pw: 0,
            control: 0,
            ad: 0,
            sr: 0,
            acc: 0,
            lfsr: 0x7F_FFFF,
            env: 0,
            state: EnvState::Release,
            rate_counter: 0,
            exp_counter: 0,
            gate: false,
        }
    }
}

impl Voice {
    fn set_control(&mut self, v: u8) {
        let new_gate = v & 0x01 != 0;
        if new_gate && !self.gate {
            self.state = EnvState::Attack;
        } else if !new_gate && self.gate {
            self.state = EnvState::Release;
        }
        self.gate = new_gate;
        if v & 0x08 != 0 {
            self.acc = 0; // test bit holds the oscillator reset
        }
        self.control = v;
    }

    fn clock_osc(&mut self) {
        if self.control & 0x08 != 0 {
            self.acc = 0;
            return;
        }
        let prev = self.acc;
        self.acc = (self.acc + self.freq as u32) & 0x00FF_FFFF;
        // Noise LFSR advances when accumulator bit 19 rises.
        if prev & 0x0008_0000 == 0 && self.acc & 0x0008_0000 != 0 {
            let bit = ((self.lfsr >> 22) ^ (self.lfsr >> 17)) & 1;
            self.lfsr = ((self.lfsr << 1) | bit) & 0x7F_FFFF;
        }
    }

    /// 12-bit waveform output. `ring_msb` is the ring-mod source's top bit.
    fn waveform(&self, ring_msb: u32) -> u16 {
        let ctrl = self.control;
        if ctrl & 0xF0 == 0 {
            return 0;
        }
        let mut out = 0x0FFFu16; // combined waveforms are AND-ed together
        if ctrl & 0x10 != 0 {
            // Triangle.
            let msb = if ctrl & 0x04 != 0 {
                ((self.acc >> 23) ^ ring_msb) & 1
            } else {
                (self.acc >> 23) & 1
            };
            let mut t = ((self.acc >> 11) & 0x0FFF) as u16;
            if msb != 0 {
                t ^= 0x0FFF;
            }
            out &= t;
        }
        if ctrl & 0x20 != 0 {
            // Sawtooth.
            out &= ((self.acc >> 12) & 0x0FFF) as u16;
        }
        if ctrl & 0x40 != 0 {
            // Pulse.
            let p = if ((self.acc >> 12) & 0x0FFF) >= self.pw as u32 { 0x0FFF } else { 0 };
            out &= p;
        }
        if ctrl & 0x80 != 0 {
            // Noise: assemble 8 bits of the LFSR into the high output bits.
            let l = self.lfsr;
            let n = (((l >> 20) & 1) << 11)
                | (((l >> 18) & 1) << 10)
                | (((l >> 14) & 1) << 9)
                | (((l >> 11) & 1) << 8)
                | (((l >> 9) & 1) << 7)
                | (((l >> 5) & 1) << 6)
                | (((l >> 2) & 1) << 5)
                | ((l & 1) << 4);
            out &= n as u16;
        }
        out
    }

    /// Exponential decay/release period multiplier as a function of envelope.
    fn exp_period(&self) -> u32 {
        match self.env {
            0 => 1,
            1..=5 => 30,
            6..=13 => 16,
            14..=25 => 8,
            26..=53 => 4,
            54..=93 => 2,
            _ => 1,
        }
    }

    fn clock_env(&mut self) {
        let period = match self.state {
            EnvState::Attack => RATE[(self.ad >> 4) as usize],
            EnvState::Decay => RATE[(self.ad & 0x0F) as usize],
            EnvState::Release => RATE[(self.sr & 0x0F) as usize],
        };
        self.rate_counter += 1;
        if self.rate_counter < period {
            return;
        }
        self.rate_counter = 0;

        match self.state {
            EnvState::Attack => {
                if self.env >= 254 {
                    self.env = 255;
                    self.state = EnvState::Decay;
                } else {
                    self.env += 1;
                }
            }
            EnvState::Decay => {
                self.exp_counter += 1;
                if self.exp_counter >= self.exp_period() {
                    self.exp_counter = 0;
                    let sustain = (self.sr >> 4) as u16 * 0x11; // 0..255
                    if self.env > sustain {
                        self.env -= 1;
                    }
                }
            }
            EnvState::Release => {
                self.exp_counter += 1;
                if self.exp_counter >= self.exp_period() {
                    self.exp_counter = 0;
                    if self.env > 0 {
                        self.env -= 1;
                    }
                }
            }
        }
    }
}

pub struct Sid {
    regs: [u8; 0x20],
    voices: [Voice; 3],
    sample_acc: f64,
    samples: Vec<i16>,
}

impl Default for Sid {
    fn default() -> Self {
        Sid {
            regs: [0; 0x20],
            voices: [Voice::default(); 3],
            sample_acc: 0.0,
            samples: Vec::new(),
        }
    }
}

impl Sid {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write(&mut self, reg: u8, value: u8) {
        let reg = (reg & 0x1F) as usize;
        if reg < 21 {
            let v = &mut self.voices[reg / 7];
            match reg % 7 {
                0 => v.freq = (v.freq & 0xFF00) | value as u16,
                1 => v.freq = (v.freq & 0x00FF) | ((value as u16) << 8),
                2 => v.pw = (v.pw & 0x0F00) | value as u16,
                3 => v.pw = (v.pw & 0x00FF) | (((value & 0x0F) as u16) << 8),
                4 => v.set_control(value),
                5 => v.ad = value,
                6 => v.sr = value,
                _ => {}
            }
        }
        self.regs[reg] = value;
    }

    pub fn read(&self, reg: u8) -> u8 {
        match reg & 0x1F {
            0x1B => (self.voices[2].waveform(0) >> 4) as u8, // OSC3 high bits
            0x1C => self.voices[2].env as u8,                // ENV3
            _ => 0,                                          // write-only registers
        }
    }

    /// Advance the chip by `cycles` SID clocks, emitting 44.1 kHz samples.
    pub fn clock(&mut self, cycles: u64) {
        let step = SAMPLE_RATE as f64 / SID_CLOCK;
        for _ in 0..cycles {
            for v in &mut self.voices {
                v.clock_osc();
                v.clock_env();
            }
            self.sample_acc += step;
            if self.sample_acc >= 1.0 {
                self.sample_acc -= 1.0;
                self.samples.push(self.mix());
            }
        }
    }

    fn mix(&self) -> i16 {
        // Ring-mod sources: v0<-v2, v1<-v0, v2<-v1.
        let ring = [
            self.voices[2].acc >> 23,
            self.voices[0].acc >> 23,
            self.voices[1].acc >> 23,
        ];
        let mut acc: i32 = 0;
        let v3_off = self.regs[0x18] & 0x80 != 0; // voice 3 disconnect
        for i in 0..3 {
            if i == 2 && v3_off {
                continue;
            }
            let w = self.voices[i].waveform(ring[i]) as i32 - 2048;
            let e = self.voices[i].env as i32;
            acc += w * e / 256;
        }
        let vol = (self.regs[0x18] & 0x0F) as i32;
        let s = acc * vol / 15 * 4;
        s.clamp(-32768, 32767) as i16
    }

    /// Take the samples generated since the last call.
    pub fn take_samples(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_samples_at_rate() {
        let mut sid = Sid::new();
        sid.clock(SID_CLOCK as u64); // one second of SID time
        let n = sid.take_samples().len();
        // Should be ~44100 samples (within rounding).
        assert!((n as i32 - SAMPLE_RATE as i32).abs() < 100, "got {n}");
    }

    #[test]
    fn gated_voice_rises_then_releases() {
        let mut sid = Sid::new();
        sid.write(0x05, 0x00); // attack=0 (fast), decay=0
        sid.write(0x06, 0xF0); // sustain=15 (full), release=0
        sid.write(0x01, 0x10); // some frequency
        sid.write(0x04, 0x11); // pulse... actually triangle+gate (0x10|0x01)
        sid.clock(5000);
        assert!(sid.voices[0].env > 0, "envelope should have risen with gate on");
        sid.write(0x04, 0x10); // gate off -> release
        sid.clock(50000);
        assert_eq!(sid.voices[0].env, 0, "envelope should release to 0");
    }
}

//! MOS 6526 CIA (Complex Interface Adapter).
//!
//! The C64 has two: CIA #1 at `$DC00` (keyboard/joystick, its IRQ line drives
//! the CPU IRQ and the jiffy clock) and CIA #2 at `$DD00` (serial bus, VIC bank
//! select, user port; its IRQ line drives the CPU NMI).
//!
//! This is a first, boot-focused implementation: both 16-bit timers with the
//! phi2 counting mode, the interrupt-control register (ICR) semantics, the
//! data ports with DDRs, and a keyboard matrix for CIA #1. Timer B chained to
//! Timer A underflows, the serial shift register, and a full TOD clock are
//! stubbed/simplified for now — enough for the KERNAL to boot and tick.

/// Register offsets within a CIA's 16-byte block.
mod reg {
    pub const PRA: u8 = 0x0;
    pub const PRB: u8 = 0x1;
    pub const DDRA: u8 = 0x2;
    pub const DDRB: u8 = 0x3;
    pub const TA_LO: u8 = 0x4;
    pub const TA_HI: u8 = 0x5;
    pub const TB_LO: u8 = 0x6;
    pub const TB_HI: u8 = 0x7;
    pub const TOD_10TH: u8 = 0x8;
    pub const TOD_SEC: u8 = 0x9;
    pub const TOD_MIN: u8 = 0xA;
    pub const TOD_HR: u8 = 0xB;
    pub const SDR: u8 = 0xC;
    pub const ICR: u8 = 0xD;
    pub const CRA: u8 = 0xE;
    pub const CRB: u8 = 0xF;
}

/// ICR / control-register bit masks.
mod bits {
    pub const ICR_TA: u8 = 1 << 0; // Timer A underflow
    pub const ICR_TB: u8 = 1 << 1; // Timer B underflow
    pub const ICR_IRQ: u8 = 1 << 7; // any enabled interrupt occurred

    pub const CR_START: u8 = 1 << 0;
    pub const CR_ONESHOT: u8 = 1 << 3;
    pub const CR_FORCELOAD: u8 = 1 << 4;
    pub const CRA_INMODE: u8 = 1 << 5; // 0 = count phi2
}

pub struct Cia {
    // Data ports + direction.
    pub pra: u8,
    pub prb: u8,
    pub ddra: u8,
    pub ddrb: u8,

    // Timers.
    timer_a: u16,
    timer_b: u16,
    latch_a: u16,
    latch_b: u16,
    cra: u8,
    crb: u8,

    // Interrupts.
    icr_data: u8, // which interrupt sources have fired (latched)
    icr_mask: u8, // which sources are enabled
    irq_line: bool,

    // Serial + TOD (minimal).
    sdr: u8,
    tod: [u8; 4],

    /// When true, port B reads from the keyboard matrix selected by port A
    /// (CIA #1 behaviour). `keyboard[col]` holds the 8 row bits for that
    /// column; a 0 bit means the key in that row is pressed.
    pub keyboard_enabled: bool,
    pub keyboard: [u8; 8],

    /// Joystick lines, active-low (bit clear = pressed). `joy_a` overlays port A
    /// (control port 2), `joy_b` overlays port B (control port 1). Bits:
    /// 0=up 1=down 2=left 3=right 4=fire. Default $FF = nothing pressed.
    pub joy_a: u8,
    pub joy_b: u8,
}

impl Default for Cia {
    fn default() -> Self {
        Cia {
            pra: 0xFF,
            prb: 0xFF,
            ddra: 0,
            ddrb: 0,
            timer_a: 0xFFFF,
            timer_b: 0xFFFF,
            latch_a: 0xFFFF,
            latch_b: 0xFFFF,
            cra: 0,
            crb: 0,
            icr_data: 0,
            icr_mask: 0,
            irq_line: false,
            sdr: 0,
            tod: [0; 4],
            keyboard_enabled: false,
            keyboard: [0xFF; 8],
            joy_a: 0xFF,
            joy_b: 0xFF,
        }
    }
}

impl Cia {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct CIA #1 (keyboard port B behaviour enabled).
    pub fn cia1() -> Self {
        Cia { keyboard_enabled: true, ..Self::default() }
    }

    /// Current state of the IRQ output line (high = asserted).
    pub fn irq(&self) -> bool {
        self.irq_line
    }

    fn recompute_irq(&mut self) {
        self.irq_line = self.icr_data & self.icr_mask & 0x7F != 0;
    }

    /// Read keyboard port B: AND together the matrix columns selected (driven
    /// low) by port A. Bits with no selected column read high.
    fn read_keyboard_portb(&self) -> u8 {
        let mut result = 0xFFu8;
        for col in 0..8 {
            if self.pra & (1 << col) == 0 {
                result &= self.keyboard[col];
            }
        }
        result
    }

    pub fn read(&mut self, offset: u8) -> u8 {
        match offset & 0x0F {
            // Output bits read their latch; input bits float high (pulled up).
            // External devices (joystick) can only pull lines low, so AND them in.
            reg::PRA => (self.pra | !self.ddra) & self.joy_a,
            reg::PRB => {
                let base = if self.keyboard_enabled {
                    self.read_keyboard_portb()
                } else {
                    self.prb | !self.ddrb
                };
                base & self.joy_b
            }
            reg::DDRA => self.ddra,
            reg::DDRB => self.ddrb,
            reg::TA_LO => (self.timer_a & 0xFF) as u8,
            reg::TA_HI => (self.timer_a >> 8) as u8,
            reg::TB_LO => (self.timer_b & 0xFF) as u8,
            reg::TB_HI => (self.timer_b >> 8) as u8,
            reg::TOD_10TH => self.tod[0],
            reg::TOD_SEC => self.tod[1],
            reg::TOD_MIN => self.tod[2],
            reg::TOD_HR => self.tod[3],
            reg::SDR => self.sdr,
            reg::ICR => {
                // Reading ICR returns the latched data (with the summary bit in
                // bit 7) and clears it, deasserting the IRQ line.
                let mut v = self.icr_data & 0x7F;
                if self.irq_line {
                    v |= bits::ICR_IRQ;
                }
                self.icr_data = 0;
                self.irq_line = false;
                v
            }
            reg::CRA => self.cra,
            reg::CRB => self.crb,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u8, value: u8) {
        match offset & 0x0F {
            reg::PRA => self.pra = value,
            reg::PRB => self.prb = value,
            reg::DDRA => self.ddra = value,
            reg::DDRB => self.ddrb = value,
            reg::TA_LO => self.latch_a = (self.latch_a & 0xFF00) | value as u16,
            reg::TA_HI => {
                self.latch_a = (self.latch_a & 0x00FF) | ((value as u16) << 8);
                // Writing the high byte while the timer is stopped also loads
                // the counter.
                if self.cra & bits::CR_START == 0 {
                    self.timer_a = self.latch_a;
                }
            }
            reg::TB_LO => self.latch_b = (self.latch_b & 0xFF00) | value as u16,
            reg::TB_HI => {
                self.latch_b = (self.latch_b & 0x00FF) | ((value as u16) << 8);
                if self.crb & bits::CR_START == 0 {
                    self.timer_b = self.latch_b;
                }
            }
            reg::TOD_10TH => self.tod[0] = value,
            reg::TOD_SEC => self.tod[1] = value,
            reg::TOD_MIN => self.tod[2] = value,
            reg::TOD_HR => self.tod[3] = value,
            reg::SDR => self.sdr = value,
            reg::ICR => {
                // Bit 7 = set/clear; the low bits select which mask bits change.
                if value & 0x80 != 0 {
                    self.icr_mask |= value & 0x7F;
                } else {
                    self.icr_mask &= !(value & 0x7F);
                }
                self.recompute_irq();
            }
            reg::CRA => {
                if value & bits::CR_FORCELOAD != 0 {
                    self.timer_a = self.latch_a;
                }
                self.cra = value & !bits::CR_FORCELOAD; // force-load bit is a strobe
            }
            reg::CRB => {
                if value & bits::CR_FORCELOAD != 0 {
                    self.timer_b = self.latch_b;
                }
                self.crb = value & !bits::CR_FORCELOAD;
            }
            _ => {}
        }
    }

    /// Advance the timers by `cycles` phi2 clocks. Returns the IRQ line state.
    pub fn tick(&mut self, cycles: u64) -> bool {
        for _ in 0..cycles {
            self.tick_one();
        }
        self.irq_line
    }

    fn tick_one(&mut self) {
        // Timer A: count phi2 when started and in phi2 input mode.
        if self.cra & bits::CR_START != 0 && self.cra & bits::CRA_INMODE == 0 {
            if self.timer_a == 0 {
                self.timer_a = self.latch_a;
                self.icr_data |= bits::ICR_TA;
                if self.cra & bits::CR_ONESHOT != 0 {
                    self.cra &= !bits::CR_START;
                }
            } else {
                self.timer_a -= 1;
            }
        }

        // Timer B: phi2 mode only for now (chaining to TA underflow is TODO).
        if self.crb & bits::CR_START != 0 {
            if self.timer_b == 0 {
                self.timer_b = self.latch_b;
                self.icr_data |= bits::ICR_TB;
                if self.crb & bits::CR_ONESHOT != 0 {
                    self.crb &= !bits::CR_START;
                }
            } else {
                self.timer_b -= 1;
            }
        }

        self.recompute_irq();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_a_underflow_raises_irq_when_enabled() {
        let mut cia = Cia::new();
        // Enable Timer A interrupt: write ICR with bit7 set + ICR_TA.
        cia.write(reg::ICR, 0x80 | bits::ICR_TA);
        // Latch = 3, force-load, start, continuous, phi2 mode.
        cia.write(reg::TA_LO, 3);
        cia.write(reg::TA_HI, 0);
        cia.write(reg::CRA, bits::CR_START | bits::CR_FORCELOAD);

        // Counter starts at 3; it underflows after 4 ticks (3,2,1,0->reload).
        assert!(!cia.tick(3));
        assert!(cia.tick(1), "IRQ line should assert on underflow");

        // Reading ICR reports the source and clears the line.
        let icr = cia.read(reg::ICR);
        assert!(icr & bits::ICR_IRQ != 0);
        assert!(icr & bits::ICR_TA != 0);
        assert!(!cia.irq());
    }

    #[test]
    fn underflow_without_mask_does_not_assert_irq() {
        let mut cia = Cia::new();
        cia.write(reg::TA_LO, 1);
        cia.write(reg::TA_HI, 0);
        cia.write(reg::CRA, bits::CR_START | bits::CR_FORCELOAD);
        // No ICR enable written -> line stays low even though TA flag latches.
        cia.tick(4);
        assert!(!cia.irq());
        let icr = cia.read(reg::ICR);
        assert!(icr & bits::ICR_TA != 0, "flag still latches in data reg");
    }

    #[test]
    fn oneshot_stops_after_underflow() {
        let mut cia = Cia::new();
        cia.write(reg::TA_LO, 2);
        cia.write(reg::TA_HI, 0);
        cia.write(reg::CRA, bits::CR_START | bits::CR_ONESHOT | bits::CR_FORCELOAD);
        cia.tick(3); // 2,1,0->underflow+reload, start cleared
        assert_eq!(cia.read(reg::CRA) & bits::CR_START, 0);
    }

    #[test]
    fn keyboard_matrix_read() {
        let mut cia = Cia::cia1();
        // Press the key at column 1, row 5 (clear bit 5 of that column).
        cia.keyboard[1] = !(1 << 5);
        // Select column 1 by driving port A bit 1 low (others high).
        cia.write(reg::PRA, !(1 << 1));
        let pb = cia.read(reg::PRB);
        assert_eq!(pb & (1 << 5), 0, "row 5 should read low for the pressed key");
    }
}

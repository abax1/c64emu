//! MOS 6569 VIC-II — minimal implementation.
//!
//! This is *not* the video chip yet: it draws nothing. Its only job right now is
//! to make time pass in a way the KERNAL expects, so raster-wait loops (e.g.
//! `LDA $D012 / BNE`) terminate and the machine can finish booting.
//!
//! What it models:
//!   - A free-running raster line counter (`$D012` low byte, `$D011` bit 7 high
//!     bit), advancing on the PAL timing of 63 cycles/line, 312 lines/frame.
//!   - The raster compare value (written via `$D012` and `$D011` bit 7).
//!   - The interrupt latch `$D019` and enable `$D01A`, with the raster-compare
//!     source — so a KERNAL that uses raster IRQs (and the PAL/NTSC detection)
//!     sees sensible values.
//!   - Register mirroring: the 47 registers repeat every 64 bytes across
//!     `$D000–$D3FF`.
//!
//! Real sprite/graphics rendering is a later milestone.

const CYCLES_PER_LINE: u32 = 63; // PAL 6569
const LINES_PER_FRAME: u16 = 312; // PAL

/// VIC-II register offsets we care about.
mod reg {
    pub const CR1: usize = 0x11; // control register 1 (raster bit 8 = bit 7)
    pub const RASTER: usize = 0x12; // raster line low byte / compare
    pub const IRR: usize = 0x19; // interrupt request (latch)
    pub const IMR: usize = 0x1A; // interrupt mask (enable)
}

pub struct Vic {
    /// Backing store for the 47 registers ($D000–$D02E).
    regs: [u8; 0x40],

    /// Current raster line (0..LINES_PER_FRAME-1).
    raster: u16,
    /// Cycle accumulator within the current line.
    line_cycles: u32,
    /// Raster line at which a compare IRQ fires.
    raster_compare: u16,

    /// $D019 interrupt latch (bit0 = raster compare hit).
    irq_latch: u8,
    /// $D01A interrupt enable mask.
    irq_enable: u8,
}

impl Default for Vic {
    fn default() -> Self {
        Vic {
            regs: [0; 0x40],
            raster: 0,
            line_cycles: 0,
            raster_compare: 0,
            irq_latch: 0,
            irq_enable: 0,
        }
    }
}

impl Vic {
    pub fn new() -> Self {
        Self::default()
    }

    /// Direct read of a register's stored value (for the renderer; no side
    /// effects, banking-independent). Use for the colour registers $D020/$D021.
    pub fn reg(&self, idx: usize) -> u8 {
        self.regs[idx & 0x3F]
    }

    /// Current raster line.
    pub fn raster(&self) -> u16 {
        self.raster
    }

    /// Current IRQ line state (asserted when an enabled source has latched).
    pub fn irq(&self) -> bool {
        self.irq_latch & self.irq_enable & 0x0F != 0
    }

    /// Advance the raster by `cycles` and return the IRQ line state.
    pub fn tick(&mut self, cycles: u64) -> bool {
        self.line_cycles += cycles as u32;
        while self.line_cycles >= CYCLES_PER_LINE {
            self.line_cycles -= CYCLES_PER_LINE;
            self.raster += 1;
            if self.raster >= LINES_PER_FRAME {
                self.raster = 0;
            }
            if self.raster == self.raster_compare {
                self.irq_latch |= 0x01; // raster compare source
            }
        }
        self.irq()
    }

    pub fn read(&mut self, addr: u16) -> u8 {
        let r = (addr & 0x3F) as usize;
        match r {
            reg::CR1 => (self.regs[reg::CR1] & 0x7F) | (((self.raster >> 8) as u8 & 0x01) << 7),
            reg::RASTER => (self.raster & 0xFF) as u8,
            reg::IRR => {
                // Unused bits read as 1; bit 7 set when an enabled IRQ is pending.
                let mut v = self.irq_latch | 0x70;
                if self.irq() {
                    v |= 0x80;
                }
                v
            }
            reg::IMR => self.irq_enable | 0xF0,
            0x2F..=0x3F => 0xFF, // unconnected registers read as $FF
            _ => self.regs[r],
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        let r = (addr & 0x3F) as usize;
        match r {
            reg::CR1 => {
                self.regs[reg::CR1] = value;
                // Bit 7 is raster-compare bit 8.
                self.raster_compare =
                    (self.raster_compare & 0x00FF) | (((value & 0x80) as u16) << 1);
            }
            reg::RASTER => {
                self.raster_compare = (self.raster_compare & 0x0100) | value as u16;
            }
            reg::IRR => {
                // Writing 1s acknowledges (clears) latched sources.
                self.irq_latch &= !(value & 0x0F);
            }
            reg::IMR => self.irq_enable = value & 0x0F,
            _ => self.regs[r] = value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_advances_and_wraps() {
        let mut vic = Vic::new();
        // One full line.
        vic.tick(CYCLES_PER_LINE as u64);
        assert_eq!(vic.read(0xD012), 1);
        // Advance to the last line, then one more wraps to 0.
        vic.tick((CYCLES_PER_LINE as u64) * (LINES_PER_FRAME as u64 - 1));
        assert_eq!(vic.raster, 0);
        assert_eq!(vic.read(0xD012), 0);
    }

    #[test]
    fn raster_bit8_in_cr1() {
        let mut vic = Vic::new();
        // Advance past line 255 so the high bit appears in $D011 bit 7.
        vic.tick((CYCLES_PER_LINE as u64) * 256);
        assert_eq!(vic.read(0xD012), 0); // low byte of 256
        assert_eq!(vic.read(0xD011) & 0x80, 0x80);
    }

    #[test]
    fn raster_compare_latches_irq_when_enabled() {
        let mut vic = Vic::new();
        vic.write(0xD012, 5); // compare line 5
        vic.write(0xD01A, 0x01); // enable raster IRQ
        vic.tick((CYCLES_PER_LINE as u64) * 5);
        assert!(vic.irq());
        assert!(vic.read(0xD019) & 0x80 != 0);
        // Acknowledge clears it.
        vic.write(0xD019, 0x01);
        assert!(!vic.irq());
    }
}

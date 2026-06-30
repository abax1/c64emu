//! High-level `System`: ties the CPU and bus together and runs the machine in
//! frame-sized chunks, delivering device interrupts to the CPU.

use crate::bus::C64Bus;
use crate::cpu::Cpu;
use crate::roms::{self, RomError};
use crate::video::{self, FB_H, FB_W};
use std::path::Path;

/// PAL frame timing: 312 raster lines × 63 cycles.
pub const CYCLES_PER_FRAME: u64 = 312 * 63;
const LINES_PER_FRAME: u16 = 312;

pub struct System {
    pub cpu: Cpu,
    pub bus: C64Bus,
    /// RGB framebuffer produced by the VIC renderer (FB_W × FB_H).
    pub framebuffer: Vec<u8>,
}

impl Default for System {
    fn default() -> Self {
        Self::new()
    }
}

impl System {
    pub fn new() -> Self {
        System {
            cpu: Cpu::new(),
            bus: C64Bus::new(),
            framebuffer: vec![0u8; FB_W * FB_H * 3],
        }
    }

    /// Load the system ROMs from `dir` (basic/kernal/chargen).
    pub fn load_roms(&mut self, dir: &Path) -> Result<(), RomError> {
        roms::load_system_roms(&mut self.bus, dir)
    }

    /// Power-on / reset: jump through the reset vector.
    pub fn reset(&mut self) {
        self.cpu.reset(&mut self.bus);
    }

    /// Run one PAL frame's worth of cycles, stepping the CPU and ticking the
    /// time-dependent devices, wiring their interrupt lines back to the CPU.
    pub fn run_frame(&mut self) {
        let target = self.cpu.cycles + CYCLES_PER_FRAME;
        while self.cpu.cycles < target {
            let prev_raster = self.bus.io.vic.raster();
            let cycles = self.cpu.step(&mut self.bus);
            let (irq, nmi) = self.bus.tick(cycles);
            self.cpu.irq_pending = irq; // level-sensitive
            if nmi {
                self.cpu.nmi(); // edge-detected inside the CPU
            }
            // Render every raster line the instruction just completed, using the
            // registers as they stand now (good enough for raster-split effects).
            let now = self.bus.io.vic.raster();
            let mut r = prev_raster;
            while r != now {
                video::render_line(&self.bus, r, &mut self.framebuffer);
                r = (r + 1) % LINES_PER_FRAME;
            }
        }
    }

    /// Press (`pressed = true`) or release a key at CIA #1 matrix position
    /// `(column, row)`. A pressed key pulls its row bit low.
    pub fn key(&mut self, column: u8, row: u8, pressed: bool) {
        let kb = &mut self.bus.io.cia1.keyboard;
        let c = column as usize & 7;
        if pressed {
            kb[c] &= !(1 << (row & 7));
        } else {
            kb[c] |= 1 << (row & 7);
        }
    }

    /// Read VIC colour/control register `idx` ($D0xx low byte) for rendering.
    pub fn vic_reg(&self, idx: usize) -> u8 {
        self.bus.io.vic.reg(idx)
    }

    /// Take the SID audio samples generated since the last call (44.1 kHz mono).
    pub fn take_audio(&mut self) -> Vec<i16> {
        self.bus.io.sid.take_samples()
    }

    /// Set control-port-2 joystick state (CIA #1 port A). Each argument is
    /// whether that direction/fire is currently held.
    pub fn set_joy2(&mut self, up: bool, down: bool, left: bool, right: bool, fire: bool) {
        let mut m = 0xFFu8;
        for (held, bit) in [(up, 0), (down, 1), (left, 2), (right, 3), (fire, 4)] {
            if held {
                m &= !(1u8 << bit);
            }
        }
        self.bus.io.cia1.joy_a = m;
    }
}

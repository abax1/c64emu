//! MOS 6510 CPU core.
//!
//! Implements the documented NMOS 6502 instruction set (the 6510 is a 6502 with
//! an added I/O port, handled by the bus). Decimal mode is implemented because
//! the C64 KERNAL/BASIC rely on it. Cycle counts include page-cross and
//! taken-branch penalties. Undocumented opcodes are treated as variable-length
//! NOPs for now (a later pass can add the stable illegals such as `LAX`/`SAX`).

use crate::bus::Bus;

/// Status register flag bit positions.
mod flag {
    pub const CARRY: u8 = 1 << 0;
    pub const ZERO: u8 = 1 << 1;
    pub const IRQ_DISABLE: u8 = 1 << 2;
    pub const DECIMAL: u8 = 1 << 3;
    pub const BREAK: u8 = 1 << 4;
    pub const UNUSED: u8 = 1 << 5; // always reads as 1
    pub const OVERFLOW: u8 = 1 << 6;
    pub const NEGATIVE: u8 = 1 << 7;
}

/// Interrupt vector addresses.
const VEC_NMI: u16 = 0xFFFA;
const VEC_RESET: u16 = 0xFFFC;
const VEC_IRQ: u16 = 0xFFFE;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Imm,  // immediate
    Zp,   // zero page
    Zpx,  // zero page,X
    Zpy,  // zero page,Y
    Abs,  // absolute
    Abx,  // absolute,X
    Aby,  // absolute,Y
    Ind,  // indirect (JMP only)
    Izx,  // (indirect,X)
    Izy,  // (indirect),Y
    Rel,  // relative (branches)
}

pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    pub status: u8,

    /// Total cycles elapsed since reset.
    pub cycles: u64,

    /// Pending interrupt lines.
    pub irq_pending: bool,
    pub nmi_pending: bool,
    prev_nmi: bool, // for edge detection
}

impl Default for Cpu {
    fn default() -> Self {
        Cpu {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xFD,
            pc: 0,
            status: flag::IRQ_DISABLE | flag::UNUSED,
            cycles: 0,
            irq_pending: false,
            nmi_pending: false,
            prev_nmi: false,
        }
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    // --- flag helpers ---
    #[inline]
    fn set_flag(&mut self, mask: u8, on: bool) {
        if on {
            self.status |= mask;
        } else {
            self.status &= !mask;
        }
    }
    #[inline]
    fn get_flag(&self, mask: u8) -> bool {
        self.status & mask != 0
    }
    #[inline]
    fn set_zn(&mut self, v: u8) {
        self.set_flag(flag::ZERO, v == 0);
        self.set_flag(flag::NEGATIVE, v & 0x80 != 0);
    }

    // --- stack helpers ---
    #[inline]
    fn push(&mut self, bus: &mut impl Bus, v: u8) {
        bus.write(0x0100 | self.sp as u16, v);
        self.sp = self.sp.wrapping_sub(1);
    }
    #[inline]
    fn pop(&mut self, bus: &mut impl Bus) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        bus.read(0x0100 | self.sp as u16)
    }
    #[inline]
    fn push_word(&mut self, bus: &mut impl Bus, v: u16) {
        self.push(bus, (v >> 8) as u8);
        self.push(bus, (v & 0xFF) as u8);
    }
    #[inline]
    fn pop_word(&mut self, bus: &mut impl Bus) -> u16 {
        let lo = self.pop(bus) as u16;
        let hi = self.pop(bus) as u16;
        (hi << 8) | lo
    }

    // --- fetch helpers ---
    #[inline]
    fn fetch(&mut self, bus: &mut impl Bus) -> u8 {
        let v = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }
    #[inline]
    fn fetch_word(&mut self, bus: &mut impl Bus) -> u16 {
        let lo = self.fetch(bus) as u16;
        let hi = self.fetch(bus) as u16;
        (hi << 8) | lo
    }

    /// Reset: load PC from the reset vector and set the documented register state.
    pub fn reset(&mut self, bus: &mut impl Bus) {
        self.sp = 0xFD;
        self.status = flag::IRQ_DISABLE | flag::UNUSED;
        self.pc = bus.read_word(VEC_RESET);
        self.cycles = self.cycles.wrapping_add(7);
    }

    /// Signal the NMI line. The 6502 NMI is edge-triggered.
    pub fn nmi(&mut self) {
        self.nmi_pending = true;
    }
    /// Signal the IRQ line (level-triggered; honoured when I flag is clear).
    pub fn irq(&mut self) {
        self.irq_pending = true;
    }

    fn service_nmi(&mut self, bus: &mut impl Bus) {
        self.push_word(bus, self.pc);
        // B clear, bit5 set when pushed by an interrupt.
        let pushed = (self.status & !flag::BREAK) | flag::UNUSED;
        self.push(bus, pushed);
        self.set_flag(flag::IRQ_DISABLE, true);
        self.pc = bus.read_word(VEC_NMI);
        self.cycles = self.cycles.wrapping_add(7);
    }

    fn service_irq(&mut self, bus: &mut impl Bus) {
        self.push_word(bus, self.pc);
        let pushed = (self.status & !flag::BREAK) | flag::UNUSED;
        self.push(bus, pushed);
        self.set_flag(flag::IRQ_DISABLE, true);
        self.pc = bus.read_word(VEC_IRQ);
        self.cycles = self.cycles.wrapping_add(7);
    }

    /// Execute one instruction (after first servicing any pending interrupt).
    /// Returns the number of cycles the step consumed.
    pub fn step(&mut self, bus: &mut impl Bus) -> u64 {
        let start = self.cycles;

        // NMI edge detection.
        let nmi_edge = self.nmi_pending && !self.prev_nmi;
        self.prev_nmi = self.nmi_pending;
        if nmi_edge {
            self.nmi_pending = false;
            self.service_nmi(bus);
            return self.cycles - start;
        }
        if self.irq_pending && !self.get_flag(flag::IRQ_DISABLE) {
            self.irq_pending = false;
            self.service_irq(bus);
            return self.cycles - start;
        }

        let opcode = self.fetch(bus);
        self.execute(bus, opcode);
        self.cycles - start
    }

    // -----------------------------------------------------------------
    // Addressing-mode resolution
    // -----------------------------------------------------------------

    /// Resolve an operand address for the given mode.
    /// Returns `(address, page_crossed)`. `page_crossed` only matters for the
    /// indexed read modes that add a cycle when crossing a page boundary.
    fn resolve(&mut self, bus: &mut impl Bus, mode: Mode) -> (u16, bool) {
        match mode {
            Mode::Imm => {
                let a = self.pc;
                self.pc = self.pc.wrapping_add(1);
                (a, false)
            }
            Mode::Zp => (self.fetch(bus) as u16, false),
            Mode::Zpx => ((self.fetch(bus).wrapping_add(self.x)) as u16, false),
            Mode::Zpy => ((self.fetch(bus).wrapping_add(self.y)) as u16, false),
            Mode::Abs => (self.fetch_word(bus), false),
            Mode::Abx => {
                let base = self.fetch_word(bus);
                let a = base.wrapping_add(self.x as u16);
                (a, page_crossed(base, a))
            }
            Mode::Aby => {
                let base = self.fetch_word(bus);
                let a = base.wrapping_add(self.y as u16);
                (a, page_crossed(base, a))
            }
            Mode::Ind => {
                let ptr = self.fetch_word(bus);
                (bus.read_word_bug(ptr), false)
            }
            Mode::Izx => {
                let zp = self.fetch(bus).wrapping_add(self.x);
                let lo = bus.read(zp as u16) as u16;
                let hi = bus.read(zp.wrapping_add(1) as u16) as u16;
                ((hi << 8) | lo, false)
            }
            Mode::Izy => {
                let zp = self.fetch(bus);
                let lo = bus.read(zp as u16) as u16;
                let hi = bus.read(zp.wrapping_add(1) as u16) as u16;
                let base = (hi << 8) | lo;
                let a = base.wrapping_add(self.y as u16);
                (a, page_crossed(base, a))
            }
            Mode::Rel => {
                // Returns the branch target; caller decides whether to take it.
                let off = self.fetch(bus) as i8 as i16;
                let a = (self.pc as i16).wrapping_add(off) as u16;
                (a, page_crossed(self.pc, a))
            }
        }
    }

    // -----------------------------------------------------------------
    // ALU operations
    // -----------------------------------------------------------------

    fn adc(&mut self, value: u8) {
        let carry_in = self.get_flag(flag::CARRY) as u16;
        if self.get_flag(flag::DECIMAL) {
            // BCD addition (NMOS 6502 behaviour).
            let a = self.a as u16;
            let mut lo = (a & 0x0F) + (value as u16 & 0x0F) + carry_in;
            let mut hi = (a >> 4) + (value as u16 >> 4);
            if lo > 9 {
                lo += 6;
                hi += 1;
            }
            // V is computed from the binary-ish intermediate (NMOS quirk).
            let bin = a.wrapping_add(value as u16).wrapping_add(carry_in);
            self.set_flag(flag::ZERO, bin & 0xFF == 0);
            self.set_flag(
                flag::NEGATIVE,
                ((hi << 4) & 0x80) != 0,
            );
            self.set_flag(
                flag::OVERFLOW,
                (!(a ^ value as u16) & (a ^ (hi << 4)) & 0x80) != 0,
            );
            if hi > 9 {
                hi += 6;
            }
            self.set_flag(flag::CARRY, hi > 0x0F);
            self.a = (((hi << 4) | (lo & 0x0F)) & 0xFF) as u8;
        } else {
            let sum = self.a as u16 + value as u16 + carry_in;
            let result = sum as u8;
            self.set_flag(flag::CARRY, sum > 0xFF);
            self.set_flag(
                flag::OVERFLOW,
                (!(self.a ^ value) & (self.a ^ result) & 0x80) != 0,
            );
            self.a = result;
            self.set_zn(result);
        }
    }

    fn sbc(&mut self, value: u8) {
        if self.get_flag(flag::DECIMAL) {
            let carry_in = self.get_flag(flag::CARRY) as i16;
            let a = self.a as i16;
            let v = value as i16;
            let mut lo = (a & 0x0F) - (v & 0x0F) + carry_in - 1;
            let mut hi = (a >> 4) - (v >> 4);
            if lo < 0 {
                lo += 10;
                hi -= 1;
            }
            if hi < 0 {
                hi += 10;
            }
            // Flags come from the binary subtraction (NMOS quirk).
            let bin = (self.a as u16)
                .wrapping_sub(value as u16)
                .wrapping_sub(1 - carry_in as u16);
            self.set_flag(flag::CARRY, bin < 0x100);
            let result_bin = bin as u8;
            self.set_zn(result_bin);
            self.set_flag(
                flag::OVERFLOW,
                ((self.a ^ value) & (self.a ^ result_bin) & 0x80) != 0,
            );
            self.a = (((hi << 4) | (lo & 0x0F)) & 0xFF) as u8;
        } else {
            // Binary SBC is ADC of the one's complement.
            self.adc(!value);
        }
    }

    fn cmp_reg(&mut self, reg: u8, value: u8) {
        let r = reg.wrapping_sub(value);
        self.set_flag(flag::CARRY, reg >= value);
        self.set_zn(r);
    }

    fn asl(&mut self, value: u8) -> u8 {
        self.set_flag(flag::CARRY, value & 0x80 != 0);
        let r = value << 1;
        self.set_zn(r);
        r
    }
    fn lsr(&mut self, value: u8) -> u8 {
        self.set_flag(flag::CARRY, value & 0x01 != 0);
        let r = value >> 1;
        self.set_zn(r);
        r
    }
    fn rol(&mut self, value: u8) -> u8 {
        let carry_in = self.get_flag(flag::CARRY) as u8;
        self.set_flag(flag::CARRY, value & 0x80 != 0);
        let r = (value << 1) | carry_in;
        self.set_zn(r);
        r
    }
    fn ror(&mut self, value: u8) -> u8 {
        let carry_in = self.get_flag(flag::CARRY) as u8;
        self.set_flag(flag::CARRY, value & 0x01 != 0);
        let r = (value >> 1) | (carry_in << 7);
        self.set_zn(r);
        r
    }

    fn bit(&mut self, value: u8) {
        self.set_flag(flag::ZERO, self.a & value == 0);
        self.set_flag(flag::OVERFLOW, value & 0x40 != 0);
        self.set_flag(flag::NEGATIVE, value & 0x80 != 0);
    }

    fn branch(&mut self, take: bool, target: u16, page_crossed: bool) {
        if take {
            self.cycles += 1; // taken branch
            if page_crossed {
                self.cycles += 1;
            }
            self.pc = target;
        }
    }

    // -----------------------------------------------------------------
    // Decode + execute
    // -----------------------------------------------------------------

    fn execute(&mut self, bus: &mut impl Bus, opcode: u8) {
        // (mnemonic, mode, base_cycles, adds_page_cross_penalty)
        // Helper macros keep the table readable.
        macro_rules! addr {
            ($m:expr, $base:expr, $pen:expr) => {{
                let (a, crossed) = self.resolve(bus, $m);
                self.cycles += $base;
                if $pen && crossed {
                    self.cycles += 1;
                }
                a
            }};
        }

        match opcode {
            // ---- Load / store ----
            0xA9 => { let a = addr!(Mode::Imm,2,false); let v=bus.read(a); self.a=v; self.set_zn(v); }
            0xA5 => { let a = addr!(Mode::Zp,3,false);  let v=bus.read(a); self.a=v; self.set_zn(v); }
            0xB5 => { let a = addr!(Mode::Zpx,4,false); let v=bus.read(a); self.a=v; self.set_zn(v); }
            0xAD => { let a = addr!(Mode::Abs,4,false); let v=bus.read(a); self.a=v; self.set_zn(v); }
            0xBD => { let a = addr!(Mode::Abx,4,true);  let v=bus.read(a); self.a=v; self.set_zn(v); }
            0xB9 => { let a = addr!(Mode::Aby,4,true);  let v=bus.read(a); self.a=v; self.set_zn(v); }
            0xA1 => { let a = addr!(Mode::Izx,6,false); let v=bus.read(a); self.a=v; self.set_zn(v); }
            0xB1 => { let a = addr!(Mode::Izy,5,true);  let v=bus.read(a); self.a=v; self.set_zn(v); }

            0xA2 => { let a = addr!(Mode::Imm,2,false); let v=bus.read(a); self.x=v; self.set_zn(v); }
            0xA6 => { let a = addr!(Mode::Zp,3,false);  let v=bus.read(a); self.x=v; self.set_zn(v); }
            0xB6 => { let a = addr!(Mode::Zpy,4,false); let v=bus.read(a); self.x=v; self.set_zn(v); }
            0xAE => { let a = addr!(Mode::Abs,4,false); let v=bus.read(a); self.x=v; self.set_zn(v); }
            0xBE => { let a = addr!(Mode::Aby,4,true);  let v=bus.read(a); self.x=v; self.set_zn(v); }

            0xA0 => { let a = addr!(Mode::Imm,2,false); let v=bus.read(a); self.y=v; self.set_zn(v); }
            0xA4 => { let a = addr!(Mode::Zp,3,false);  let v=bus.read(a); self.y=v; self.set_zn(v); }
            0xB4 => { let a = addr!(Mode::Zpx,4,false); let v=bus.read(a); self.y=v; self.set_zn(v); }
            0xAC => { let a = addr!(Mode::Abs,4,false); let v=bus.read(a); self.y=v; self.set_zn(v); }
            0xBC => { let a = addr!(Mode::Abx,4,true);  let v=bus.read(a); self.y=v; self.set_zn(v); }

            0x85 => { let a = addr!(Mode::Zp,3,false);  bus.write(a,self.a); }
            0x95 => { let a = addr!(Mode::Zpx,4,false); bus.write(a,self.a); }
            0x8D => { let a = addr!(Mode::Abs,4,false); bus.write(a,self.a); }
            0x9D => { let a = addr!(Mode::Abx,5,false); bus.write(a,self.a); }
            0x99 => { let a = addr!(Mode::Aby,5,false); bus.write(a,self.a); }
            0x81 => { let a = addr!(Mode::Izx,6,false); bus.write(a,self.a); }
            0x91 => { let a = addr!(Mode::Izy,6,false); bus.write(a,self.a); }

            0x86 => { let a = addr!(Mode::Zp,3,false);  bus.write(a,self.x); }
            0x96 => { let a = addr!(Mode::Zpy,4,false); bus.write(a,self.x); }
            0x8E => { let a = addr!(Mode::Abs,4,false); bus.write(a,self.x); }

            0x84 => { let a = addr!(Mode::Zp,3,false);  bus.write(a,self.y); }
            0x94 => { let a = addr!(Mode::Zpx,4,false); bus.write(a,self.y); }
            0x8C => { let a = addr!(Mode::Abs,4,false); bus.write(a,self.y); }

            // ---- Register transfers ----
            0xAA => { self.cycles+=2; self.x=self.a; self.set_zn(self.x); }
            0xA8 => { self.cycles+=2; self.y=self.a; self.set_zn(self.y); }
            0x8A => { self.cycles+=2; self.a=self.x; self.set_zn(self.a); }
            0x98 => { self.cycles+=2; self.a=self.y; self.set_zn(self.a); }
            0xBA => { self.cycles+=2; self.x=self.sp; self.set_zn(self.x); }
            0x9A => { self.cycles+=2; self.sp=self.x; }

            // ---- Stack ----
            0x48 => { self.cycles+=3; let a=self.a; self.push(bus,a); }
            0x68 => { self.cycles+=4; let v=self.pop(bus); self.a=v; self.set_zn(v); }
            0x08 => { self.cycles+=3; let p=self.status | flag::BREAK | flag::UNUSED; self.push(bus,p); }
            0x28 => { self.cycles+=4; let v=self.pop(bus); self.status=(v & !flag::BREAK) | flag::UNUSED; }

            // ---- Logic ----
            0x29 => { let a=addr!(Mode::Imm,2,false); let v=bus.read(a); self.a&=v; self.set_zn(self.a); }
            0x25 => { let a=addr!(Mode::Zp,3,false);  let v=bus.read(a); self.a&=v; self.set_zn(self.a); }
            0x35 => { let a=addr!(Mode::Zpx,4,false); let v=bus.read(a); self.a&=v; self.set_zn(self.a); }
            0x2D => { let a=addr!(Mode::Abs,4,false); let v=bus.read(a); self.a&=v; self.set_zn(self.a); }
            0x3D => { let a=addr!(Mode::Abx,4,true);  let v=bus.read(a); self.a&=v; self.set_zn(self.a); }
            0x39 => { let a=addr!(Mode::Aby,4,true);  let v=bus.read(a); self.a&=v; self.set_zn(self.a); }
            0x21 => { let a=addr!(Mode::Izx,6,false); let v=bus.read(a); self.a&=v; self.set_zn(self.a); }
            0x31 => { let a=addr!(Mode::Izy,5,true);  let v=bus.read(a); self.a&=v; self.set_zn(self.a); }

            0x09 => { let a=addr!(Mode::Imm,2,false); let v=bus.read(a); self.a|=v; self.set_zn(self.a); }
            0x05 => { let a=addr!(Mode::Zp,3,false);  let v=bus.read(a); self.a|=v; self.set_zn(self.a); }
            0x15 => { let a=addr!(Mode::Zpx,4,false); let v=bus.read(a); self.a|=v; self.set_zn(self.a); }
            0x0D => { let a=addr!(Mode::Abs,4,false); let v=bus.read(a); self.a|=v; self.set_zn(self.a); }
            0x1D => { let a=addr!(Mode::Abx,4,true);  let v=bus.read(a); self.a|=v; self.set_zn(self.a); }
            0x19 => { let a=addr!(Mode::Aby,4,true);  let v=bus.read(a); self.a|=v; self.set_zn(self.a); }
            0x01 => { let a=addr!(Mode::Izx,6,false); let v=bus.read(a); self.a|=v; self.set_zn(self.a); }
            0x11 => { let a=addr!(Mode::Izy,5,true);  let v=bus.read(a); self.a|=v; self.set_zn(self.a); }

            0x49 => { let a=addr!(Mode::Imm,2,false); let v=bus.read(a); self.a^=v; self.set_zn(self.a); }
            0x45 => { let a=addr!(Mode::Zp,3,false);  let v=bus.read(a); self.a^=v; self.set_zn(self.a); }
            0x55 => { let a=addr!(Mode::Zpx,4,false); let v=bus.read(a); self.a^=v; self.set_zn(self.a); }
            0x4D => { let a=addr!(Mode::Abs,4,false); let v=bus.read(a); self.a^=v; self.set_zn(self.a); }
            0x5D => { let a=addr!(Mode::Abx,4,true);  let v=bus.read(a); self.a^=v; self.set_zn(self.a); }
            0x59 => { let a=addr!(Mode::Aby,4,true);  let v=bus.read(a); self.a^=v; self.set_zn(self.a); }
            0x41 => { let a=addr!(Mode::Izx,6,false); let v=bus.read(a); self.a^=v; self.set_zn(self.a); }
            0x51 => { let a=addr!(Mode::Izy,5,true);  let v=bus.read(a); self.a^=v; self.set_zn(self.a); }

            0x24 => { let a=addr!(Mode::Zp,3,false);  let v=bus.read(a); self.bit(v); }
            0x2C => { let a=addr!(Mode::Abs,4,false); let v=bus.read(a); self.bit(v); }

            // ---- Arithmetic ----
            0x69 => { let a=addr!(Mode::Imm,2,false); let v=bus.read(a); self.adc(v); }
            0x65 => { let a=addr!(Mode::Zp,3,false);  let v=bus.read(a); self.adc(v); }
            0x75 => { let a=addr!(Mode::Zpx,4,false); let v=bus.read(a); self.adc(v); }
            0x6D => { let a=addr!(Mode::Abs,4,false); let v=bus.read(a); self.adc(v); }
            0x7D => { let a=addr!(Mode::Abx,4,true);  let v=bus.read(a); self.adc(v); }
            0x79 => { let a=addr!(Mode::Aby,4,true);  let v=bus.read(a); self.adc(v); }
            0x61 => { let a=addr!(Mode::Izx,6,false); let v=bus.read(a); self.adc(v); }
            0x71 => { let a=addr!(Mode::Izy,5,true);  let v=bus.read(a); self.adc(v); }

            0xE9 => { let a=addr!(Mode::Imm,2,false); let v=bus.read(a); self.sbc(v); }
            0xE5 => { let a=addr!(Mode::Zp,3,false);  let v=bus.read(a); self.sbc(v); }
            0xF5 => { let a=addr!(Mode::Zpx,4,false); let v=bus.read(a); self.sbc(v); }
            0xED => { let a=addr!(Mode::Abs,4,false); let v=bus.read(a); self.sbc(v); }
            0xFD => { let a=addr!(Mode::Abx,4,true);  let v=bus.read(a); self.sbc(v); }
            0xF9 => { let a=addr!(Mode::Aby,4,true);  let v=bus.read(a); self.sbc(v); }
            0xE1 => { let a=addr!(Mode::Izx,6,false); let v=bus.read(a); self.sbc(v); }
            0xF1 => { let a=addr!(Mode::Izy,5,true);  let v=bus.read(a); self.sbc(v); }

            0xC9 => { let a=addr!(Mode::Imm,2,false); let v=bus.read(a); self.cmp_reg(self.a,v); }
            0xC5 => { let a=addr!(Mode::Zp,3,false);  let v=bus.read(a); self.cmp_reg(self.a,v); }
            0xD5 => { let a=addr!(Mode::Zpx,4,false); let v=bus.read(a); self.cmp_reg(self.a,v); }
            0xCD => { let a=addr!(Mode::Abs,4,false); let v=bus.read(a); self.cmp_reg(self.a,v); }
            0xDD => { let a=addr!(Mode::Abx,4,true);  let v=bus.read(a); self.cmp_reg(self.a,v); }
            0xD9 => { let a=addr!(Mode::Aby,4,true);  let v=bus.read(a); self.cmp_reg(self.a,v); }
            0xC1 => { let a=addr!(Mode::Izx,6,false); let v=bus.read(a); self.cmp_reg(self.a,v); }
            0xD1 => { let a=addr!(Mode::Izy,5,true);  let v=bus.read(a); self.cmp_reg(self.a,v); }

            0xE0 => { let a=addr!(Mode::Imm,2,false); let v=bus.read(a); self.cmp_reg(self.x,v); }
            0xE4 => { let a=addr!(Mode::Zp,3,false);  let v=bus.read(a); self.cmp_reg(self.x,v); }
            0xEC => { let a=addr!(Mode::Abs,4,false); let v=bus.read(a); self.cmp_reg(self.x,v); }

            0xC0 => { let a=addr!(Mode::Imm,2,false); let v=bus.read(a); self.cmp_reg(self.y,v); }
            0xC4 => { let a=addr!(Mode::Zp,3,false);  let v=bus.read(a); self.cmp_reg(self.y,v); }
            0xCC => { let a=addr!(Mode::Abs,4,false); let v=bus.read(a); self.cmp_reg(self.y,v); }

            // ---- Inc / dec ----
            0xE6 => { let a=addr!(Mode::Zp,5,false);  let v=bus.read(a).wrapping_add(1); bus.write(a,v); self.set_zn(v); }
            0xF6 => { let a=addr!(Mode::Zpx,6,false); let v=bus.read(a).wrapping_add(1); bus.write(a,v); self.set_zn(v); }
            0xEE => { let a=addr!(Mode::Abs,6,false); let v=bus.read(a).wrapping_add(1); bus.write(a,v); self.set_zn(v); }
            0xFE => { let a=addr!(Mode::Abx,7,false); let v=bus.read(a).wrapping_add(1); bus.write(a,v); self.set_zn(v); }

            0xC6 => { let a=addr!(Mode::Zp,5,false);  let v=bus.read(a).wrapping_sub(1); bus.write(a,v); self.set_zn(v); }
            0xD6 => { let a=addr!(Mode::Zpx,6,false); let v=bus.read(a).wrapping_sub(1); bus.write(a,v); self.set_zn(v); }
            0xCE => { let a=addr!(Mode::Abs,6,false); let v=bus.read(a).wrapping_sub(1); bus.write(a,v); self.set_zn(v); }
            0xDE => { let a=addr!(Mode::Abx,7,false); let v=bus.read(a).wrapping_sub(1); bus.write(a,v); self.set_zn(v); }

            0xE8 => { self.cycles+=2; self.x=self.x.wrapping_add(1); self.set_zn(self.x); }
            0xCA => { self.cycles+=2; self.x=self.x.wrapping_sub(1); self.set_zn(self.x); }
            0xC8 => { self.cycles+=2; self.y=self.y.wrapping_add(1); self.set_zn(self.y); }
            0x88 => { self.cycles+=2; self.y=self.y.wrapping_sub(1); self.set_zn(self.y); }

            // ---- Shifts / rotates ----
            0x0A => { self.cycles+=2; let v=self.asl(self.a); self.a=v; }
            0x06 => { let a=addr!(Mode::Zp,5,false);  let v=bus.read(a); let r=self.asl(v); bus.write(a,r); }
            0x16 => { let a=addr!(Mode::Zpx,6,false); let v=bus.read(a); let r=self.asl(v); bus.write(a,r); }
            0x0E => { let a=addr!(Mode::Abs,6,false); let v=bus.read(a); let r=self.asl(v); bus.write(a,r); }
            0x1E => { let a=addr!(Mode::Abx,7,false); let v=bus.read(a); let r=self.asl(v); bus.write(a,r); }

            0x4A => { self.cycles+=2; let v=self.lsr(self.a); self.a=v; }
            0x46 => { let a=addr!(Mode::Zp,5,false);  let v=bus.read(a); let r=self.lsr(v); bus.write(a,r); }
            0x56 => { let a=addr!(Mode::Zpx,6,false); let v=bus.read(a); let r=self.lsr(v); bus.write(a,r); }
            0x4E => { let a=addr!(Mode::Abs,6,false); let v=bus.read(a); let r=self.lsr(v); bus.write(a,r); }
            0x5E => { let a=addr!(Mode::Abx,7,false); let v=bus.read(a); let r=self.lsr(v); bus.write(a,r); }

            0x2A => { self.cycles+=2; let v=self.rol(self.a); self.a=v; }
            0x26 => { let a=addr!(Mode::Zp,5,false);  let v=bus.read(a); let r=self.rol(v); bus.write(a,r); }
            0x36 => { let a=addr!(Mode::Zpx,6,false); let v=bus.read(a); let r=self.rol(v); bus.write(a,r); }
            0x2E => { let a=addr!(Mode::Abs,6,false); let v=bus.read(a); let r=self.rol(v); bus.write(a,r); }
            0x3E => { let a=addr!(Mode::Abx,7,false); let v=bus.read(a); let r=self.rol(v); bus.write(a,r); }

            0x6A => { self.cycles+=2; let v=self.ror(self.a); self.a=v; }
            0x66 => { let a=addr!(Mode::Zp,5,false);  let v=bus.read(a); let r=self.ror(v); bus.write(a,r); }
            0x76 => { let a=addr!(Mode::Zpx,6,false); let v=bus.read(a); let r=self.ror(v); bus.write(a,r); }
            0x6E => { let a=addr!(Mode::Abs,6,false); let v=bus.read(a); let r=self.ror(v); bus.write(a,r); }
            0x7E => { let a=addr!(Mode::Abx,7,false); let v=bus.read(a); let r=self.ror(v); bus.write(a,r); }

            // ---- Jumps / calls ----
            0x4C => { let a=addr!(Mode::Abs,3,false); self.pc=a; }
            0x6C => { let a=addr!(Mode::Ind,5,false); self.pc=a; }
            0x20 => {
                // JSR: push (return address - 1), then jump.
                let target = self.fetch_word(bus);
                self.cycles += 6;
                let ret = self.pc.wrapping_sub(1);
                self.push_word(bus, ret);
                self.pc = target;
            }
            0x60 => { self.cycles+=6; let v=self.pop_word(bus); self.pc=v.wrapping_add(1); }
            0x40 => {
                // RTI
                self.cycles += 6;
                let p = self.pop(bus);
                self.status = (p & !flag::BREAK) | flag::UNUSED;
                self.pc = self.pop_word(bus);
            }
            0x00 => {
                // BRK: software interrupt. PC already advanced past opcode; skip
                // the padding byte, push PC+1, push status with B set.
                self.cycles += 7;
                let ret = self.pc.wrapping_add(1);
                self.push_word(bus, ret);
                self.push(bus, self.status | flag::BREAK | flag::UNUSED);
                self.set_flag(flag::IRQ_DISABLE, true);
                self.pc = bus.read_word(VEC_IRQ);
            }

            // ---- Branches ----
            0x10 => { let (t,c)=self.resolve(bus,Mode::Rel); self.cycles+=2; self.branch(!self.get_flag(flag::NEGATIVE),t,c); }
            0x30 => { let (t,c)=self.resolve(bus,Mode::Rel); self.cycles+=2; self.branch(self.get_flag(flag::NEGATIVE),t,c); }
            0x50 => { let (t,c)=self.resolve(bus,Mode::Rel); self.cycles+=2; self.branch(!self.get_flag(flag::OVERFLOW),t,c); }
            0x70 => { let (t,c)=self.resolve(bus,Mode::Rel); self.cycles+=2; self.branch(self.get_flag(flag::OVERFLOW),t,c); }
            0x90 => { let (t,c)=self.resolve(bus,Mode::Rel); self.cycles+=2; self.branch(!self.get_flag(flag::CARRY),t,c); }
            0xB0 => { let (t,c)=self.resolve(bus,Mode::Rel); self.cycles+=2; self.branch(self.get_flag(flag::CARRY),t,c); }
            0xD0 => { let (t,c)=self.resolve(bus,Mode::Rel); self.cycles+=2; self.branch(!self.get_flag(flag::ZERO),t,c); }
            0xF0 => { let (t,c)=self.resolve(bus,Mode::Rel); self.cycles+=2; self.branch(self.get_flag(flag::ZERO),t,c); }

            // ---- Flag ops ----
            0x18 => { self.cycles+=2; self.set_flag(flag::CARRY,false); }
            0x38 => { self.cycles+=2; self.set_flag(flag::CARRY,true); }
            0x58 => { self.cycles+=2; self.set_flag(flag::IRQ_DISABLE,false); }
            0x78 => { self.cycles+=2; self.set_flag(flag::IRQ_DISABLE,true); }
            0xB8 => { self.cycles+=2; self.set_flag(flag::OVERFLOW,false); }
            0xD8 => { self.cycles+=2; self.set_flag(flag::DECIMAL,false); }
            0xF8 => { self.cycles+=2; self.set_flag(flag::DECIMAL,true); }

            // ---- NOP (official) ----
            0xEA => { self.cycles+=2; }

            // ---- Undocumented: treat as NOPs of the right length/cycles ----
            // Implied 2-cycle NOPs.
            0x1A | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA => { self.cycles+=2; }
            // Immediate 2-byte NOPs.
            0x80 | 0x82 | 0x89 | 0xC2 | 0xE2 => { let _=addr!(Mode::Imm,2,false); }
            // Zero-page NOPs.
            0x04 | 0x44 | 0x64 => { let _=addr!(Mode::Zp,3,false); }
            // Zero-page,X NOPs.
            0x14 | 0x34 | 0x54 | 0x74 | 0xD4 | 0xF4 => { let _=addr!(Mode::Zpx,4,false); }
            // Absolute NOP.
            0x0C => { let _=addr!(Mode::Abs,4,false); }
            // Absolute,X NOPs (page-cross penalty).
            0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => { let _=addr!(Mode::Abx,4,true); }

            // Anything else: unimplemented illegal opcode. Consume 2 cycles and
            // continue rather than panicking, so a stray byte can't kill the run.
            other => {
                let _ = other;
                self.cycles += 2;
            }
        }
    }
}

/// True if `a` and `b` lie in different 256-byte pages.
#[inline]
fn page_crossed(a: u16, b: u16) -> bool {
    (a & 0xFF00) != (b & 0xFF00)
}

// ===========================================================================
// Unit tests
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::FlatRam;

    /// Build a CPU + flat RAM with `prog` loaded at $0200 and PC pointed there.
    fn setup(prog: &[u8]) -> (Cpu, FlatRam) {
        let mut ram = FlatRam::new();
        ram.load(0x0200, prog);
        let mut cpu = Cpu::new();
        cpu.pc = 0x0200;
        (cpu, ram)
    }

    #[test]
    fn lda_immediate_sets_zn() {
        let (mut cpu, mut ram) = setup(&[0xA9, 0x00]); // LDA #$00
        cpu.step(&mut ram);
        assert_eq!(cpu.a, 0x00);
        assert!(cpu.get_flag(flag::ZERO));
        assert!(!cpu.get_flag(flag::NEGATIVE));

        let (mut cpu, mut ram) = setup(&[0xA9, 0x80]); // LDA #$80
        cpu.step(&mut ram);
        assert_eq!(cpu.a, 0x80);
        assert!(!cpu.get_flag(flag::ZERO));
        assert!(cpu.get_flag(flag::NEGATIVE));
    }

    #[test]
    fn adc_basic_and_carry_overflow() {
        // LDA #$50 ; ADC #$50  -> $A0, V set, N set, C clear
        let (mut cpu, mut ram) = setup(&[0xA9, 0x50, 0x69, 0x50]);
        cpu.step(&mut ram);
        cpu.step(&mut ram);
        assert_eq!(cpu.a, 0xA0);
        assert!(cpu.get_flag(flag::OVERFLOW));
        assert!(cpu.get_flag(flag::NEGATIVE));
        assert!(!cpu.get_flag(flag::CARRY));
    }

    #[test]
    fn adc_carry_out() {
        // LDA #$FF ; ADC #$01 -> $00, C set, Z set
        let (mut cpu, mut ram) = setup(&[0xA9, 0xFF, 0x69, 0x01]);
        cpu.step(&mut ram);
        cpu.step(&mut ram);
        assert_eq!(cpu.a, 0x00);
        assert!(cpu.get_flag(flag::CARRY));
        assert!(cpu.get_flag(flag::ZERO));
    }

    #[test]
    fn sbc_basic() {
        // SEC ; LDA #$50 ; SBC #$30 -> $20, C set (no borrow)
        let (mut cpu, mut ram) = setup(&[0x38, 0xA9, 0x50, 0xE9, 0x30]);
        cpu.step(&mut ram); // SEC
        cpu.step(&mut ram); // LDA
        cpu.step(&mut ram); // SBC
        assert_eq!(cpu.a, 0x20);
        assert!(cpu.get_flag(flag::CARRY));
    }

    #[test]
    fn adc_decimal_mode() {
        // SED ; SEC=clear ; LDA #$09 ; ADC #$01 -> $10 in BCD
        let (mut cpu, mut ram) = setup(&[0xF8, 0x18, 0xA9, 0x09, 0x69, 0x01]);
        cpu.step(&mut ram); // SED
        cpu.step(&mut ram); // CLC
        cpu.step(&mut ram); // LDA #$09
        cpu.step(&mut ram); // ADC #$01
        assert_eq!(cpu.a, 0x10);
        assert!(!cpu.get_flag(flag::CARRY));
    }

    #[test]
    fn jsr_rts_roundtrip() {
        // $0200: JSR $0205 ; (then) at $0205: RTS
        let mut ram = FlatRam::new();
        ram.load(0x0200, &[0x20, 0x05, 0x02]); // JSR $0205
        ram.load(0x0205, &[0x60]); // RTS
        let mut cpu = Cpu::new();
        cpu.pc = 0x0200;
        cpu.sp = 0xFD;
        cpu.step(&mut ram); // JSR
        assert_eq!(cpu.pc, 0x0205);
        cpu.step(&mut ram); // RTS
        assert_eq!(cpu.pc, 0x0203);
    }

    #[test]
    fn branch_taken_adds_cycles_and_moves_pc() {
        // LDA #$00 (Z=1) ; BEQ +2
        let (mut cpu, mut ram) = setup(&[0xA9, 0x00, 0xF0, 0x02]);
        cpu.step(&mut ram); // LDA
        let before = cpu.cycles;
        cpu.step(&mut ram); // BEQ taken
        // base 2 + 1 taken = 3 cycles, no page cross
        assert_eq!(cpu.cycles - before, 3);
        // PC = 0x0204 (after operand) + 2 = 0x0206
        assert_eq!(cpu.pc, 0x0206);
    }

    #[test]
    fn indexed_indirect_and_indirect_indexed() {
        // (indirect),Y : set up pointer at $10 -> $0300, Y=4 => read $0304
        let mut ram = FlatRam::new();
        ram.mem[0x10] = 0x00;
        ram.mem[0x11] = 0x03;
        ram.mem[0x0304] = 0x42;
        ram.load(0x0200, &[0xA0, 0x04, 0xB1, 0x10]); // LDY #4 ; LDA ($10),Y
        let mut cpu = Cpu::new();
        cpu.pc = 0x0200;
        cpu.step(&mut ram); // LDY
        cpu.step(&mut ram); // LDA
        assert_eq!(cpu.a, 0x42);
    }

    #[test]
    fn php_plp_preserves_break_and_unused() {
        // PHP pushes with B and bit5 set; PLP restores ignoring B.
        let (mut cpu, mut ram) = setup(&[0x08, 0x28]);
        cpu.sp = 0xFD;
        cpu.set_flag(flag::CARRY, true);
        cpu.step(&mut ram); // PHP
        cpu.set_flag(flag::CARRY, false);
        cpu.step(&mut ram); // PLP restores carry
        assert!(cpu.get_flag(flag::CARRY));
        assert!(cpu.get_flag(flag::UNUSED));
    }

    #[test]
    fn inc_dec_memory() {
        // INC $20 from 0xFF -> 0x00, Z set
        let mut ram = FlatRam::new();
        ram.mem[0x20] = 0xFF;
        ram.load(0x0200, &[0xE6, 0x20]);
        let mut cpu = Cpu::new();
        cpu.pc = 0x0200;
        cpu.step(&mut ram);
        assert_eq!(ram.mem[0x20], 0x00);
        assert!(cpu.get_flag(flag::ZERO));
    }
}

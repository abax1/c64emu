//! Memory bus and the C64 address-space banking.
//!
//! The CPU talks to memory only through the [`Bus`] trait, so it can be tested
//! against a flat 64 KB RAM ([`FlatRam`]) independent of any C64 hardware. The
//! real machine uses [`C64Bus`], which implements the PLA banking driven by the
//! 6510 on-chip processor port at addresses `$0000`/`$0001`.

use crate::cia::Cia;
use crate::sid::Sid;
use crate::vic::Vic;

/// Anything the CPU can read bytes from and write bytes to.
///
/// Reads are `&mut self` because real device reads can have side effects
/// (clearing interrupt flags, advancing latches, etc.).
pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);

    /// Little-endian 16-bit read. Default impl composes two byte reads.
    fn read_word(&mut self, addr: u16) -> u16 {
        let lo = self.read(addr) as u16;
        let hi = self.read(addr.wrapping_add(1)) as u16;
        (hi << 8) | lo
    }

    /// 16-bit read with the 6502 page-wrap bug used by indirect `JMP`:
    /// the high byte is fetched from the same page as the low byte.
    fn read_word_bug(&mut self, addr: u16) -> u16 {
        let lo = self.read(addr) as u16;
        let hi_addr = (addr & 0xFF00) | ((addr.wrapping_add(1)) & 0x00FF);
        let hi = self.read(hi_addr) as u16;
        (hi << 8) | lo
    }
}

/// A flat 64 KB RAM. Used for CPU unit tests and the functional-test harness.
pub struct FlatRam {
    pub mem: Box<[u8; 0x10000]>,
}

impl Default for FlatRam {
    fn default() -> Self {
        FlatRam { mem: Box::new([0u8; 0x10000]) }
    }
}

impl FlatRam {
    pub fn new() -> Self {
        Self::default()
    }

    /// Copy `data` into memory starting at `addr` (wrapping at 64 KB).
    pub fn load(&mut self, addr: u16, data: &[u8]) {
        for (i, b) in data.iter().enumerate() {
            self.mem[(addr as usize + i) & 0xFFFF] = *b;
        }
    }
}

impl Bus for FlatRam {
    fn read(&mut self, addr: u16) -> u8 {
        self.mem[addr as usize]
    }
    fn write(&mut self, addr: u16, value: u8) {
        self.mem[addr as usize] = value;
    }
}

// ---------------------------------------------------------------------------
// C64 system bus
// ---------------------------------------------------------------------------

/// The I/O devices that live in the `$D000–$DFFF` window.
///
/// CIA #1 and CIA #2 are real; VIC-II and SID are still open-RAM scratch until
/// those chips are implemented.
pub struct IoDevices {
    /// Colour RAM is 1 KB of nybbles at `$D800–$DBFF`.
    pub color_ram: [u8; 0x0400],
    /// Scratch backing for the SID ($D400) and I/O expansion ($DE00) regions
    /// until those devices exist. Indexed by `addr - $D000`.
    pub io_scratch: [u8; 0x1000],
    pub vic: Vic,
    pub sid: Sid,
    pub cia1: Cia,
    pub cia2: Cia,
}

impl Default for IoDevices {
    fn default() -> Self {
        IoDevices {
            color_ram: [0u8; 0x0400],
            io_scratch: [0u8; 0x1000],
            vic: Vic::new(),
            sid: Sid::new(),
            cia1: Cia::cia1(),
            cia2: Cia::new(),
        }
    }
}

impl IoDevices {
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0xD000..=0xD3FF => self.vic.read(addr),
            0xD400..=0xD7FF => self.sid.read((addr & 0x1F) as u8),
            0xD800..=0xDBFF => self.color_ram[(addr - 0xD800) as usize] | 0xF0,
            0xDC00..=0xDCFF => self.cia1.read((addr & 0x0F) as u8),
            0xDD00..=0xDDFF => self.cia2.read((addr & 0x0F) as u8),
            _ => self.io_scratch[(addr - 0xD000) as usize],
        }
    }
    fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xD000..=0xD3FF => self.vic.write(addr, value),
            0xD400..=0xD7FF => self.sid.write((addr & 0x1F) as u8, value),
            0xD800..=0xDBFF => self.color_ram[(addr - 0xD800) as usize] = value & 0x0F,
            0xDC00..=0xDCFF => self.cia1.write((addr & 0x0F) as u8, value),
            0xDD00..=0xDDFF => self.cia2.write((addr & 0x0F) as u8, value),
            _ => self.io_scratch[(addr - 0xD000) as usize] = value,
        }
    }
}

/// The full C64 memory system: 64 KB RAM overlaid with ROMs and I/O according
/// to the processor-port banking bits.
pub struct C64Bus {
    pub ram: Box<[u8; 0x10000]>,
    pub basic_rom: [u8; 0x2000],   // $A000-$BFFF
    pub kernal_rom: [u8; 0x2000],  // $E000-$FFFF
    pub char_rom: [u8; 0x1000],    // $D000-$DFFF when CHAREN selects it
    pub io: IoDevices,

    /// 6510 data-direction register at $0000.
    port_dir: u8,
    /// 6510 output register at $0001 (only driven bits matter).
    port_data: u8,
}

impl Default for C64Bus {
    fn default() -> Self {
        C64Bus {
            ram: Box::new([0u8; 0x10000]),
            basic_rom: [0u8; 0x2000],
            kernal_rom: [0u8; 0x2000],
            char_rom: [0u8; 0x1000],
            io: IoDevices::default(),
            // At reset the port reads as if LORAM/HIRAM/CHAREN are all high:
            // BASIC + KERNAL + I/O are visible. DDR defaults make bits 0-2 outputs.
            port_dir: 0x2F,
            port_data: 0x37,
        }
    }
}

impl C64Bus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance time-dependent devices (VIC raster + CIAs) by `cycles` phi2
    /// clocks and return the CPU interrupt lines as `(irq, nmi)`. The IRQ line
    /// is the OR of CIA #1 and the VIC raster IRQ; the NMI line is CIA #2.
    pub fn tick(&mut self, cycles: u64) -> (bool, bool) {
        let vic_irq = self.io.vic.tick(cycles);
        let cia1_irq = self.io.cia1.tick(cycles);
        let nmi = self.io.cia2.tick(cycles);
        self.io.sid.clock(cycles);
        (vic_irq || cia1_irq, nmi)
    }

    /// Effective value of the three banking lines (LORAM, HIRAM, CHAREN).
    /// A line reads high if it is an input (DDR bit 0) or its data bit is 1.
    fn banking_bits(&self) -> u8 {
        // For each of bits 0..=2: if the line is an output, use the data bit;
        // if it's an input, the C64 pull-ups make it read high.
        let effective = (self.port_data & self.port_dir) | (!self.port_dir);
        effective & 0x07
    }

    fn loram(&self) -> bool { self.banking_bits() & 0x01 != 0 } // BASIC enable
    fn hiram(&self) -> bool { self.banking_bits() & 0x02 != 0 } // KERNAL enable
    fn charen(&self) -> bool { self.banking_bits() & 0x04 != 0 } // I/O vs CHARGEN

    /// True when BASIC ROM is mapped at $A000-$BFFF (needs LORAM and HIRAM).
    fn basic_mapped(&self) -> bool {
        self.loram() && self.hiram()
    }

    /// True when KERNAL ROM is mapped at $E000-$FFFF (needs HIRAM).
    fn kernal_mapped(&self) -> bool {
        self.hiram()
    }

    /// What occupies $D000-$DFFF: I/O, character ROM, or RAM.
    fn d000_region(&self) -> D000 {
        // I/O appears when CHAREN is high AND at least one of LORAM/HIRAM is high.
        if (self.loram() || self.hiram()) && self.charen() {
            D000::Io
        } else if (self.loram() || self.hiram()) && !self.charen() {
            D000::CharRom
        } else {
            D000::Ram
        }
    }
}

enum D000 {
    Io,
    CharRom,
    Ram,
}

impl Bus for C64Bus {
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000 => self.port_dir,
            0x0001 => {
                // Reading the port returns driven output bits plus input lines
                // (which float high on a stock C64).
                (self.port_data & self.port_dir) | (!self.port_dir & 0x07) | (self.port_data & !self.port_dir & 0xF8)
            }
            0xA000..=0xBFFF if self.basic_mapped() => self.basic_rom[(addr - 0xA000) as usize],
            0xD000..=0xDFFF => match self.d000_region() {
                D000::Io => self.io.read(addr),
                D000::CharRom => self.char_rom[(addr - 0xD000) as usize],
                D000::Ram => self.ram[addr as usize],
            },
            0xE000..=0xFFFF if self.kernal_mapped() => self.kernal_rom[(addr - 0xE000) as usize],
            _ => self.ram[addr as usize],
        }
    }

    fn write(&mut self, addr: u16, value: u8) {
        // Writes always land in RAM underneath ROM (the C64 has no write-through
        // to ROM), except I/O writes go to devices.
        match addr {
            0x0000 => self.port_dir = value,
            0x0001 => self.port_data = value,
            0xD000..=0xDFFF => match self.d000_region() {
                D000::Io => self.io.write(addr, value),
                // CharROM/RAM banked here still writes to underlying RAM.
                _ => self.ram[addr as usize] = value,
            },
            _ => self.ram[addr as usize] = value,
        }
    }
}

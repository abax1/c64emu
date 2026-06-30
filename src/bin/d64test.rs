//! Headless D64 load-and-run check.
//!
//! Boots the C64, loads the first PRG from a .d64 image into RAM (like
//! `LOAD"*",8,1`), jumps to its BASIC `SYS` entry (like `RUN`), runs for a
//! while, and reports whether the program took over the machine. It does NOT
//! render anything — it only confirms the disk path works and the game's code
//! is executing.
//!
//! Usage: `cargo run --bin d64test -- "disks/Silk Worm.d64"`

use c64emu::bus::Bus;
use c64emu::d64::D64;
use c64emu::system::System;
use std::path::Path;

const DEFAULT_DISK: &str = "disks/Silk Worm.d64";

/// Parse the `SYS <addr>` target out of a BASIC autostart stub.
fn parse_sys_target(payload: &[u8], load_addr: u16) -> u16 {
    // Tokenised BASIC: [link lo,hi][line lo,hi] then tokens; $9E = SYS.
    if let Some(p) = payload.iter().position(|&b| b == 0x9E) {
        let mut n: u32 = 0;
        let mut saw = false;
        for &b in &payload[p + 1..] {
            match b {
                b' ' if !saw => continue,
                b'0'..=b'9' => {
                    n = n * 10 + (b - b'0') as u32;
                    saw = true;
                }
                _ => break,
            }
        }
        if saw {
            return n as u16;
        }
    }
    load_addr.wrapping_add(0x000A) // fallback: just past a typical stub
}

fn boot_to_ready(sys: &mut System) {
    // Boot takes ~2.2M cycles; give it a generous margin and watch screen RAM.
    for _ in 0..250 {
        sys.run_frame();
        // "READY." screen codes: R E A D Y .
        let scr = &sys.bus.ram[0x0400..0x0400 + 1000];
        if scr.windows(6).any(|w| w == [18, 5, 1, 4, 25, 46]) {
            return;
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_DISK.to_string());

    let mut sys = System::new();
    if let Err(e) = sys.load_roms(Path::new("roms")) {
        eprintln!("ROMs not loaded ({e:?}); cannot boot. Put basic/kernal/chargen in roms/.");
        std::process::exit(1);
    }
    sys.reset();
    boot_to_ready(&mut sys);

    // Load the disk image and pick the first PRG.
    let disk = match D64::load(Path::new(&path)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to read {path}: {e}");
            std::process::exit(1);
        }
    };
    println!("Disk: \"{}\"  ({})", disk.disk_name(), path);
    let entry = match disk.find("*") {
        Some(e) => e,
        None => {
            eprintln!("no PRG file found on disk");
            std::process::exit(1);
        }
    };
    let bytes = disk.read_file(&entry);
    if bytes.len() < 3 {
        eprintln!("file too short");
        std::process::exit(1);
    }
    let load_addr = bytes[0] as u16 | ((bytes[1] as u16) << 8);
    let payload = &bytes[2..];
    let end_addr = load_addr as usize + payload.len();
    println!(
        "File: \"{}\"  {} blocks, {} bytes, load ${:04X}-${:04X}",
        entry.name_ascii(),
        entry.blocks,
        payload.len(),
        load_addr,
        end_addr - 1
    );

    // Inject into RAM.
    for (i, &b) in payload.iter().enumerate() {
        sys.bus.ram[(load_addr as usize + i) & 0xFFFF] = b;
    }

    // Snapshot the boot screen + key indicators before running the game.
    let boot_screen: Vec<u8> = sys.bus.ram[0x0400..0x0400 + 1000].to_vec();
    let before = Indicators::capture(&mut sys);

    // Jump to the SYS entry (what RUN would do).
    let sys_target = parse_sys_target(payload, load_addr);
    println!("Entry: SYS {sys_target} (${sys_target:04X})\n");
    sys.cpu.pc = sys_target;

    // Run the game for a while.
    let start_cycles = sys.cpu.cycles;
    for _ in 0..300 {
        sys.run_frame();
    }
    let after = Indicators::capture(&mut sys);

    // Compare.
    let screen_changed = sys.bus.ram[0x0400..0x0400 + 1000]
        .iter()
        .zip(boot_screen.iter())
        .filter(|(a, b)| a != b)
        .count();

    println!("Ran {} cycles after entry.", sys.cpu.cycles - start_cycles);
    println!("Final PC = ${:04X}\n", sys.cpu.pc);
    println!("Indicator           before   after");
    before.print_diff(&after);
    println!("\nScreen RAM cells changed since boot: {screen_changed}/1000");

    // Short trace to show it's running varied code, not stuck on one address.
    println!("\nPC trace (8 steps):");
    let mut last = sys.cpu.pc;
    let mut distinct = 0;
    for _ in 0..8 {
        let pc = sys.cpu.pc;
        if pc != last {
            distinct += 1;
        }
        last = pc;
        let op = sys.bus.ram[pc as usize];
        println!("  ${pc:04X}: {op:02X}");
        let cyc = sys.cpu.step(&mut sys.bus);
        let (irq, nmi) = sys.bus.tick(cyc);
        sys.cpu.irq_pending = irq;
        if nmi {
            sys.cpu.nmi();
        }
    }

    // Verdict heuristic.
    let took_over = before != after || screen_changed > 5 || (distinct > 1 && !(0xE000..=0xFFFF).contains(&sys.cpu.pc));
    println!(
        "\nVerdict: {}",
        if took_over {
            "the loaded program is EXECUTING (machine state diverged from the BASIC prompt)."
        } else {
            "no clear sign of execution — investigate (still sitting like the idle prompt?)."
        }
    );
}

/// A snapshot of machine state used to detect the game taking over.
#[derive(PartialEq, Eq)]
struct Indicators {
    port01: u8,
    irq_vec: u16,
    nmi_vec: u16,
    d011: u8,
    d016: u8,
    d018: u8,
    d015: u8, // sprite enable
    d020: u8,
    d021: u8,
    sid_vol: u8, // $D418
}

impl Indicators {
    fn capture(sys: &mut System) -> Self {
        Indicators {
            port01: sys.bus.read(0x0001),
            irq_vec: sys.bus.ram[0x0314] as u16 | ((sys.bus.ram[0x0315] as u16) << 8),
            nmi_vec: sys.bus.ram[0x0318] as u16 | ((sys.bus.ram[0x0319] as u16) << 8),
            d011: sys.vic_reg(0x11),
            d016: sys.vic_reg(0x16),
            d018: sys.vic_reg(0x18),
            d015: sys.vic_reg(0x15),
            d020: sys.vic_reg(0x20),
            d021: sys.vic_reg(0x21),
            sid_vol: sys.bus.io.io_scratch[0x0418],
        }
    }

    fn print_diff(&self, other: &Indicators) {
        let row = |name: &str, a: u16, b: u16, hex4: bool| {
            let mark = if a != b { " <-- changed" } else { "" };
            if hex4 {
                println!("  {name:<16} ${a:04X}    ${b:04X}{mark}");
            } else {
                println!("  {name:<16} ${a:02X}      ${b:02X}{mark}");
            }
        };
        row("$01 port", self.port01 as u16, other.port01 as u16, false);
        row("IRQ vec $0314", self.irq_vec, other.irq_vec, true);
        row("NMI vec $0318", self.nmi_vec, other.nmi_vec, true);
        row("VIC $D011", self.d011 as u16, other.d011 as u16, false);
        row("VIC $D016", self.d016 as u16, other.d016 as u16, false);
        row("VIC $D018", self.d018 as u16, other.d018 as u16, false);
        row("sprites $D015", self.d015 as u16, other.d015 as u16, false);
        row("border $D020", self.d020 as u16, other.d020 as u16, false);
        row("bg $D021", self.d021 as u16, other.d021 as u16, false);
        row("SID vol $D418", self.sid_vol as u16, other.sid_vol as u16, false);
    }
}

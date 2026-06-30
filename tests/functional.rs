//! Klaus Dormann 6502 functional test harness.
//!
//! This is the gold-standard correctness test for a 6502 core. It exercises
//! every documented instruction and addressing mode and traps (loops on itself)
//! at a known "success" PC if everything passes.
//!
//! Setup:
//!   1. Get `6502_functional_test.bin` from
//!      https://github.com/Klaus2m5/6502_65C02_functional_tests
//!      (the prebuilt binary, assembled with `disable_decimal = 0` is fine since
//!      this core implements decimal mode).
//!   2. Place it at `roms/6502_functional_test.bin`.
//!   3. Run: `cargo test --release -- --ignored functional`
//!
//! The standard binary loads at $0000 with the entry point at $0400 and signals
//! success by an infinite "JMP to self" at PC $3469 (for the commonly used
//! build). If your build differs, adjust `SUCCESS_PC` below — on failure the
//! test prints the PC of the trap so you can look it up in the listing.

use c64emu::bus::FlatRam;
use c64emu::cpu::Cpu;
use std::path::Path;

const LOAD_ADDR: u16 = 0x0000;
const ENTRY_PC: u16 = 0x0400;
// Success trap for the canonical prebuilt binary. If you assemble your own,
// check the listing for the address of the final `success` label.
const SUCCESS_PC: u16 = 0x3469;

#[test]
#[ignore = "requires roms/6502_functional_test.bin"]
fn functional() {
    let path = Path::new("roms/6502_functional_test.bin");
    let data = std::fs::read(path).unwrap_or_else(|_| {
        panic!(
            "{} not found. Download the prebuilt 6502_functional_test.bin from \
             https://github.com/Klaus2m5/6502_65C02_functional_tests and place it there. \
             (This test only runs with `--ignored`, so a missing file is a real failure, \
             not a skip.)",
            path.display()
        )
    });

    let mut ram = FlatRam::new();
    ram.load(LOAD_ADDR, &data);

    let mut cpu = Cpu::new();
    cpu.pc = ENTRY_PC;

    let mut last_pc = cpu.pc;
    // The test is millions of instructions; cap generously.
    for _ in 0..200_000_000u64 {
        let pc_before = cpu.pc;
        cpu.step(&mut ram);

        if cpu.pc == pc_before {
            // The CPU is stuck on a "JMP to self" trap.
            if cpu.pc == SUCCESS_PC {
                return; // passed
            }
            panic!(
                "functional test trapped at ${:04X} (not the success address ${:04X}). \
                 Look this PC up in the test listing to find the failing case.",
                cpu.pc, SUCCESS_PC
            );
        }
        last_pc = pc_before;
    }
    let _ = last_pc;
    panic!("functional test did not reach a trap within the instruction budget");
}

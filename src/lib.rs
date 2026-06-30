//! c64emu library crate root.
//!
//! Exposes the emulator components so both the binary (`main.rs`) and the
//! integration tests under `tests/` can use them.

pub mod bus;
pub mod cia;
pub mod cpu;
pub mod d64;
pub mod roms;
pub mod sid;
pub mod system;
pub mod vic;
pub mod video;

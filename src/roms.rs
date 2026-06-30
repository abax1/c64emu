//! ROM loading helpers.
//!
//! The C64 ROMs are copyrighted and not distributed with this project. Drop your
//! own dumps into `roms/`:
//!   - `basic.901226-01.bin`        (8 KB)  -> $A000-$BFFF
//!   - `kernal.901227-03.bin`       (8 KB)  -> $E000-$FFFF
//!   - `characters.901225-01.bin`   (4 KB)  -> CHARGEN
//!
//! See https://www.zimmers.net/anonftp/pub/cbm/firmware/computers/c64/ for dumps.

use crate::bus::C64Bus;
use std::io;
use std::path::Path;

#[derive(Debug)]
pub enum RomError {
    Io(io::Error),
    WrongSize { name: &'static str, expected: usize, got: usize },
}

impl From<io::Error> for RomError {
    fn from(e: io::Error) -> Self {
        RomError::Io(e)
    }
}

/// Try each candidate filename in `dir`; load the first that exists and is the
/// right size. Accepts both the canonical dump names and VICE's bundled names.
fn load_fixed<const N: usize>(
    dir: &Path,
    name: &'static str,
    candidates: &[&str],
) -> Result<[u8; N], RomError> {
    let mut last_err: Option<io::Error> = None;
    for cand in candidates {
        let path = dir.join(cand);
        match std::fs::read(&path) {
            Ok(data) => {
                if data.len() != N {
                    return Err(RomError::WrongSize { name, expected: N, got: data.len() });
                }
                let mut out = [0u8; N];
                out.copy_from_slice(&data);
                return Ok(out);
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(RomError::Io(last_err.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, format!("no {name} ROM found"))
    })))
}

/// Load all three system ROMs from `dir` into the bus.
pub fn load_system_roms(bus: &mut C64Bus, dir: &Path) -> Result<(), RomError> {
    bus.basic_rom =
        load_fixed::<0x2000>(dir, "basic", &["basic.901226-01.bin", "basic.bin", "basic"])?;
    bus.kernal_rom =
        load_fixed::<0x2000>(dir, "kernal", &["kernal.901227-03.bin", "kernal.bin", "kernal"])?;
    bus.char_rom = load_fixed::<0x1000>(
        dir,
        "chargen",
        &["characters.901225-01.bin", "chargen.bin", "chargen", "characters.bin"],
    )?;
    Ok(())
}

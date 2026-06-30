//! D64 disk-image parsing (1541, 35 tracks, 683 sectors, no error info).
//!
//! Enough to read the directory and extract a file's bytes by following its
//! sector chain. This is *image* parsing only — it does not emulate the 1541
//! drive or the serial bus. Loading a file into the machine is done by the
//! caller (see `src/bin/d64test.rs`), which is sufficient for single-load
//! programs but not for fastloaders or copy-protected disks.

use std::collections::HashSet;
use std::io;
use std::path::Path;

const DISK_SIZE_35: usize = 174_848;
const DIR_TRACK: u8 = 18;

#[derive(Debug, Clone)]
pub struct DirEntry {
    /// File name as raw PETSCII bytes with $A0 padding stripped.
    pub name: Vec<u8>,
    /// File type low nibble (2 = PRG).
    pub file_type: u8,
    /// First track/sector of the file's data chain.
    pub track: u8,
    pub sector: u8,
    /// Size in blocks (sectors).
    pub blocks: u16,
}

impl DirEntry {
    /// File name as an ASCII string (PETSCII letters map closely enough).
    pub fn name_ascii(&self) -> String {
        self.name
            .iter()
            .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '?' })
            .collect()
    }

    pub fn is_prg(&self) -> bool {
        self.file_type & 0x0F == 2
    }
}

pub struct D64 {
    data: Vec<u8>,
}

impl D64 {
    pub fn load(path: &Path) -> io::Result<Self> {
        let data = std::fs::read(path)?;
        if data.len() < DISK_SIZE_35 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("file too small for a D64 ({} bytes)", data.len()),
            ));
        }
        Ok(D64 { data })
    }

    fn sectors_per_track(track: u8) -> usize {
        match track {
            1..=17 => 21,
            18..=24 => 19,
            25..=30 => 18,
            _ => 17,
        }
    }

    /// Byte offset of track/sector `(t, s)` within the image (1-based track).
    fn sector_offset(t: u8, s: u8) -> usize {
        let mut base = 0usize;
        for tr in 1..t {
            base += Self::sectors_per_track(tr);
        }
        (base + s as usize) * 256
    }

    /// 16-byte disk name from the BAM (track 18, sector 0, offset $90).
    pub fn disk_name(&self) -> String {
        let o = Self::sector_offset(DIR_TRACK, 0) + 0x90;
        self.data[o..o + 16]
            .iter()
            .map(|&b| if b == 0xA0 { ' ' } else if (0x20..0x7F).contains(&b) { b as char } else { '?' })
            .collect::<String>()
            .trim()
            .to_string()
    }

    /// Walk the directory chain starting at track 18, sector 1.
    pub fn directory(&self) -> Vec<DirEntry> {
        let mut out = Vec::new();
        let (mut t, mut s) = (DIR_TRACK, 1u8);
        let mut seen = HashSet::new();
        while t != 0 && seen.insert((t, s)) {
            let o = Self::sector_offset(t, s);
            let (nt, ns) = (self.data[o], self.data[o + 1]);
            for e in 0..8 {
                let eo = o + e * 32;
                let file_type = self.data[eo + 2];
                let track = self.data[eo + 3];
                let sector = self.data[eo + 4];
                // Skip scratched/empty slots (type 0 and no start track).
                if file_type & 0x0F == 0 && track == 0 {
                    continue;
                }
                let mut name: Vec<u8> = self.data[eo + 5..eo + 5 + 16].to_vec();
                while matches!(name.last(), Some(&0xA0)) {
                    name.pop();
                }
                let blocks = self.data[eo + 30] as u16 | ((self.data[eo + 31] as u16) << 8);
                out.push(DirEntry { name, file_type, track, sector, blocks });
            }
            t = nt;
            s = ns;
        }
        out
    }

    /// Find a directory entry by name. `"*"` returns the first PRG.
    pub fn find(&self, name: &str) -> Option<DirEntry> {
        let dir = self.directory();
        if name == "*" {
            return dir.into_iter().find(|e| e.is_prg());
        }
        let want = name.to_ascii_uppercase();
        dir.into_iter().find(|e| e.name_ascii().to_ascii_uppercase() == want)
    }

    /// Extract a file's full byte stream by following its sector chain.
    /// For a PRG the first two bytes are the little-endian load address.
    pub fn read_file(&self, entry: &DirEntry) -> Vec<u8> {
        let mut out = Vec::new();
        let (mut t, mut s) = (entry.track, entry.sector);
        let mut seen = HashSet::new();
        while t != 0 {
            if !seen.insert((t, s)) {
                break; // chain loop guard
            }
            let o = Self::sector_offset(t, s);
            let (nt, ns) = (self.data[o], self.data[o + 1]);
            if nt == 0 {
                // Last sector: `ns` is the offset of the final used byte.
                let used = (ns as usize).saturating_sub(1);
                out.extend_from_slice(&self.data[o + 2..o + 2 + used]);
                break;
            }
            out.extend_from_slice(&self.data[o + 2..o + 256]);
            t = nt;
            s = ns;
        }
        out
    }
}

/// Parse the `SYS <addr>` target from a tokenised BASIC autostart stub
/// (the payload, i.e. the file bytes after the 2-byte load address).
pub fn basic_sys_target(payload: &[u8]) -> Option<u16> {
    let p = payload.iter().position(|&b| b == 0x9E)?; // SYS token
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
    saw.then_some(n as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sys_stub() {
        // 0B 08  CA 07  9E  "2059"  00  -> SYS 2059
        let payload = [0x0B, 0x08, 0xCA, 0x07, 0x9E, b'2', b'0', b'5', b'9', 0x00];
        assert_eq!(basic_sys_target(&payload), Some(2059));
    }

    #[test]
    fn geometry_offsets() {
        // Track 1 sector 0 is offset 0; track 18 sector 0 is the BAM at 0x16500.
        assert_eq!(D64::sector_offset(1, 0), 0);
        assert_eq!(D64::sector_offset(18, 0), 0x16500);
        // 17 tracks * 21 sectors = 357 sectors before track 18.
        assert_eq!(D64::sector_offset(18, 0), 357 * 256);
    }

    #[test]
    fn sectors_per_track_zones() {
        assert_eq!(D64::sectors_per_track(1), 21);
        assert_eq!(D64::sectors_per_track(18), 19);
        assert_eq!(D64::sectors_per_track(25), 18);
        assert_eq!(D64::sectors_per_track(35), 17);
    }
}

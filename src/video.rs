//! VIC-II line-based renderer (background graphics; sprites added separately).
//!
//! `render_line` draws one raster line into an RGB framebuffer using the VIC
//! registers *as they currently stand*, so raster-IRQ effects (split screens,
//! per-line colour/mode changes) come out right. It reads video memory through
//! the VIC's view of the bus: the selected 16 KB bank (CIA #2 $DD00), the
//! video-matrix / character / bitmap bases ($D018), and the character-ROM
//! overlay that appears at $1000–$1FFF in banks 0 and 2.
//!
//! Supported modes: standard text, multicolour text, extended-background text,
//! standard (hi-res) bitmap, multicolour bitmap, plus border and DEN blanking.

use crate::bus::C64Bus;

/// Framebuffer dimensions (a generous PAL-ish visible window).
pub const FB_W: usize = 384;
pub const FB_H: usize = 272;

/// Raster line shown at the top of the framebuffer.
const Y_TOP: u16 = 16;
/// Vertical display window (RSEL = 1): raster lines 51..=250.
const DISP_TOP: u16 = 51;
const DISP_BOT: u16 = 251; // exclusive
/// Framebuffer X where the 320-pixel active area begins (CSEL = 1).
const X_LEFT: usize = 32;

/// C64 16-colour palette (Pepto), RGB.
pub const PALETTE: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00),
    (0xFF, 0xFF, 0xFF),
    (0x68, 0x37, 0x2B),
    (0x70, 0xA4, 0xB2),
    (0x6F, 0x3D, 0x86),
    (0x58, 0x8D, 0x43),
    (0x35, 0x28, 0x79),
    (0xB8, 0xC7, 0x6F),
    (0x6F, 0x4F, 0x25),
    (0x43, 0x39, 0x00),
    (0x9A, 0x67, 0x59),
    (0x44, 0x44, 0x44),
    (0x6C, 0x6C, 0x6C),
    (0x9A, 0xD2, 0x84),
    (0x6C, 0x5E, 0xB5),
    (0x95, 0x95, 0x95),
];

#[inline]
fn col(idx: u8) -> (u8, u8, u8) {
    PALETTE[(idx & 0x0F) as usize]
}

#[inline]
fn put(fb: &mut [u8], x: usize, y: usize, (r, g, b): (u8, u8, u8)) {
    if x >= FB_W || y >= FB_H {
        return;
    }
    let i = (y * FB_W + x) * 3;
    fb[i] = r;
    fb[i + 1] = g;
    fb[i + 2] = b;
}

/// Read a byte from the VIC's 14-bit address space (bank + char-ROM overlay).
fn vic_read(bus: &C64Bus, addr: u16) -> u8 {
    let pra = bus.io.cia2.pra;
    let bank_no = (!pra) & 0x03;
    let a = (addr & 0x3FFF) as usize;
    // Character ROM is visible to the VIC at $1000-$1FFF in banks 0 and 2.
    if (bank_no == 0 || bank_no == 2) && (0x1000..0x2000).contains(&a) {
        return bus.char_rom[a & 0x0FFF];
    }
    bus.ram[(bank_no as usize) * 0x4000 + a]
}

/// Render raster line `raster` into `fb`.
pub fn render_line(bus: &C64Bus, raster: u16, fb: &mut [u8]) {
    if raster < Y_TOP || raster >= Y_TOP + FB_H as u16 {
        return;
    }
    let y = (raster - Y_TOP) as usize;
    let vic = &bus.io.vic;

    let d011 = vic.reg(0x11);
    let d016 = vic.reg(0x16);
    let den = d011 & 0x10 != 0;
    let ecm = d011 & 0x40 != 0;
    let bmm = d011 & 0x20 != 0;
    let mcm = d016 & 0x10 != 0;

    let border = col(vic.reg(0x20));
    // Border fills the whole line first; the display window overwrites the middle.
    for x in 0..FB_W {
        put(fb, x, y, border);
    }

    let d018 = vic.reg(0x18);
    let vm_base = (((d018 >> 4) & 0x0F) as u16) * 0x0400;

    // Background graphics only inside the vertical display window (and when the
    // display is enabled). Sprites are drawn afterwards and may sit in the border.
    if den && raster >= DISP_TOP && raster < DISP_BOT {
        render_background(bus, vic, raster, vm_base, d018, ecm, bmm, mcm, fb, y);
    }

    render_sprites(bus, vic, raster, vm_base, fb, y);
}

/// Draw any sprites that intersect this raster line, on top of the background.
///
/// Priority handling is simplified: all sprites are drawn over the background
/// (the $D01B sprite-behind-foreground bit is not yet honoured), and lower-
/// numbered sprites are drawn last so they win overlaps (sprite 0 on top).
fn render_sprites(bus: &C64Bus, vic: &crate::vic::Vic, raster: u16, vm_base: u16, fb: &mut [u8], y: usize) {
    let enable = vic.reg(0x15);
    if enable == 0 {
        return;
    }
    let x_msb = vic.reg(0x10);
    let x_exp = vic.reg(0x1D);
    let y_exp = vic.reg(0x17);
    let mc_en = vic.reg(0x1C);
    let mc0 = col(vic.reg(0x25));
    let mc1 = col(vic.reg(0x26));

    // Sprite 0 has the highest priority: draw 7..0 so 0 is painted last.
    for i in (0..8usize).rev() {
        let bit = 1u8 << i;
        if enable & bit == 0 {
            continue;
        }
        let sy = vic.reg(0x01 + i * 2) as u16;
        let expand_y = y_exp & bit != 0;
        let height: u16 = if expand_y { 42 } else { 21 };
        if raster < sy || raster >= sy + height {
            continue;
        }
        let mut srow = raster - sy;
        if expand_y {
            srow /= 2;
        }

        // Sprite data: pointer byte at vm_base+$3F8+i, ×64 = data block.
        let ptr = vic_read(bus, vm_base + 0x03F8 + i as u16) as u16;
        let base = ptr * 64 + srow * 3;
        let data = [
            vic_read(bus, base),
            vic_read(bus, base + 1),
            vic_read(bus, base + 2),
        ];
        let getbit = |n: usize| -> u8 { (data[n / 8] >> (7 - (n % 8))) & 1 };

        let sx = vic.reg(0x00 + i * 2) as i32 | if x_msb & bit != 0 { 0x100 } else { 0 };
        // C64 X coordinate 24 == left edge of the 40-col display (fb X_LEFT).
        let x0 = X_LEFT as i32 + (sx - 24);
        let expand_x = x_exp & bit != 0;
        let color = col(vic.reg(0x27 + i));

        if mc_en & bit != 0 {
            // Multicolour: 12 pixel-pairs, each 2 (or 4 if X-expanded) px wide.
            let unit = if expand_x { 4 } else { 2 };
            for p in 0..12 {
                let pair = (getbit(p * 2) << 1) | getbit(p * 2 + 1);
                let pc = match pair {
                    0 => continue, // transparent
                    1 => mc0,
                    2 => color,
                    _ => mc1,
                };
                let px0 = x0 + (p as i32) * unit;
                for w in 0..unit {
                    put_i(fb, px0 + w, y, pc);
                }
            }
        } else {
            // Hi-res: 24 pixels, each 1 (or 2 if X-expanded) px wide.
            let unit = if expand_x { 2 } else { 1 };
            for p in 0..24 {
                if getbit(p) == 0 {
                    continue; // transparent
                }
                let px0 = x0 + (p as i32) * unit;
                for w in 0..unit {
                    put_i(fb, px0 + w, y, color);
                }
            }
        }
    }
}

/// `put` with a signed x (sprites can be partly off the left edge / in border).
#[inline]
fn put_i(fb: &mut [u8], x: i32, y: usize, color: (u8, u8, u8)) {
    if x >= 0 && (x as usize) < FB_W {
        put(fb, x as usize, y, color);
    }
}

/// Draw the 40-column background graphics for one line.
#[allow(clippy::too_many_arguments)]
fn render_background(
    bus: &C64Bus,
    vic: &crate::vic::Vic,
    raster: u16,
    vm_base: u16,
    d018: u8,
    ecm: bool,
    bmm: bool,
    mcm: bool,
    fb: &mut [u8],
    y: usize,
) {
    let line = raster - DISP_TOP; // 0..199
    let row = (line / 8) as usize;
    let pr = (line % 8) as u16;

    let char_base = (((d018 >> 1) & 0x07) as u16) * 0x0800;
    let bm_base = (((d018 >> 3) & 0x01) as u16) * 0x2000;
    let bg0 = col(vic.reg(0x21));

    for c in 0..40usize {
        let cell = row * 40 + c;
        let vm = vic_read(bus, vm_base + cell as u16);
        let cram = bus.io.color_ram[cell] & 0x0F;
        let xbase = X_LEFT + c * 8;

        if !bmm {
            // ----- Text modes -----
            let code = if ecm { (vm & 0x3F) as u16 } else { vm as u16 };
            let data = vic_read(bus, char_base + code * 8 + pr);

            if mcm && (cram & 0x08) != 0 {
                // Multicolour text: 4 double-wide pixels.
                let c01 = col(vic.reg(0x22));
                let c10 = col(vic.reg(0x23));
                let c11 = col(cram & 0x07);
                for p in 0..4 {
                    let bits = (data >> (6 - p * 2)) & 0x03;
                    let pc = match bits {
                        0 => bg0,
                        1 => c01,
                        2 => c10,
                        _ => c11,
                    };
                    put(fb, xbase + p * 2, y, pc);
                    put(fb, xbase + p * 2 + 1, y, pc);
                }
            } else {
                // Hi-res text (standard / ECM / mc-but-bit3-clear).
                let bg = if ecm {
                    col(vic.reg(0x21 + ((vm >> 6) & 0x03) as usize))
                } else {
                    bg0
                };
                let fg = if mcm { col(cram & 0x07) } else { col(cram) };
                for p in 0..8 {
                    let on = data & (0x80 >> p) != 0;
                    put(fb, xbase + p, y, if on { fg } else { bg });
                }
            }
        } else {
            // ----- Bitmap modes -----
            // cell*8 == row*320 + c*8, exactly the bitmap byte offset.
            let data = vic_read(bus, bm_base + (cell as u16) * 8 + pr);
            if !mcm {
                // Standard hi-res bitmap: vm hi nibble = '1' colour, lo = '0'.
                let fg = col(vm >> 4);
                let bg = col(vm & 0x0F);
                for p in 0..8 {
                    let on = data & (0x80 >> p) != 0;
                    put(fb, xbase + p, y, if on { fg } else { bg });
                }
            } else {
                // Multicolour bitmap.
                let c01 = col(vm >> 4);
                let c10 = col(vm & 0x0F);
                let c11 = col(cram);
                for p in 0..4 {
                    let bits = (data >> (6 - p * 2)) & 0x03;
                    let pc = match bits {
                        0 => bg0,
                        1 => c01,
                        2 => c10,
                        _ => c11,
                    };
                    put(fb, xbase + p * 2, y, pc);
                    put(fb, xbase + p * 2 + 1, y, pc);
                }
            }
        }
    }
}

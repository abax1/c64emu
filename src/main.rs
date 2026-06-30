//! c64emu SDL2 frontend.
//!
//! Opens a window, runs the emulated C64 one PAL frame per redraw, blits the
//! VIC-II framebuffer, and maps the host keyboard into the CIA #1 matrix so you
//! can type at BASIC.

use c64emu::d64::{basic_sys_target, D64};
use c64emu::sid::SAMPLE_RATE;
use c64emu::system::System;
use c64emu::video::{FB_H, FB_W};
use sdl2::audio::AudioSpecDesired;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use std::path::Path;
use std::time::Duration;

const SCALE: u32 = 2;
/// Bytes of mono i16 audio per PAL frame (50 Hz).
const AUDIO_BYTES_PER_FRAME: u32 = (SAMPLE_RATE / 50) * 2;

/// Map an SDL keycode to a C64 keyboard matrix position `(column, row)`.
fn matrix_pos(key: Keycode) -> Option<(u8, u8)> {
    use Keycode::*;
    let pos = match key {
        A => (1, 2), B => (3, 4), C => (2, 4), D => (2, 2), E => (1, 6),
        F => (2, 5), G => (3, 2), H => (3, 5), I => (4, 1), J => (4, 2),
        K => (4, 5), L => (5, 2), M => (4, 4), N => (4, 7), O => (4, 6),
        P => (5, 1), Q => (7, 6), R => (2, 1), S => (1, 5), T => (2, 6),
        U => (3, 6), V => (3, 7), W => (1, 1), X => (2, 7), Y => (3, 1),
        Z => (1, 4),
        Num0 => (4, 3), Num1 => (7, 0), Num2 => (7, 3), Num3 => (1, 0),
        Num4 => (1, 3), Num5 => (2, 0), Num6 => (2, 3), Num7 => (3, 0),
        Num8 => (3, 3), Num9 => (4, 0),
        Return => (0, 1),
        Space => (7, 4),
        Tab | Escape => (7, 7), // RUN/STOP
        Backspace => (0, 0),
        Right => (0, 2),
        Down => (0, 7),
        LShift => (1, 7),
        RShift => (6, 4),
        LCtrl => (7, 2),
        Comma => (5, 7),
        Period => (5, 4),
        Slash => (6, 7),
        Semicolon => (6, 2),
        Minus => (5, 3),
        Equals => (6, 5),
        _ => return None,
    };
    Some(pos)
}

/// Map a key to a joystick-2 input: index 0=up 1=down 2=left 3=right 4=fire.
/// Arrow keys drive the stick; Right-Ctrl is fire.
fn joy_index(key: Keycode) -> Option<usize> {
    use Keycode::*;
    Some(match key {
        Up => 0,
        Down => 1,
        Left => 2,
        Right => 3,
        // Fire: Right-Shift sits by the arrows and, unlike Ctrl/Option + arrow,
        // is NOT a macOS system shortcut, so the OS won't swallow it.
        RShift => 4,
        _ => return None,
    })
}

fn main() {
    let mut sys = System::new();
    match sys.load_roms(Path::new("roms")) {
        Ok(()) => println!("Loaded ROMs."),
        Err(e) => eprintln!("ROMs not loaded ({e:?}); screen will be blank. See src/roms.rs."),
    }
    sys.reset();

    // Optional: a .d64 path on the command line is booted and autostarted,
    // like LOAD"*",8,1 then RUN.
    if let Some(path) = std::env::args().nth(1) {
        // Run frames until the BASIC "READY." prompt appears (or give up).
        for _ in 0..250 {
            sys.run_frame();
            let scr = &sys.bus.ram[0x0400..0x0400 + 1000];
            if scr.windows(6).any(|w| w == [18, 5, 1, 4, 25, 46]) {
                break;
            }
        }
        match D64::load(Path::new(&path)) {
            Ok(disk) => match disk.find("*") {
                Some(entry) => {
                    let bytes = disk.read_file(&entry);
                    if bytes.len() >= 3 {
                        let load = bytes[0] as u16 | ((bytes[1] as u16) << 8);
                        for (i, &b) in bytes[2..].iter().enumerate() {
                            sys.bus.ram[(load as usize + i) & 0xFFFF] = b;
                        }
                        let target =
                            basic_sys_target(&bytes[2..]).unwrap_or(load.wrapping_add(0x0A));
                        sys.cpu.pc = target;
                        println!("Loaded \"{}\" -> SYS ${target:04X}", entry.name_ascii());
                    }
                }
                None => eprintln!("no PRG on disk {path}"),
            },
            Err(e) => eprintln!("disk load failed: {e}"),
        }
    }

    let sdl = sdl2::init().expect("sdl init");
    let video = sdl.video().expect("sdl video");
    let window = video
        .window("c64emu", FB_W as u32 * SCALE, FB_H as u32 * SCALE)
        .position_centered()
        .build()
        .expect("window");
    // No vsync: timing is driven by the audio queue (below), which also keeps
    // the PAL machine at the correct 50 Hz speed/pitch on a 60 Hz display.
    let mut canvas = window.into_canvas().build().expect("canvas");
    let texture_creator = canvas.texture_creator();
    let mut texture = texture_creator
        .create_texture_streaming(PixelFormatEnum::RGB24, FB_W as u32, FB_H as u32)
        .expect("texture");

    // Audio: a queue we push SID samples into each frame.
    let audio = sdl.audio().expect("sdl audio");
    let desired = AudioSpecDesired {
        freq: Some(SAMPLE_RATE as i32),
        channels: Some(1),
        samples: Some(1024),
    };
    let queue = audio
        .open_queue::<i16, _>(None, &desired)
        .expect("audio queue");
    queue.resume();

    let mut event_pump = sdl.event_pump().expect("event pump");
    // Joystick-2 state: [up, down, left, right, fire].
    let mut joy = [false; 5];

    'running: loop {
        for event in event_pump.poll_iter() {
            match event {
                // Quit via the window's close button (Escape is now RUN/STOP).
                Event::Quit { .. } => break 'running,
                Event::KeyDown { keycode: Some(k), .. } => {
                    if let Some(j) = joy_index(k) {
                        joy[j] = true;
                        sys.set_joy2(joy[0], joy[1], joy[2], joy[3], joy[4]);
                    } else if let Some((c, r)) = matrix_pos(k) {
                        sys.key(c, r, true);
                    }
                }
                Event::KeyUp { keycode: Some(k), .. } => {
                    if let Some(j) = joy_index(k) {
                        joy[j] = false;
                        sys.set_joy2(joy[0], joy[1], joy[2], joy[3], joy[4]);
                    } else if let Some((c, r)) = matrix_pos(k) {
                        sys.key(c, r, false);
                    }
                }
                _ => {}
            }
        }

        sys.run_frame();

        // Queue this frame's audio.
        let samples = sys.take_audio();
        if !samples.is_empty() {
            queue.queue_audio(&samples).expect("queue audio");
        }

        texture.update(None, &sys.framebuffer, FB_W * 3).expect("tex update");
        canvas.clear();
        canvas.copy(&texture, None, None).expect("copy");
        canvas.present();

        // Pace to real time: don't get more than a few frames ahead of audio.
        while queue.size() > AUDIO_BYTES_PER_FRAME * 4 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

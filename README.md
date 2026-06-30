# c64emu

A Commodore 64 emulator written from scratch in Rust. It boots the real C64
KERNAL/BASIC ROMs to the `READY.` prompt, renders VIC-II graphics (text, bitmap,
and sprites), produces SID audio, takes keyboard and joystick input, and loads
programs from `.d64` disk images — enough to play real games.

> **You provide the ROMs and games.** No copyrighted Commodore ROMs or
> commercial software are included. See [Legal](#legal).

## Features

- **MOS 6510 CPU** — full documented instruction set, all addressing modes,
  decimal mode, and interrupts. Verified against Klaus Dormann's 6502 functional
  test suite.
- **Memory banking** — the C64 PLA banking driven by the 6510 processor port
  (RAM / BASIC / KERNAL / CHARGEN / I/O).
- **VIC-II video** — per-scanline rendering (raster-split effects work):
  standard/multicolour/extended text, hi-res and multicolour bitmap, 8 hardware
  sprites (multicolour, X/Y expansion), border, and the 16-colour palette.
- **SID audio** — 3 voices (triangle/sawtooth/pulse/noise), ring modulation,
  ADSR envelopes, master volume. (The analog filter is not yet modelled.)
- **CIA #1/#2** — timers and interrupts, keyboard matrix, joystick.
- **Input** — host keyboard mapped to the C64 matrix, plus joystick port 2.
- **Disk** — reads `.d64` images and loads/starts single-load programs.

## Build

Requires a Rust toolchain and the SDL2 native library.

```sh
# macOS
brew install sdl2 pkg-config

# Debian/Ubuntu
sudo apt install libsdl2-dev pkg-config

cargo build --release
```

On Apple Silicon you may need to help the linker find Homebrew:

```sh
export LIBRARY_PATH="/opt/homebrew/lib:$LIBRARY_PATH"
```

## Run

1. Put your ROMs in `roms/` (see [roms/README.md](roms/README.md)).
2. Boot to BASIC:

   ```sh
   cargo run --release
   ```

3. Or boot and start a disk:

   ```sh
   cargo run --release -- "disks/Your Game.d64"
   ```

There is also a headless disk-loader diagnostic:

```sh
cargo run --release --bin d64test -- "disks/Your Game.d64"
```

## Controls

| Key                 | C64                          |
|---------------------|------------------------------|
| Letters / digits    | Keyboard                     |
| Return, Space       | Return, Space                |
| Backspace           | INST/DEL                     |
| Esc / Tab           | RUN/STOP                     |
| Arrow keys          | Joystick 2 (directions)      |
| Right Shift         | Joystick 2 fire              |
| Window close button | Quit                         |

> macOS note: Ctrl/Option + arrow are system shortcuts, so fire is mapped to
> Right Shift, which doesn't collide with them.

## Testing

```sh
cargo test
```

Unit tests cover the CPU, CIA, VIC, SID, and D64 parser. The full 6502
functional test is opt-in (it needs an external binary):

```sh
# place 6502_functional_test.bin in roms/ first — see tests/functional.rs
cargo test --release -- --ignored functional
```

## Architecture

```
src/
  cpu.rs      MOS 6510 core (+ unit tests)
  bus.rs      64 KB memory map, PLA banking, I/O routing
  vic.rs      VIC-II registers + raster timing
  video.rs    VIC-II per-scanline renderer (text/bitmap/sprites)
  sid.rs      SID audio synthesis
  cia.rs      6526 CIA (timers, keyboard, joystick)
  d64.rs      .d64 disk-image parsing
  roms.rs     ROM loading
  system.rs   wires CPU + bus, runs frames, owns the framebuffer
  main.rs     SDL2 frontend (video, audio, input)
  bin/d64test.rs  headless disk load/run diagnostic
```

## Roadmap

- SID analog filter for accurate timbre
- Sprite/background priority and sprite collision registers
- Joystick port 1; physical gamepad (SDL GameController); fullscreen
- KERNAL LOAD trap so `LOAD"*",8,1` works interactively
- 1541 drive emulation for fastloaded / copy-protected disks
- Save states

## Legal

Emulators are legal; the copyrighted bits are not mine to ship. This repository
contains **only original emulator code**. It does **not** include:

- the Commodore 64 system ROMs (KERNAL, BASIC, CHARGEN), or
- any commercial games or disk images.

You must supply your own ROMs and any software you are legally entitled to run.
"Commodore" and "Commodore 64" are trademarks of their respective owners; this
project is not affiliated with or endorsed by them.

## License

[MIT](LICENSE) © 2026 Andrew Baxter.

## Acknowledgements

- Klaus Dormann's [6502 functional tests](https://github.com/Klaus2m5/6502_65C02_functional_tests)
- The [VICE](https://vice-emu.sourceforge.io/) project as a reference
- The "Pepto" PAL colour palette

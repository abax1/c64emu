# ROMs (bring your own)

The Commodore 64 system ROMs are copyrighted and are **not** distributed with
this project. To run the emulator you must supply your own copies, placed in
this directory:

| File      | Size    | Maps to        |
|-----------|---------|----------------|
| `basic`   | 8192 B  | BASIC ROM      |
| `kernal`  | 8192 B  | KERNAL ROM     |
| `chargen` | 4096 B  | character ROM  |

The loader also accepts the canonical dump filenames (e.g.
`basic.901226-01.bin`, `kernal.901227-03.bin`, `characters.901225-01.bin`) and
`*.bin` variants — see `src/roms.rs`.

## Where to get them legally

If you own a Commodore 64, you can dump your own ROMs. They are also bundled
with the free, open-source [VICE emulator](https://vice-emu.sourceforge.io/);
after installing VICE the files are typically under its `C64/` data directory
named `basic`, `kernal`, and `chargen` — copy those here.

Everything in this directory except this README is git-ignored.

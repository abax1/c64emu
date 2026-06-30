# Contributing

Thanks for your interest in c64emu! Contributions are welcome.

## Ground rules

- **Never commit copyrighted material.** No Commodore ROMs (KERNAL/BASIC/
  CHARGEN), no games, no disk/tape/cartridge images. The `.gitignore` blocks the
  common formats, but please double-check `git status` before committing.
- By contributing, you agree your contributions are licensed under the project's
  [MIT License](LICENSE).

## Development

```sh
cargo build
cargo test
cargo fmt
cargo clippy
```

Please run `cargo fmt` and make sure `cargo test` passes before opening a pull
request. New behaviour should come with tests where practical — the CPU, CIA,
VIC, SID, and D64 modules all have `#[cfg(test)]` units to follow.

For correctness work on the CPU, the Klaus Dormann functional test
(`cargo test --release -- --ignored functional`, see `tests/functional.rs`) is
the gold standard — keep it passing.

## Good first areas

See the Roadmap in the [README](README.md). High-value, well-scoped items:

- SID analog filter
- Sprite/background priority and collision registers
- Joystick port 1 and SDL GameController (physical gamepad) support
- Fullscreen toggle
- A KERNAL LOAD trap so `LOAD"*",8,1` works from the live machine

## Reporting bugs

Open an issue describing what you ran (which program/ROM revision), what you
expected, and what happened. Screenshots help for video issues; for CPU/timing
issues, a minimal reproducing program is ideal.

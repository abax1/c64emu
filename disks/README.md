# Disks (bring your own)

Place `.d64` disk images you legally own in this directory, then run:

```sh
cargo run -- "disks/Your Game.d64"
```

Commercial game images are copyrighted; none are distributed with this project.
Everything in this directory except this README is git-ignored.

The loader currently handles single-load programs by reading the first PRG from
the image and starting it (it is not a full 1541 drive emulation, so fastloaders
and copy-protected disks may not work).

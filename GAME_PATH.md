# Original game location (external, read-only reference)

The original *Need for Speed: Porsche Unleashed* installation lives **outside** this repository:

```
~/Games/NFS-Porsche-Unleashed/
```

The actual game root (the directory the original executable runs from, with `GameData/`, `FEData/`, `nfs5.exe`) is:

```
~/Games/NFS-Porsche-Unleashed/game_info/data/
```

Everything under that tree is **reference material only**. Nothing in it is ever modified, renamed,
patched, copied wholesale into this repository or committed to Git. Tools that need writable data
copy individual files elsewhere first.

## How the native code locates it

Runtime code and tools resolve the game root in this order:

1. `NFS5_GAME_PATH` environment variable, if set.
2. `~/Games/NFS-Porsche-Unleashed/game_info/data`.
3. `~/Games/NFS-Porsche-Unleashed` (if it directly contains `GameData/`).

See `crates/nfs-assets/src/paths.rs`.

## What is there

See `RE/BINARY.md` for the executable triage and `RE/FORMATS.md` for the data file inventory.

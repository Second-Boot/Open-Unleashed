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
2. `game/` beside the executable (the Windows / Linux archives).
3. `<binary dir>/../Resources/game` — the data a self-contained app bundle carries
   (`packaging/make_app.sh --with-data`: a private build for your own machines, never to be shared — the assets are EA's).
4. on macOS, `game/` beside the `.app`.
5. `~/Games/NFS-Porsche-Unleashed/game_info/data`.
6. `~/Games/NFS-Porsche-Unleashed` (if it directly contains `GameData/`).

See `crates/nfs-assets/src/paths.rs`.

# Building from source

**English** | [Русский](../ru/building.md)

## Requirements

- Rust 1.95 or newer (`rustup`);
- CMake 3.16 or newer and a C compiler (Xcode command line tools, Visual Studio build tools or `build-essential`): the
  controller library SDL3 (zlib licence) is built from the source the `sdl3-src` crate ships (through `sdl3-sys`) and
  linked statically, joystick / gamepad / haptic parts only, so nothing has to be installed system-wide;
- the game data as described in [game data](game-data.md);
- Linux: the development packages `libasound2-dev libudev-dev libx11-dev libxkbcommon-dev libwayland-dev
  libxcursor-dev libxrandr-dev libxi-dev libxinerama-dev libvulkan-dev pkg-config`.

## Run

```sh
cargo run --release -p open-unleashed                 # as the original: intro movies, then the front end
cargo run --release -p open-unleashed -- alps 993     # straight into a race (track, car)
cargo run --release -p open-unleashed -- --debug-menu # the developer text menu: quick race, career, missions…
```

## Tests

```sh
cargo test --workspace
```

Tests that read the original data skip themselves when the game is not found.

## Release archives

```sh
packaging/build_macos.sh      # universal macOS app (arm64 + x86_64)
packaging/build_windows.sh    # Windows x64, cross-built in Docker (+ the official DirectX Shader Compiler: dxcompiler.dll, dxil.dll beside the exe)
packaging/build_linux.sh      # Linux x86_64, built in Docker
```

The archives go to `target/dist/`. They contain the engine, an empty `game/` folder with instructions, `LICENSE.txt`
and `THIRD-PARTY-LICENSES.txt` — never any file of the original game.

## Command-line options

| Option | Effect |
|---|---|
| `<track> <car>` | start straight into a race, e.g. `alps 993` |
| `--opponents <n>`, `--laps <n>`, `--backwards`, `--quick-knockout` | quick-race opponents, laps, direction, lap knockout |
| `--career <tournament id>` | the next event of that Evolution tournament from the saved career |
| `--mission` | the current Factory Driver mission (`NFS_MISSION=<idx>` picks one) |
| `--replay <file>` | play back a saved replay |
| `--fe=<screen>` | open the front end on a given screen (`garage`, `carcompare`, …) |
| `--debug-menu` | the developer text menu |
| `--autotest <s> [--follow]` | drive for `s` seconds without a window, then exit (testing) |
| `--screenshot <png>` | render one frame to a file |
| `--record <csv>` | per-tick controls / car state (testing) |
| `--layout <screen>` | draw a front-end screen and walk its buttons (testing) |

## Environment variables

| Variable | Effect |
|---|---|
| `NFS5_GAME_PATH=<folder>` | the game data folder |
| `NFS5_SAVE_DIR=<folder>` | the saves folder |
| `NFS_TRAFFIC=0..3`, `NFS_TRAFFIC_SEED=<n>` | traffic density (none / light / medium / heavy), repeatable spawns |
| `NFS_NO_CARCOL=1` | no car-to-car collisions |
| `NFS_DEMO=1` | frame rate, GPU, graphics API and system in the window title; the window stays on top (for recordings) |
| `NFS_AUTOPILOT=1` with `--follow` | a windowed race drives itself (for recordings); `NFS_AUTOPILOT=ai` — the opponents' AI drives the player's car |
| `NFS_AI_TRACE=<idx>` | a quarter-second trace of one AI car in the log |
| `NFS_PHOTO_DIR=<folder>` | where photo mode saves its pictures (default `Pictures/PorscheUnleashed`) |
| `NFS_MIC=1` | voice chat from the microphone: in a network session the game sends your voice from the default input device to the other players, as the original did. Off by default (you still hear the others) |
| `WGPU_BACKEND=vulkan`/`gl`/`dx12`/`metal` | the graphics API (by default Vulkan / Metal / Direct3D 12, OpenGL only without them) |
| `NFS_GPU_PROFILE=1` | a per-pass GPU timing in the log, every 120 frames (diagnostics; slows the frame down). `NFS_GPU_PROFILE=minimal` asks for a plain GPU device with no optional features |
| `NFS_GUARD=0`/`1` | the start-up crash guard (see [install](install.md)): on by default in a normal release run, `0` turns it off, `1` forces it |

## `nfs5.ini`

The port reads the `nfs5.ini` of the Verok patch, from the game data folder (beside `nfs5.exe`), with the same keys, defaults and effects as the patched original; without the file the defaults apply. Section `[NFS5]`:

| Key (default) | Effect |
|---|---|
| `Language=English` | `English`, `French`, `German`, `Italian`, `Spanish`, `Swedish`, `PortBrzl` (any case); anything else is English |
| `NoMovies=0` | 1 skips the start-up movies |
| `Fog=1` | 0 turns the weather fog off |
| `Filtering=1` | 0 samples textures with nearest-neighbour instead of bilinear filtering |
| `CarControl=0` | 1 keeps the steering, throttle and brake live after a human car has finished the race |

`ThrashDriver`, `WindowedMode`, `SingleProcAffinity`, `CpuLimit` and `NoErrorReporting` belong to the original's Windows / Direct3D start-up and are ignored.

## Code layout

| Crate | Role |
|---|---|
| `nfs-formats` | the original's file formats |
| `nfs-assets` | game-data location, tracks, cars, textures |
| `nfs-physics` | vehicle model, collisions |
| `nfs-game` | race logic, AI, traffic, police, career, front-end runtime |
| `nfs-renderer` | wgpu renderer reproducing the original's look |
| `nfs-audio`, `nfs-input`, `nfs-net` | sound, controls and force feedback, the network protocol |
| `open-unleashed` | the game application |
| `tools/` | asset viewers and extractors |

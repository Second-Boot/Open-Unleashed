# Installation and first start

**English** | [Русский](../ru/install.md)

## What you need

- Your own copy of **Need for Speed: Porsche Unleashed** (PC, 2000), installed or copied from the disc.
  The engine uses its data; nothing of the original game is included.
- One of:
  - **macOS** 13 or newer, Apple Silicon or Intel (one universal app);
  - **Windows** 10 or newer, 64-bit, a GPU driver with DirectX 12 or Vulkan;
  - **Linux** x86_64 with glibc 2.36 or newer (Debian 12, Ubuntu 23.04 and later), a Vulkan driver (or, without
    one, OpenGL 3.3), X11 or Wayland.

## Steps

1. Download the archive for your system from the releases and unpack it.
2. Copy the folders **`GameData`** and **`FEData`** from the installed game (the folder that holds `nfs5.exe`) into
   the **`game`** folder of the unpacked archive. For network games also copy `version.txt` (the game's version,
   `3.5`: every player of a race must have the same). Optionally also copy `nfs5.exe` (the intro movies read their
   tables from it; it is never run) and `fe.txt`. Other places the engine looks: [game data](game-data.md).
3. Start the game:
   - **macOS:** `OpenUnleashed.app`. The app is not notarised: the first time, right-click it → **Open**, or run
     `xattr -dr com.apple.quarantine OpenUnleashed.app` in Terminal.
   - **Windows:** `OpenUnleashed.exe`.
   - **Linux:** `./run.sh` (or `./OpenUnleashed` from the archive's folder).

On Linux the runtime needs (Debian / Ubuntu package names) `libasound2` or `libasound2t64` (sound), `libudev1`
(gamepads), `libvulkan1` and a Vulkan driver (`mesa-vulkan-drivers` or your vendor's); without Vulkan the game
falls back to OpenGL 3.3 through EGL (`libegl1` and `libgl1-mesa-dri` or your vendor's driver).

## Network games

Network races use **UDP port 9442**.

- **macOS** asks to allow **Local Network** on the first start — press Allow (later: System Settings → Privacy &
  Security → Local Network).
- **Windows** may show a firewall prompt — allow private networks for LAN / internet play.
- **Linux:** open UDP 9442 in your firewall if you host.

## Saves, settings and the log

| System | Folder |
|---|---|
| macOS | `~/Library/Application Support/PorscheUnleashed` |
| Windows | `%APPDATA%\PorscheUnleashed` |
| Linux | `$XDG_DATA_HOME/PorscheUnleashed`, else `~/.local/share/PorscheUnleashed` |

Career saves are written as the original writes them (`SaveData/…sav`). `open-unleashed.log` in the same folder records
each start — attach it when you report a problem. `NFS5_SAVE_DIR=<folder>` points the saves elsewhere.

## If something goes wrong

- **"original game data not found"** — the `game` folder does not hold `GameData` and `FEData`, or they are one level
  deeper. See [game data](game-data.md).
- **The game does not start / crashes at start-up** — look at `open-unleashed.log` (above) and report it with your system
  and GPU.

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

### Internet games

There is no public lobby server yet, so the **internet** button is hidden until an address is set (1 or 2 below).

**multiplayer → internet** (the sign-on screen) connects to an online lobby over the same UDP protocol: type an account and a
password, press **sign on**, choose a server, then create or join a game there. The lobby's address is set in this order:

1. the environment variable `NFS_LOBBY_ADDRESS=host[:port]`;
2. a line `address=host[:port]` in the text file `lobby.txt` in the saves folder (see below);
3. the address built into the game (the port defaults to 9442).

**register** opens the registration page in your browser. The server list shows only servers that answer a ping; on a system
that does not allow an unprivileged ping, every listed server shows the time of your own connection instead.

## Saves, settings and the log

| System | Folder |
|---|---|
| macOS | `~/Library/Application Support/PorscheUnleashed` |
| Windows | `%APPDATA%\PorscheUnleashed` |
| Linux | `$XDG_DATA_HOME/PorscheUnleashed`, else `~/.local/share/PorscheUnleashed` |

Career saves are written as the original writes them (`SaveData/…sav`). `open-unleashed.log` in the same folder records
each start — attach it when you report a problem. `enhancements.json` there holds the choices of Options > graphics > enhancements (delete it to get the defaults back; in a safe-mode start those choices are ignored once). `NFS5_SAVE_DIR=<folder>` points the saves elsewhere.

## If something goes wrong

- **"original game data not found"** — the `game` folder does not hold `GameData` and `FEData`, or they are one level
  deeper. See [game data](game-data.md).
- **The game does not start / crashes at start-up** — look at `open-unleashed.log` (above) and report it with your system
  and GPU. A start that follows a crashed one runs once in a safe mode (a plain GPU device, on Windows Direct3D 12) and
  the log says so; if the next start is normal again, nothing is needed. On Windows an Intel integrated GPU uses
  Direct3D 12 by default, and the archive carries `dxcompiler.dll` and `dxil.dll` (the DirectX Shader Compiler) beside
  the exe; `WGPU_BACKEND=vulkan` (or `dx12` / `gl`) picks the graphics API by hand.

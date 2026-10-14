<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="packaging/brand/mark-dark.svg">
    <img src="packaging/brand/mark-light.svg" alt="OpenUnleashed" width="180">
  </picture>
</p>

<h1 align="center">OpenUnleashed</h1>

<p align="center"><b>English</b> | <a href="README.ru.md">Русский</a></p>

An unofficial, open-source reimplementation of the engine of **Need for Speed: Porsche Unleashed** (PC, 2000).
It runs natively on macOS (Apple Silicon and Intel), Windows and Linux — no Wine, no emulation, no original
executable — and plays like the original: physics, opponents and traffic, the Evolution career, Factory Driver,
the original menus, and network races, including races against the original game.

An independent implementation written from scratch in Rust (winit + wgpu) that reproduces the original's behaviour.

> **You need your own copy of the game.** No file of the original game is in this repository or in its releases.
> The project is not affiliated with Electronic Arts, Eden Games or Porsche AG; "Need for Speed" and "Porsche" are
> trademarks of their owners and are named here only to say which game this engine runs.

## Documentation

| | English | Русский |
|---|---|---|
| Installation and first start | [docs/en/install.md](docs/en/install.md) | [docs/ru/install.md](docs/ru/install.md) |
| Where the game data goes | [docs/en/game-data.md](docs/en/game-data.md) | [docs/ru/game-data.md](docs/ru/game-data.md) |
| Controls | [docs/en/controls.md](docs/en/controls.md) | [docs/ru/controls.md](docs/ru/controls.md) |
| Building from source, options | [docs/en/building.md](docs/en/building.md) | [docs/ru/building.md](docs/ru/building.md) |

## Quick start

1. Download the archive for your system from the releases.
2. Copy the `GameData` and `FEData` folders of your installed game into the `game` folder of the archive.
3. Start `OpenUnleashed` (on macOS, `OpenUnleashed.app`).

Details and troubleshooting: [installation](docs/en/install.md).

## Licence

GNU General Public License v3.0 or later ([LICENSE](LICENSE)). Third-party components keep their own licences;
the release archives list them in `THIRD-PARTY-LICENSES.txt`.

## Acknowledgements

The community patch 1.0.6 by Verok, whose run-time fixes to the original executable this engine reproduces where
they change the game's behaviour.

*OpenUnleashed is a working name.*

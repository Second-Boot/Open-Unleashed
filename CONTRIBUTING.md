# Contributing

**English** | [Русский](CONTRIBUTING.ru.md)

## Reporting a problem

Open an issue with the matching template and include:

- your system (OS and version, CPU, GPU and driver) and the archive or commit you run;
- what you did, what you expected and what happened instead;
- `open-unleashed.log` from the saves folder ([installation](docs/en/install.md#saves-settings-and-the-log)).

**Never attach files of the original game** — no `GameData` / `FEData` files, no `nfs5.exe`, no extracted textures,
sounds or models. A screenshot or a short video is fine. Issues may be written in English or Russian.

## Pull requests

- The aim is to play like the original. A change to game rules, physics, AI or the network protocol has to match what
  the original game does; say in the description how you checked it.
- `cargo build --workspace --all-targets` must finish without warnings, and `cargo test --workspace` must pass (the
  tests that need the game data skip themselves without it).
- Nothing of the original game goes into the repository, not even extracted or converted data.
- New dependencies must have a licence compatible with GPL-3.0 (MIT, Apache-2.0, BSD, Zlib, ISC, MPL-2.0, LGPL, CC0).
- Your contribution is licensed under the GNU GPL v3.0 or later, like the rest of the project.

Pull requests are merged into `main` as they are, so you stay the author of your commits.

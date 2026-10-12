# Game data

**English** | [Русский](../ru/game-data.md)

The engine contains no file of the original game. It reads the data of **your own copy** of Need for Speed: Porsche
Unleashed and never writes to it.

## What is needed

From the installed game — the folder that holds `nfs5.exe`:

| File / folder | Needed | What for |
|---|---|---|
| `GameData/` | yes | tracks, cars, physics, sounds |
| `FEData/` | yes | menus, fonts, texts, movies |
| `nfs5.exe` | no | the intro movies' decoder reads its tables from it (it is never run); without it the intro is skipped |
| `fe.txt` | no | the original's settings file; without it the defaults are used |

## Where the engine looks

In this order; the first folder that contains both `GameData` and `FEData` is used:

1. the folder in the environment variable `NFS5_GAME_PATH`;
2. `game/` beside the executable (the release archives come with an empty one);
3. macOS: `game/` beside `OpenUnleashed.app`, then inside the app bundle (`Contents/Resources/game`);
4. `~/Games/NFS-Porsche-Unleashed/game_info/data`, then `~/Games/NFS-Porsche-Unleashed`.

A common mistake is one folder too many: `game/data/GameData` instead of `game/GameData`.

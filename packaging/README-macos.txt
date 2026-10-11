@TITLE@ - macOS (Apple Silicon and Intel, universal)

@TITLE@ is an unofficial, fan-made reimplementation of the engine of Need for Speed: Porsche Unleashed
(PC, 2000). It is not affiliated with, endorsed or sponsored by Electronic Arts, Eden Games or Porsche AG;
all trademarks belong to their owners. No file of the original game is included: you need your own copy.
Licence: GNU GPL v3.0 or later (LICENSE.txt); third-party components: THIRD-PARTY-LICENSES.txt.

Game data: copy GameData and FEData of your installed game into the "game" folder beside @APP@.app
(game/PUT-GAME-DATA-HERE.txt), or into ~/Games/NFS-Porsche-Unleashed, or set NFS5_GAME_PATH.
Start: @APP@.app. The app is not notarised: on the first start use right click -> Open, or run
  xattr -dr com.apple.quarantine @APP@.app
Network games: macOS asks to allow "Local Network" on the first start - press Allow
(System Settings -> Privacy & Security -> Local Network). Games over IP use UDP port 9442.

Saves and settings: ~/Library/Application Support/PorscheUnleashed (NFS5_SAVE_DIR overrides).

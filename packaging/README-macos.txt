Porsche Unleashed (Rust port) - macOS (Apple Silicon and Intel, universal)

Start: PorscheUnleashed-Full.app (the game data is inside the app; NFS5_GAME_PATH overrides it).
The app is not notarised: on the first start use right click -> Open, or run
  xattr -dr com.apple.quarantine PorscheUnleashed-Full.app
Network games: macOS asks to allow "Local Network" on the first start - press Allow
(System Settings -> Privacy & Security -> Local Network). Games over IP use UDP port 9442.

Saves and settings: ~/Library/Application Support/PorscheUnleashed (NFS5_SAVE_DIR overrides).

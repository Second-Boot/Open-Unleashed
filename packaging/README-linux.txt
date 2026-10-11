@TITLE@ - Linux x86_64

@TITLE@ is an unofficial, fan-made reimplementation of the engine of Need for Speed: Porsche Unleashed
(PC, 2000). It is not affiliated with, endorsed or sponsored by Electronic Arts, Eden Games or Porsche AG;
all trademarks belong to their owners. No file of the original game is included: you need your own copy.
Licence: GNU GPL v3.0 or later (LICENSE.txt); third-party components: THIRD-PARTY-LICENSES.txt.

Game data: copy GameData and FEData of your installed game into the "game" folder beside the executable
(game/PUT-GAME-DATA-HERE.txt), or set NFS5_GAME_PATH to the installed game's folder.
Start: ./run.sh   (or ./@APP@ from this directory)

Runtime packages (Debian / Ubuntu names): libasound2 / libasound2t64 (sound), libudev1 (gamepads), a Vulkan or
OpenGL driver (mesa-vulkan-drivers / the vendor driver; libvulkan1), and an X11 or Wayland session.
Built against glibc 2.36 (Debian 12); newer systems work.

Saves and settings: $XDG_DATA_HOME/PorscheUnleashed, else ~/.local/share/PorscheUnleashed
(NFS5_SAVE_DIR overrides). Network games over IP use UDP port 9442 (open it in the firewall).

Porsche Unleashed (Rust port) - Linux x86_64

Start: ./run.sh   (or ./PorscheUnleashed from this directory)
The game data is the "game" directory beside the executable; NFS5_GAME_PATH overrides it.

Runtime packages (Debian / Ubuntu names): libasound2 / libasound2t64 (sound), libudev1 (gamepads), a Vulkan or
OpenGL driver (mesa-vulkan-drivers / the vendor driver; libvulkan1), and an X11 or Wayland session.
Built against glibc 2.36 (Debian 12); newer systems work.

Saves and settings: $XDG_DATA_HOME/PorscheUnleashed, else ~/.local/share/PorscheUnleashed
(NFS5_SAVE_DIR overrides). Network games use UDP port 1106 (open it in the firewall).

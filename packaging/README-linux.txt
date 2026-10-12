@TITLE@ - Linux x86_64
======================

@TITLE@ is an unofficial, fan-made reimplementation of the engine of Need for Speed: Porsche Unleashed
(PC, 2000). It is not affiliated with, endorsed or sponsored by Electronic Arts, Eden Games or Porsche AG;
all trademarks belong to their owners. No file of the original game is included: you need your own copy.
Licence: GNU GPL v3.0 or later (LICENSE.txt); third-party components: THIRD-PARTY-LICENSES.txt.

Game data: copy GameData and FEData of your installed game into the "game" folder beside the executable
(game/PUT-GAME-DATA-HERE.txt), or set NFS5_GAME_PATH to the installed game's folder.
Start: ./run.sh   (or ./@APP@ from this folder)
Runtime packages (Debian / Ubuntu): libasound2 or libasound2t64 (sound), libudev1 (gamepads), libvulkan1 and a
Vulkan driver (mesa-vulkan-drivers or the vendor's) or an OpenGL driver; an X11 or Wayland session. glibc 2.36 or newer.
Network races use UDP port 9442 (open it in the firewall to host).
Saves, settings and open-unleashed.log: $XDG_DATA_HOME/PorscheUnleashed, else ~/.local/share/PorscheUnleashed (NFS5_SAVE_DIR overrides).
More: docs/en/install.md in the source repository.


@TITLE@ - Linux x86_64
======================

@TITLE@ — неофициальная фанатская реализация движка Need for Speed: Porsche Unleashed (PC, 2000).
Проект не связан с Electronic Arts, Eden Games и Porsche AG; все товарные знаки принадлежат их владельцам.
Файлов оригинальной игры в комплекте нет: нужна своя копия.
Лицензия: GNU GPL v3.0 или новее (LICENSE.txt); сторонние компоненты — THIRD-PARTY-LICENSES.txt.

Данные игры: скопируйте GameData и FEData из установленной игры в папку "game" рядом с программой
(game/PUT-GAME-DATA-HERE.txt) или укажите в NFS5_GAME_PATH папку установленной игры.
Запуск: ./run.sh   (или ./@APP@ из этой папки)
Пакеты (Debian / Ubuntu): libasound2 или libasound2t64 (звук), libudev1 (геймпады), libvulkan1 и драйвер Vulkan
(mesa-vulkan-drivers или драйвер производителя) либо драйвер OpenGL; сеанс X11 или Wayland. glibc 2.36 или новее.
Сетевые гонки идут через UDP-порт 9442 (откройте его в брандмауэре, если создаёте игру).
Сохранения, настройки и open-unleashed.log: $XDG_DATA_HOME/PorscheUnleashed, иначе ~/.local/share/PorscheUnleashed (NFS5_SAVE_DIR меняет папку).
Подробнее: docs/ru/install.md в репозитории с исходниками.

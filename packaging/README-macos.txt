@TITLE@ - macOS (Apple Silicon and Intel, universal)
==================================================

@TITLE@ is an unofficial, fan-made reimplementation of the engine of Need for Speed: Porsche Unleashed
(PC, 2000). It is not affiliated with, endorsed or sponsored by Electronic Arts, Eden Games or Porsche AG;
all trademarks belong to their owners. No file of the original game is included: you need your own copy.
Licence: GNU GPL v3.0 or later (LICENSE.txt); third-party components: THIRD-PARTY-LICENSES.txt.

Game data: copy GameData and FEData of your installed game into the "game" folder beside @APP@.app
(game/PUT-GAME-DATA-HERE.txt), or into ~/Games/NFS-Porsche-Unleashed, or set NFS5_GAME_PATH.
Start: @APP@.app. The app is not notarised: the first time, right-click it -> Open, or run
  xattr -dr com.apple.quarantine @APP@.app
Network games: macOS asks to allow "Local Network" on the first start - press Allow
(System Settings -> Privacy & Security -> Local Network). Network races use UDP port 9442.
Saves, settings and open-unleashed.log: ~/Library/Application Support/PorscheUnleashed (NFS5_SAVE_DIR overrides).
More: docs/en/install.md in the source repository.


@TITLE@ - macOS (Apple Silicon и Intel, универсальная сборка)
=============================================================

@TITLE@ — неофициальная фанатская реализация движка Need for Speed: Porsche Unleashed (PC, 2000).
Проект не связан с Electronic Arts, Eden Games и Porsche AG; все товарные знаки принадлежат их владельцам.
Файлов оригинальной игры в комплекте нет: нужна своя копия.
Лицензия: GNU GPL v3.0 или новее (LICENSE.txt); сторонние компоненты — THIRD-PARTY-LICENSES.txt.

Данные игры: скопируйте GameData и FEData из установленной игры в папку "game" рядом с @APP@.app
(game/PUT-GAME-DATA-HERE.txt), или в ~/Games/NFS-Porsche-Unleashed, или укажите NFS5_GAME_PATH.
Запуск: @APP@.app. Приложение не нотаризовано: в первый раз нажмите правой кнопкой -> Открыть, или выполните
  xattr -dr com.apple.quarantine @APP@.app
Сетевая игра: при первом запуске macOS попросит доступ к "Локальной сети" - нажмите "Разрешить"
(Системные настройки -> Конфиденциальность и безопасность -> Локальная сеть). Сетевые гонки идут через UDP-порт 9442.
Сохранения, настройки и open-unleashed.log: ~/Library/Application Support/PorscheUnleashed (NFS5_SAVE_DIR меняет папку).
Подробнее: docs/ru/install.md в репозитории с исходниками.

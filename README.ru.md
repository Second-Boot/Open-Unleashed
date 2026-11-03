<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="packaging/brand/mark-dark.svg">
    <img src="packaging/brand/mark-light.svg" alt="OpenUnleashed" width="180">
  </picture>
</p>

<h1 align="center">OpenUnleashed</h1>

<p align="center"><a href="README.md">English</a> | <b>Русский</b></p>

Неофициальная открытая реализация движка **Need for Speed: Porsche Unleashed** (PC, 2000). Работает нативно на
macOS (Apple Silicon и Intel), Windows и Linux — без Wine, эмуляторов и оригинального exe — и играется как
оригинал: физика, соперники и трафик, карьера «Эволюция», «Гонщик-испытатель», оригинальные меню и сетевые гонки,
в том числе против оригинальной игры.

Независимая реализация, написанная с нуля на Rust (winit + wgpu) и воспроизводящая поведение оригинала.

> **Нужна своя копия игры.** Ни одного файла оригинальной игры нет ни в репозитории, ни в релизах.
> Проект не связан с Electronic Arts, Eden Games и Porsche AG; «Need for Speed» и «Porsche» — товарные знаки их
> владельцев и упоминаются здесь только для того, чтобы сказать, для какой игры этот движок.

## Документация

| | Русский | English |
|---|---|---|
| Установка и первый запуск | [docs/ru/install.md](docs/ru/install.md) | [docs/en/install.md](docs/en/install.md) |
| Куда положить данные игры | [docs/ru/game-data.md](docs/ru/game-data.md) | [docs/en/game-data.md](docs/en/game-data.md) |
| Управление | [docs/ru/controls.md](docs/ru/controls.md) | [docs/en/controls.md](docs/en/controls.md) |
| Сборка из исходников, параметры | [docs/ru/building.md](docs/ru/building.md) | [docs/en/building.md](docs/en/building.md) |
| Улучшения | [docs/ru/enhanced.md](docs/ru/enhanced.md) | [docs/en/enhanced.md](docs/en/enhanced.md) |

## Улучшения

Поверх точного порта в игре есть улучшения графики, по умолчанию включённые: свет, тени, отражения, сглаживание,
апскейлинг (AMD FidelityFX FSR 3), HD-текстуры, сделанные из вашей копии игры, и другое. У каждого улучшения свой
переключатель на экране «Улучшения» в игре, а **F12** сразу показывает графику оригинала. Игровой процесс, физика и
сетевой протокол не меняются. Подробнее: [docs/ru/enhanced.md](docs/ru/enhanced.md).

## Быстрый старт

1. Скачайте архив для своей системы из раздела релизов.
2. Скопируйте папки `GameData` и `FEData` из установленной игры в папку `game` из архива.
3. Запустите `OpenUnleashed` (на macOS — `OpenUnleashed.app`).

Подробности и решение проблем — в [инструкции по установке](docs/ru/install.md).

## Лицензия

GNU General Public License v3.0 или новее ([LICENSE](LICENSE)). Сторонние компоненты сохраняют свои лицензии;
их список — в файле `THIRD-PARTY-LICENSES.txt` в архивах релизов.

## Участие

Сообщения об ошибках и pull request: [CONTRIBUTING.ru.md](CONTRIBUTING.ru.md).

## Благодарности

Неофициальному патчу 1.0.6 от Verok: его исправления оригинального exe этот движок воспроизводит там, где они
меняют поведение игры.

Улучшения: AMD FidelityFX Super Resolution 3 через [wgpu-ffx](https://github.com/cwfitzgerald/wgpu-ffx)
(MIT, лежит в `third_party/`); DirectX Shader Compiler (сборки для Windows, лицензии в `licenses/dxc`); модели
апскейла для `tools/hd-textures`: 4x-PBRify (CC0) и 4xTextures_GTAV_rgt-s, автор **Philip Hofmann** (CC-BY-4.0).

*OpenUnleashed — рабочее название.*

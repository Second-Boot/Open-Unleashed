# OpenUnleashed

[English](README.md) | **Русский**

Неофициальная открытая реализация движка **Need for Speed: Porsche Unleashed** (PC, 2000). Работает нативно на
macOS (Apple Silicon и Intel), Windows и Linux — без Wine, эмуляторов и оригинального exe — и играется как
оригинал: физика, соперники и трафик, карьера «Эволюция», «Заводской пилот», оригинальные меню и сетевые гонки,
в том числе против оригинальной игры.

Поведение оригинала восстановлено реверс-инжинирингом и заново написано на Rust (winit + wgpu).

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

## Быстрый старт

1. Скачайте архив для своей системы из раздела релизов.
2. Скопируйте папки `GameData` и `FEData` из установленной игры в папку `game` из архива.
3. Запустите `OpenUnleashed` (на macOS — `OpenUnleashed.app`).

Подробности и решение проблем — в [инструкции по установке](docs/ru/install.md).

## Лицензия

GNU General Public License v3.0 или новее ([LICENSE](LICENSE)). Сторонние компоненты сохраняют свои лицензии;
их список — в файле `THIRD-PARTY-LICENSES.txt` в архивах релизов.

## Благодарности

Неофициальному патчу 1.0.6 от Verok: его исправления оригинального exe этот движок воспроизводит там, где они
меняют поведение игры.

*OpenUnleashed — рабочее название.*

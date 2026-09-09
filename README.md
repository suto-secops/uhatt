# uhatt

A task and time-tracking desktop app for KDE Plasma.

Rust core, Qt 6 / QML UI (via [`cxx-qt`](https://github.com/KDAB/cxx-qt)). Native window,
no web runtime.

## Status

Early development. Milestone 1 (in progress): projects, tasks, subtasks, deadlines, an
in-app timer plus manual time entry, and a time-invested graph per task/project by
day / week / month / year.

Planned for later: bulk task import from a standardized file format, AUR package,
release automation.

## Build

Requires a Rust toolchain (>= 1.85), CMake, and Qt 6 with QtDeclarative:

```sh
# Arch
sudo pacman -S --needed rust cargo cmake qt6-base qt6-declarative qt6-charts

cargo run
```

## License

[PolyForm Noncommercial License 1.0.0](LICENSE.md) - source-available; any noncommercial
use is permitted, commercial use is not.

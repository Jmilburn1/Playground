# FS Mod Manager

A Farming Simulator mod manager written in Rust, similar to Farm Sim Hub.

## Download (no build needed)

Pre-compiled Windows builds are attached to the
[Releases](https://github.com/Jmilburn1/Playground/releases) page:
download `FS-Mod-Manager-Windows.zip`, unzip it anywhere, and
double-click `fs-mod-manager.exe`. No Rust, no terminal.

## Features
- Browse, download, and manage mods for FS17, FS19, FS22, FS25
- Multiple folder support (archive + active directories)
- Move mods between directories
- Scan for mod issues and conflicts
- Search Mod Hub, GitHub, and KingMods for updates
- Fast scanning and conflict detection

## Prerequisites
- Rust (1.70+): https://rustup.rs/
- Cargo (comes with Rust)

## Setup

1. Install Rust:
```bash
# Windows
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

2. Navigate to the project directory and build the release executable:
```bat
cd /d E:\Playground
cargo build --release
```

3. Launch the executable (no Cargo command needed after the build):
```bat
target\release\fs-mod-manager.exe
```

You can double-click `fs-mod-manager.exe` in `target\release`, create a
desktop shortcut to it, or use `Launch-FS-Mod-Manager.bat` in this folder.
The launcher builds the release executable the first time if it is missing,
then starts the `.exe` directly on later runs.

## Project Structure
- `src/main.rs` - Application entry point
- `src/ui/` - UI modules (TUI/CLI)
- `src/mod_manager/` - Core mod management logic
- `src/scanner/` - Mod scanning and conflict detection
- `src/sources/` - Mod source integrations (Mod Hub, GitHub, KingMods)
- `src/config/` - Configuration management

## Dependencies
See `Cargo.toml` for the full list.
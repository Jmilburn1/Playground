<p align="center">
  <img src="assets/logo.png" alt="Playground logo" width="300">
</p>

# Playground

A farming simulator mod manager — by Farmer Jay.

Playground keeps a large mod library under control: browse your game
folder and your archive side by side as a card grid, spot conflicts,
favourite the mods you always come back to, and stage a different mod
set for each savegame.

## Download (no build needed)

Grab the pre-compiled Windows build from the
[Releases](https://github.com/Jmilburn1/Playground/releases) page:

1. Download `Playground-Windows.zip` from the latest release.
2. Unzip it anywhere.
3. Double-click `playground.exe`.

No Rust, no compiler, no terminal — and nothing to install. The exe is
portable: run it from the unzipped folder, a USB stick, wherever.

## First run

Open the **Settings** tab and point Playground at your folders:

- **Game mods folder** — your game's mods folder, e.g.
  `Documents\My Games\FarmingSimulator2025\mods`
- **Library folder** — wherever you keep the rest of your library, on any drive

Save, and Playground scans both folders.

## Features

- Card-grid library with each mod's real icon, category, author, and version
- Browse by category (tractors, trailers, maps, scripts, and more),
  Active / Archived, or Favourites
- Star favourites — they survive moving a mod between folders
- Search across the whole library
- Conflict detection (duplicate mods, Active/Archive overlaps)
- Move mods between Active and Archive, including across drives
- **Configure Save (work in progress)** — build a mod loadout for each
  savegame slot and apply it; Playground stages the right mods into the
  game folder and returns the rest to the archive
- Everything runs locally; your library never leaves your PC

## On the roadmap

- Update checks against ModHub, GitHub, and KingMods
- Support for FS17 / FS19 / FS22 profiles alongside FS25
- Launching the game from Playground

## Building from source (developers only)

Normal users should use the Releases download above. To build it
yourself you need Rust (stable, MSVC) on Windows:

```bat
cargo build --release
target\release\playground.exe
```

## Project structure

- `src/main.rs` — entry point (GUI, plus a small `cli` diagnostic mode)
- `src/ui/` — the Iced GUI: card grid, sidebar, save configurator
- `src/mod_manager/` — scanning, modDesc parsing, categories, conflict
  detection, cross-drive moves, savegame loadouts
- `src/sources/` — update-source integrations (in progress)

## Credits

V1.0.0 © 2026 Farmer Jay.
Created with the Assistance of Albert — Farmer Jay's Muse AI Agent.

<p align="center">
  <img src="assets/logo.png" alt="Playground logo" width="300">
</p>

# Playground

A farming simulator mod manager — by Farmer Jay.

Playground keeps a large mod library under control: browse your whole
library as a card grid, spot conflicts, keep a sorted master library,
stage a different mod set for each savegame, and let Playground watch
for mod updates in the background.

## Download (no build needed)

Grab the pre-compiled Windows build from the
[Releases](https://github.com/Jmilburn1/Playground/releases) page:

1. Download `Playground-Windows.zip` from the latest release.
2. Unzip it anywhere.
3. Double-click `playground.exe`.

No Rust, no compiler, no terminal — and nothing to install. The exe is
portable: run it from the unzipped folder, a USB stick, wherever.
Playground also updates itself: when a new release is published it
offers to download and install it for you.

## First run

Open the **Settings** tab and point Playground at your folders:

- **Game mods folder** — your game's mods folder, e.g.
  `Documents\My Games\FarmingSimulator2025\mods`
- **Library folder** — wherever you keep the rest of your library, on any drive

Save, and Playground scans both folders. From the **Layout** page you
can then sort the library into category folders, with the game folder
holding links to the mods you have active — the library stays whole,
the game only ever sees the set you chose.

## Features

- Card-grid library with each mod's real icon, author, and version —
  categories are the game's own store categories, sizes included
- Browse Mods / Maps / Scripts, Active / Archived, or Favourites,
  with search across the whole library
- Star favourites — they survive moving a mod between folders
- Conflict detection (duplicate mods, Active/Archive overlaps) with
  per-copy removal
- Move mods between Active and Archive, including across drives
- **Library Layout** — one master library sorted into category
  folders; the game folder holds hard links to the active set, so
  nothing is ever duplicated
- **Configure Save** — a step-by-step wizard that builds a mod
  loadout for each savegame: cash and loan, timescale, the mods for
  that save, and an optional AutoDrive config
- **Update checks** against ModHub, GitHub, KingMods, and itch.io —
  one source link per mod, the well your copy actually came from
- **Downloads section** — every download is a visible job with live
  progress, and failures say plainly what happened
- Background update checks via the bundled update helper, with an
  optional Windows schedule (three times a day)
- **Log Reader** — read the game's log in-app, with error and
  warning counts
- Everything runs locally; your library never leaves your PC

One honest limit: a few file hosts (ShareMods is one) guard their
free downloads with a browser check that no app can complete.
Playground detects that, names it, and points you at the browser
download instead of failing silently.

## On the roadmap

- A fuller Settings page
- Radio Stations — manage the game's streaming radio, with a
  companion mod for in-game use
- Linux release
- Game Switching — FS13 / FS15 / FS17 / FS19 / FS22 profiles
  alongside FS25

## Building from source (developers only)

Normal users should use the Releases download above. To build it
yourself you need Rust (stable, MSVC) on Windows:

```bat
cargo build --release
target\release\playground.exe
```

## Project structure

- `src/main.rs` — entry point (GUI, plus a small `cli` diagnostic mode)
- `src/ui/` — the Iced GUI: card grid, sidebar, downloads, log reader,
  save configurator
- `src/mod_manager/` — scanning, modDesc parsing, categories, conflict
  detection, library layout, savegame loadouts
- `src/sources/` — update-source integrations: ModHub, GitHub,
  KingMods, itch.io, and the catalog matching behind them
- `src/bin/` — the standalone update helper (`update.exe`)

## Credits

V1.5.2 © 2026 Farmer Jay.
Created with the Assistance of Albert — Farmer Jay's Muse AI Agent.

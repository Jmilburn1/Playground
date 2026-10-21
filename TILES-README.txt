Farmer Jay v1.1.1 - the tidy-up round (2026-10-18)
==================================================
Everything here is John's live calls from the evening
v1.1.0 shipped. Contents at the ZIP root:
  Cargo.toml            (version 1.1.1)
  src/ui/app.rs
  src/ui/components.rs
  assets/fs25_banner.png   (NEW - his FS25 banner)
Extract over C:\Playground, cargo build --release, run
the exe. Replaces the earlier v1.1.1 handoff ZIPs.

1. Fourth Browse stat tile: DISK USAGE -> AVAILABLE
   UPDATES (the update-database count; "-" when no
   database yet). Disk usage already lives in the
   sidebar; nothing was lost.
2. The reader's "Updates available" line removed from
   the sidebar - one home per number: updates in the
   tiles, disk usage in the sidebar.
3. "Updates" added to the Browse Show: dropdown -
   filters the grid to only mods with an update.
4. Disk usage moved up under the Scripts row, with
   the library block; the empty space falls between
   it and the copyright footer at the bottom.
5. Scroll-clip fix (attempt 1): the Browse column now
   claims its full height explicitly. Left shrink-
   sized, the grid's scrollable measured its viewport
   as if the toolbar wasn't there - fine at rest, but
   once scrolling started, cards slid up over the
   toolbar and tucked under the tiles (his screenshot).
   TEST: scroll a little and stop - the toolbar
   (Show: + search) must stay on screen and the card
   tops must clip BELOW it, never reaching the tiles.
6. Header rebuilt: the four buttons (Detect ModHub
   links, Check for updates, Rescan, Exit) are GONE,
   and so is the "FS Mod Manager" text. The header is
   now his Farming Simulator 25 banner alone (the
   image he supplied, margins trimmed, pre-scaled to
   2x display height so it renders crisp).
7. Detection is part of Rescan now: pressing Rescan
   (sidebar or Settings) runs scan -> icons -> the
   ModHub detection pass automatically, same chain
   the Detect button used to start. A plain launch
   still loads silently with no detection - the
   database design is untouched. Check for updates as
   a button is gone for good; the update database +
   reader replaced it (the per-mod Check in a mod's
   details panel still works).
8. Rescan button sits in the sidebar, above Browse.

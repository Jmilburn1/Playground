PLAYGROUND v1.1.3 - windowed Browse grid + the
Playground title. Extract over C:\Playground (repo
root), build, run.

1. Browsing home no longer hangs. The grid used to
   build and measure every card in the library on
   every frame - at ~2,900 mods, coming back to
   Browse from any other tab froze while ~580 rows
   were rebuilt. Cards are a fixed height, so the
   grid now builds only the rows around the visible
   window (plus a buffer) and pads the rest with
   spacers; the total height - and the scrollbar -
   behave exactly as before. The filter/sort also
   caches its sort keys instead of re-lowercasing
   every title on every comparison. Measured in the
   Linux rig at 2,900 mods (relative numbers): the
   Conflicts -> Browse return went from ~55 seconds
   to ~3.5. What to look for: hop between tabs -
   Browse should come back near-instantly; scroll
   the grid end to end - no blank stretches, no
   jumps; change the Show: filter or type in search
   - the grid jumps back to the top.
2. The window title bar says Playground (John's
   last-minute call that missed the v1.1.2 push by
   a minute - it rides in this release instead).
3. Footer reads V1.1.3.

Files: Cargo.toml (1.1.3), src/ui/app.rs,
src/ui/components.rs, plus the unchanged carriers
from the v1.1.2 pack (icons.rs, manager.rs,
types.rs, conflict.rs, assets/fs25_banner.png).

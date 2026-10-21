Farmer Jay v1.1.1 - scroll-clip fix + icon name fallback
=========================================================

Applies on top of the earlier v1.1.1 handoffs (tiles / banner /
grey nav). This zip carries the full current file set for the
release, so it is safe to extract over any of them.

1. Scroll clip (the big one). Proven root cause in a local
   repro rig: in iced 0.13's renderers, image and mesh
   primitives inside a scrollable are NOT cut by the
   scrollable's clip the way boxes and text are - a partially
   visible card's icon kept painting at its natural position,
   over the toolbar at the top and over the status bar at the
   bottom. Fix: view() now stacks the main area in two layers.
   The tab content sits in the base layer with empty holes
   where the header zone and status bar belong; the banner,
   tiles, toolbar and status bar draw in the top layer (a
   stack child after the first gets its own layer, painted
   over escaped pixels), backed by opaque window-colored
   slabs so nothing shows through their gaps. Verified at
   rest (layout pixel-identical) and at several scroll
   offsets, top and bottom.

2. Icon names. The loader's candidate list never included
   the GIANTS convention names - most mods ship their icon
   as icon_<something>.dds (maps: icon_map.dds). When the
   declared name and the fixed fallbacks all miss, the
   loader now takes any icon_* image in the mod (DDS first,
   then PNG/JPG). Verified with a synthetic mod whose only
   icon is icon_map.dds: placeholder before, picture after.

Files: Cargo.toml (1.1.1), src/ui/app.rs, src/ui/components.rs,
src/mod_manager/icons.rs, assets/fs25_banner.png.

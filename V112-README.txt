Farmer Jay v1.1.2 - icon cache + exact-filename identity
=========================================================

1. Icon cache. The icon pass no longer opens every
   archive and decodes every icon on every launch and
   Rescan. First pass stores each mod's decoded picture
   as a PNG in .config/fs-mod-manager/icon-cache/,
   keyed by the mod file's name + size + mtime. Later
   launches read the ready-made pictures. A mod replaced
   by an update changes size/mtime and re-decodes alone;
   entries for removed mods are swept automatically.
   First launch on this build pays the old full pass one
   last time (filling the cache); later launches are
   the fast ones. (v1.1.1 warm-launch baseline: ~1 min.)

2. Conflicts tell the truth. A mod's identity is now
   its EXACT file name - every character, case
   included - never its modDesc title. The old code
   grouped by title, so different mods sharing a title
   ("Bale Storage" x4) showed as copies of one mod, and
   hardlink twins counted double: 1,055 phantom
   conflicts. Now: same exact file name + size + version
   in both folders is the same file and loads once
   (Active copy wins); same file name but different
   size or version means the copies diverged - both
   load and THAT is reported as a conflict; different
   file names are different mods, whatever the titles
   say. The Conflicts page groups by file name and
   shows each copy's state, version and size.

3. The window title bar now says Playground
   (was FS Mod Manager).

Files: Cargo.toml (1.1.2), src/ui/app.rs,
src/ui/components.rs (unchanged, carried for the full
set), src/mod_manager/icons.rs,
src/mod_manager/manager.rs, src/mod_manager/types.rs,
src/mod_manager/conflict.rs, assets/fs25_banner.png.

# Testing Playground

Playground is built and room-tested by one person (Farmer Jay, on
Windows). Outside testers are how the rough edges on other machines
— and the whole Linux build — get found. Thank you for helping.

## Get it

Download the newest release for your platform:

- **Windows:** `Playground-Windows.zip` — unzip anywhere, run
  `playground.exe`. It's unsigned, so SmartScreen may warn:
  **More info → Run anyway**.
- **Linux:** `Playground-Linux.zip` — unzip, run `./install.sh`,
  then start Playground from your applications menu. No root
  needed; everything installs under your home folder.

## What to test first

1. **First run & folders** — does it find your FS25 profile
   folder? On Linux with the Steam version, it should find the
   profile inside the Proton prefix on its own. If it doesn't,
   that's a report we want.
2. **Rescan** your real library. Counts sane? Icons and
   categories right? A mod's category is the **first** one its
   file lists — if one looks miscategorised, check whether the
   file itself lists another category first.
3. **Library Layout** — try it on a **copy** of your library
   first. The Layout page's Preview lists every move before
   Apply runs.
4. **Update checks** — ModHub and GitHub mods should offer
   installs. KingMods and itch.io mods live under
   **Downloads → Other Links** *by design*: Playground never
   downloads from those two, it opens the mod's page so you can
   grab the file yourself and import it. A newer source version
   is only ever offered — never a downgrade.
5. **Linux only:** self-update. When the next release ships,
   Settings → Maintenance → "Check now" should offer it and the
   app should come back up on the new version by itself.

## Reporting a bug

File it at **github.com/Jmilburn1/Playground/issues** with:

- Playground version (bottom of the sidebar, or Settings → About)
- Your OS and version (e.g. Windows 11 24H2, Fedora 44, Ubuntu 26.04)
- What you did, what you expected, what happened
- For a Layout or Log Reader issue: the relevant part of the
  game's `log.txt` helps a lot

One issue per problem, please — and check the list first in case
it's already known.

## Known limits (not bugs)

- KingMods and itch.io are manual-link sources by design (above).
- Linux: the scheduled catalog fill isn't wired up yet — the
  fill still runs at launch, on Rescan, and from Settings →
  Maintenance. The `update` helper can be run from cron or a
  systemd timer if you want it automatic.
- The mod database ships from ModHub's catalog; mods that only
  exist on other sites show what Playground can work out from
  the file itself, plus any link you paste in.

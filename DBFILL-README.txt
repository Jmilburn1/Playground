Playground — db-fill handoff (Albert, 2026-10-18 night)
=====================================================

WHAT THIS IS
------------
The personal update database, first fill. A new CLI command:

    cli db-fill

One swipe: scan facts -> ModHub catalog join (by file name;
matched rows earn their file id) -> polls of your existing
GitHub / ModHub links -> residue report. Writes:

    C:\Users\jmilburn\.config\fs-mod-manager\updates.json
    C:\Users\jmilburn\.config\fs-mod-manager\updates-summary.txt

Rows are listed by file id; the downgrade block is in the
data (a mod newer than its source is counted as
"ahead of source", never as an update).

db-fill only READS your library. It never moves, downloads,
or changes a mod file. Its only writes are the two files
above.

WHAT'S IN THE ZIP
-----------------
    src\update_db.rs   - the whole feature, one new file
    db-fill.patch      - three small edits to src\main.rs
                         (module line, command arm, help line)
    DBFILL-README.txt  - this file

INSTALL (from /c/Playground, the usual way)
-------------------------------------------
1. Extract the ZIP into /c/Playground (src\update_db.rs
   lands in src\, the patch and this readme at the root).

2. Apply the main.rs patch:

       git apply db-fill.patch

   If git apply ever refuses, the three edits are tiny and
   listed at the bottom of this file — paste them by hand.

3. Build:

       cargo build --release

TONIGHT'S SANITY RUN (optional, 2 minutes)
-----------------------------------------
Run it the same way you ran catalog-build:

    cli db-fill

Expect it to scan, join, poll the linked mods, and finish
with the summary counts. The full detail is in
updates.json; the headline numbers are in
updates-summary.txt.

TOMORROW'S SCHEDULED RUN
------------------------
Register this in an admin cmd or PowerShell window
(fire time = 30 minutes before you leave for work —
confirm the clock time with Albert tonight):

    schtasks /create /tn "Playground DB Fill" /tr "\"C:\Playground\target\release\fs-mod-manager.exe\" cli db-fill" /sc once /sd 10/18/2026 /st HH:MM /f

The release exe has no console, so a scheduled run prints
nothing anywhere — its report is updates-summary.txt,
waiting for you when you get home.

SCOPE — WHAT V1 DOES AND DOESN'T DO
-----------------------------------
- ModHub: answered from the local catalog (no crawling),
  plus one page fetch for any mod you hand-linked that the
  catalog doesn't carry yet.
- GitHub: your existing links, polled via the API.
- itch.io / KingMods: not yet — those links have nowhere
  to live until the Other Site box is built, so those mods
  land in the residue ("no source") for now.
- The fill does NOT write detected links into config.toml.
  updates.json is the record; GUI wiring comes later.
- Nothing here applies updates. This only fills the
  database — the review screen spends it later.

THE THREE PATCH EDITS (fallback, if git apply refuses)
------------------------------------------------------
1. In src\main.rs, after the line  mod sources;  add:

       mod update_db;

2. In run_cli, immediately before  Some("update") => {  add:

        Some("db-fill") => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async { crate::update_db::fill(mod_manager).await })?;
        }

3. In print_help, after the catalog-build line, add:

    println!("  cli db-fill - Fill the personal update database (updates.json): scan + catalog join + link polls, in one swipe");

Playground — Complete Walk (--full) + Released dates
Built 2026-10-18 (Albert) — based on your v1.0.3 + db-fill tree.
cargo check PASSED (16 pre-existing warnings, no new ones).
cargo test 11/11 PASSED (was 10 — new test: pre-Released catalogs still load).
Live smoke test PASSED on this machine: page 0 read (27 ids), 4 mod
pages fetched, all 4 records written WITH Released dates, report written
and correctly flagged that capped test walk as incomplete.

FILES IN THIS ZIP (extract replaces these in C:\Playground)
  src/main.rs            — catalog-build gains --full + checkpoints + report
  src/sources/mod_hub.rs — parser reads the "Released" table field;
                           CatalogEntry gains `released` (old catalogs
                           without the field still load, date empty)

WHAT --full DOES
  cli catalog-build <catalog.json> --full
  - Walks EVERY ModHub FS25 listing page to the very end. No early
    stop at the first all-known page (the normal walk stops there).
  - Fetches EVERY listed mod's page (four at a time, politely) and
    records all four tags — mod id, download link, filename,
    Released date — plus version.
  - Checkpoints the catalog file every 500 fetched pages, so a
    dropped connection or a closed laptop does NOT lose the run.
    Re-running the same command resumes from the merged records.
  - At the end prints AND writes a complete-list report beside the
    catalog file (catalog-full-report.txt):
      * mods on ModHub (ids seen in the walk)
      * new to the catalog in this walk
      * records now carrying a Released date (x of total)
      * walked mods still missing a record/date (fetch failures)
      * catalogued ids the walk did NOT see (delisted candidates —
        their records are KEPT, never auto-deleted)
      * the three id lists themselves
  - Normal (non-full) catalog-build behavior is unchanged.

BEFORE THE REAL RUN — BACK UP THE KNOWN-GOOD CATALOG
  cp /c/Playground/catalog.json /c/Playground/catalog-pre-fullwalk.json
  (The walk merges into the same file. Checkpoints keep it valid,
  but the Oct 17 seed is proven — keep a copy until the report
  says the walk reached the end and dates are filled.)

RUN IT (Git Bash)
  unzip -o /c/Users/jmilburn/Downloads/playground-fullwalk-2026-10-18.zip -d /c/Playground
  cd /c/Playground && cargo build --release
  ./target/release/fs-mod-manager.exe cli catalog-build 'C:/Playground/catalog.json' --full

  Do NOT add --pages or --max to the real run (those were only for
  my capped smoke test — they make the walk stop early and the
  report will say so).
  Runtime is an ESTIMATE: the Oct 17 seed fetched ~6,872 pages at
  this same pace in ~55 minutes; expect roughly an hour. ModHub
  has also added mods since Friday, so the seen count will likely
  land above 6,872.

AFTER A GOOD WALK
  - Read C:\Playground\catalog-full-report.txt — "Walk reached the
    end of the listings: yes" is the success line.
  - Publishing is your git move, same as the seed: the updated
    catalog.json replaces the one in C:\Playground-catalog, commit,
    push. Clients then pick up Released dates on their next fetch.

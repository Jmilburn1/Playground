Farmer Jay — v1.1.0: the update-database READER
Handoff 2026-10-18 (Albert). Replaces 3 files:
  Cargo.toml          (version 1.0.3 -> 1.1.0)
  src/ui/app.rs       (the reader + sidebar footer V1.1.0)
  src/update_db.rs    (reader support: load_default,
                       is_changed, summarize + tests)

WHAT IT DOES
The GUI now READS the personal update database
(C:\Users\<you>\.config\fs-mod-manager\updates.json) that
db-fill maintains. Silently, at every library load, with
NO network. Rows are judged against the versions actually
on disk, so you see:

- "Update" pills on cards the moment the app opens —
  no Check click, no crawl.
- Status bar: "... Update database: N updates available
  (filled <date, 12-hour time>)." Sidebar shows the same.
- Details panel, new "Update database:" section per mod:
  source (ModHub mod id / GitHub repo / none), installed
  -> latest, and the direction. Rows whose latest changed
  at the last fill say so.
- Install buttons work straight off the database row —
  including the ModHub mods the catalog join matched that
  were never hand-linked.
- Downgrade-blocked rows (installed NEWER than source)
  get NO pill and NO button, ever. The details panel
  explains the block instead.
- A fill that lands while the app is open is picked up
  on the next Rescan. Live check results from the same
  session are never overwritten by the database re-apply.
- The "Check for updates" button stays for now as the
  manual escape hatch.

ACCEPTANCE (your library, your database)
Your updates.json was filled Sun Oct 18, 11:31 AM
(149 updates available, 8 downgrade-blocked). Launch the
built exe and expect: pills already on, status line
ending "... 149 updates available (filled Oct 18,
11:31 AM)", footer reading V1.1.0. Open one of the 8
ahead mods: details show the blocked explanation and
no install button.

Verified in the sandbox: cargo check clean (16
pre-existing warnings), cargo test 14/14 — 3 new tests
cover the reader's derivation (update vs up-to-date vs
ahead-blocked), the changed-at-last-fill delta, and
summarizing rows against live installed versions.

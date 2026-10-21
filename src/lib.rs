//! Playground — FS Mod Manager. The library holds the
//! whole application (manager, sources, the update
//! database, the CLI verbs, the GUI); the binaries are
//! thin entry points over it:
//!
//! - `fs-mod-manager` (src/main.rs) — the app: GUI, or the
//!   full CLI behind `cli`.
//! - `update` (src/bin/update.rs) — the isolated update
//!   program (owner, 2026-10-19): the fill and detect
//!   verbs as a standalone executable the scheduler can
//!   fire unattended, so the app's update work — the only
//!   scheduled network work — never has to open the GUI,
//!   and a crash or throttling halt in it can never take
//!   the app down.

pub mod cli;
pub mod config;
pub mod mod_manager;
pub mod scanner;
pub mod sources;
pub mod ui;
pub mod update_db;

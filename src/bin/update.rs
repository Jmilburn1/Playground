//! update.exe — the isolated update program (owner,
//! 2026-10-19): "the update is not a file, it's the call
//! to arms — the script that populates the catalogs."
//!
//! This binary is the network-talking half of Playground
//! as its own executable, built to be fired by the Task
//! Scheduler ~3x/day (see tools/register-update-schedule.ps1)
//! and to be spawnable by the app on demand. It shares the
//! app's library — the same fill, the same detect, the
//! same stores — so behavior can never drift between the
//! two; a crash or a GIANTS throttling halt here can never
//! take the GUI down.
//!
//! Usage:
//!   update                  — run the fill (the update)
//!   update update           — same, explicitly
//!   update db-fill          — deprecated alias of the fill
//!   update detect-modhub [limit] — run ModHub detection
//!   update help             — this text
//!
//! Exit status is nonzero when a pass fails — including a
//! throttling tripwire halt — so schedulers and scripts
//! can tell a halted pass from a clean one.

use playground::config::Config;
use playground::mod_manager::ModManager;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("help") | Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some("detect-modhub") => run(&args),
        // The fill: bare, its own name, and the deprecated
        // alias all land on the one implementation.
        None | Some("update") => run(&["update".to_string()]),
        Some("db-fill") => {
            eprintln!(
                "note: 'db-fill' is deprecated — the fill verb is now 'update' (running it)."
            );
            run(&["update".to_string()])
        }
        Some(other) => {
            eprintln!(
                "update: unknown command '{}' — this program runs the fill and ModHub detection.",
                other
            );
            print_usage();
            std::process::exit(2);
        }
    }
}

fn run(cli_args: &[String]) -> anyhow::Result<()> {
    let config = Config::load()?;
    let mut mod_manager = ModManager::new(config)?;
    playground::cli::run_cli(&mut mod_manager, cli_args)
}

fn print_usage() {
    println!("update — Playground's isolated update program");
    println!("Usage:");
    println!("  update                    Run the update fill (scan + catalog join + link polls)");
    println!("  update update             Same, explicitly");
    println!("  update db-fill            Deprecated alias of the fill");
    println!("  update detect-modhub [N]  Auto-link mods to ModHub (optionally only the first N)");
    println!("  update help               This text");
}

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
//!   update apply-app-update --staging <dir> --target <playground exe> --pid <gui pid>
//!                           — the app self-update's swap hand:
//!                           the GUI stages a verified new
//!                           release, spawns this verb, and
//!                           exits; this process waits for
//!                           it, swaps the binaries, and
//!                           relaunches (see app_update.rs)
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
        Some("apply-app-update") => run_apply_app_update(&args[1..]),
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

/// update.exe's `apply-app-update` verb: the GUI has
/// downloaded and verified a new Playground release into
/// a staging folder and is exiting; this process waits
/// for it, swaps the binaries, and relaunches. It loads
/// no config and touches no catalog — the one job is the
/// swap (playground::app_update::apply_app_update), and
/// its log lands in the staging root's app-update.log.
fn run_apply_app_update(args: &[String]) -> anyhow::Result<()> {
    let mut staging: Option<std::path::PathBuf> = None;
    let mut target: Option<std::path::PathBuf> = None;
    let mut log_arg: Option<std::path::PathBuf> = None;
    let mut pid: u32 = 0;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--staging" => staging = it.next().map(std::path::PathBuf::from),
            "--target" => target = it.next().map(std::path::PathBuf::from),
            "--log" => log_arg = it.next().map(std::path::PathBuf::from),
            "--pid" => pid = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            other => {
                eprintln!("update apply-app-update: unknown argument '{}'", other);
                print_usage();
                std::process::exit(2);
            }
        }
    }
    let (Some(staging), Some(target)) = (staging, target) else {
        eprintln!("update apply-app-update: --staging <dir> and --target <playground exe> are required");
        print_usage();
        std::process::exit(2);
    };
    // The GUI passes --log explicitly (the staging root's
    // app-update.log); without it, derive the same place
    // from the staging folder's shape as a fallback.
    let log_path = log_arg.or_else(|| {
        staging
            .parent()
            .and_then(|p| p.parent())
            .map(|root| root.join("app-update.log"))
    });
    playground::app_update::apply_app_update(&staging, &target, pid, log_path.as_deref())
}

fn print_usage() {
    println!("update — Playground's isolated update program");
    println!("Usage:");
    println!("  update                    Run the update fill (scan + catalog join + link polls)");
    println!("  update update             Same, explicitly");
    println!("  update db-fill            Deprecated alias of the fill");
    println!("  update detect-modhub [N]  Auto-link mods to ModHub (optionally only the first N)");
    println!("  update apply-app-update --staging <dir> --target <exe> --pid <pid>");
    println!("                            Apply a staged app self-update (spawned by the app)");
    println!("  update help               This text");
}

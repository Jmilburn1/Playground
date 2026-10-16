mod mod_manager;
mod scanner;
mod sources;
mod config;
mod ui;

use config::Config;
use mod_manager::ModManager;
use std::env;

fn main() -> anyhow::Result<()> {
    // Setup logging
    tracing_subscriber::fmt::init();

    // Load configuration
    let config = Config::load()?;
    
    // Initialize mod manager
    let mut mod_manager = ModManager::new(config)?;

    // Check if we should use CLI mode
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && args[1] == "cli" {
        return run_cli(&mut mod_manager, &args[2..]);
    }

    // Start GUI
    ui::run(mod_manager)
}

fn run_cli(mod_manager: &mut ModManager, args: &[String]) -> anyhow::Result<()> {
    match args.get(0).map(|s| s.as_str()) {
        Some("scan") => {
            mod_manager.scan_all()?;
            let mods = mod_manager.get_mods();
            println!("Found {} mods:", mods.len());
            for m in mods {
                println!("- {} ({:?})", m.name, m.state);
            }
        }
        Some("list") => {
            let mods = mod_manager.get_mods();
            println!("Mods in archive: {}", mod_manager.get_mods_by_state(mod_manager::ModState::Archived).len());
            println!("Mods active: {}", mod_manager.get_mods_by_state(mod_manager::ModState::Active).len());
            for m in mods {
                println!("- {} ({:?})", m.name, m.state);
            }
        }
        Some("search") => {
            if let Some(query) = args.get(1) {
                let results = mod_manager.search_mods(query);
                println!("Found {} matching mods:", results.len());
                for m in results {
                    println!("- {}", m.name);
                }
            }
        }
        Some("move") => {
            if let Some(mod_id) = args.get(1) {
                if let Some(target) = args.get(2) {
                    let target_path = std::path::PathBuf::from(target);
                    mod_manager.move_between_directories(mod_id, &target_path)?;
                    println!("Moved mod to {:?}", target_path);
                }
            }
        }
        Some("conflicts") => {
            let mods = mod_manager.get_mods();
            let mods_vec: Vec<mod_manager::Mod> = mods.into_iter().cloned().collect();
            let conflicts = mod_manager::conflict::ConflictDetector::detect_conflicts(&mods_vec);
            println!("Found {} conflicts:", conflicts.len());
            for c in conflicts {
                println!("- {} vs {}: {:?}", c.mod1, c.mod2, c.conflict_type);
            }
        }
        Some("help") | _ => {
            print_help();
        }
    }

    Ok(())
}

fn print_help() {
    println!("FS Mod Manager - Farming Simulator Mod Manager");
    println!("Usage: fs-mod-manager [command] [args]");
    println!("Commands:");
    println!("  cli scan - Scan all directories for mods");
    println!("  cli list - List all mods");
    println!("  cli search <query> - Search mods");
    println!("  cli move <mod_id> <target_dir> - Move mod to directory");
    println!("  cli conflicts - Check for mod conflicts");
    println!("  cli help - Show this help");
    println!("  (no args) - Launch GUI");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_load() {
        let config = Config::load();
        assert!(config.is_ok());
    }
}
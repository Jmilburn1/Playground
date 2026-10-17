// Release builds on Windows use the GUI subsystem, so launching the
// finished .exe does not leave a console window behind. Debug builds keep
// the console for diagnostics and CLI work.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

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

    // Check if we should use CLI mode
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && args[1] == "cli" {
        let mut mod_manager = ModManager::new(config)?;
        return run_cli(&mut mod_manager, &args[2..]);
    }

    // Start GUI — unscanned: the app loads the library in a background
    // task so the window opens immediately.
    let mod_manager = ModManager::new_unscanned(config)?;
    ui::run(mod_manager)
}

fn format_mod_line(m: &mod_manager::Mod) -> String {
    let short_id = &m.id[..m.id.len().min(8)];
    match &m.author {
        Some(author) => format!(
            "{} v{} by {} ({:?}) [{}]",
            m.name, m.version, author, m.state, short_id
        ),
        None => format!("{} v{} ({:?}) [{}]", m.name, m.version, m.state, short_id),
    }
}

fn run_cli(mod_manager: &mut ModManager, args: &[String]) -> anyhow::Result<()> {
    match args.get(0).map(|s| s.as_str()) {
        Some("scan") => {
            mod_manager.scan_all()?;
            let mods = mod_manager.get_mods();
            println!("Found {} mods:", mods.len());
            for m in mods {
                println!("- {}", format_mod_line(m));
            }
        }
        Some("list") => {
            let mods = mod_manager.get_mods();
            println!("Mods in archive: {}", mod_manager.get_mods_by_state(mod_manager::ModState::Archived).len());
            println!("Mods active: {}", mod_manager.get_mods_by_state(mod_manager::ModState::Active).len());
            for m in mods {
                println!("- {}", format_mod_line(m));
            }
        }
        Some("search") => {
            if let Some(query) = args.get(1) {
                let results = mod_manager.search_mods(query);
                println!("Found {} matching mods:", results.len());
                for m in results {
                    println!("- {}", format_mod_line(m));
                }
            }
        }
        Some("move") => {
            if let Some(query) = args.get(1) {
                if let Some(target) = args.get(2) {
                    // Accept a full ID, the short ID shown by scan/list, or
                    // the mod's name.
                    let found = mod_manager
                        .get_mods()
                        .into_iter()
                        .find(|m| {
                            m.id == *query
                                || m.id.starts_with(query.as_str())
                                || m.name.eq_ignore_ascii_case(query)
                        })
                        .map(|m| m.id.clone());
                    match found {
                        Some(id) => {
                            let target_path = std::path::PathBuf::from(target);
                            mod_manager.move_between_directories(&id, &target_path)?;
                            println!("Moved mod to {:?}", target_path);
                        }
                        None => println!("No mod matching '{}'", query),
                    }
                }
            }
        }
        Some("icons") => {
            for m in mod_manager.get_mods() {
                match mod_manager::icons::load_icon(&m.path, m.icon_filename.as_deref()) {
                    Some(mod_manager::icons::LoadedIcon::Rgba { width, height, .. }) => {
                        println!("- {}: DDS icon decoded ({}x{})", m.name, width, height)
                    }
                    Some(mod_manager::icons::LoadedIcon::Encoded(bytes)) => {
                        println!("- {}: image icon ({} bytes)", m.name, bytes.len())
                    }
                    None => println!("- {}: no icon found", m.name),
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
        Some("categories") => {
            let mut counts: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::new();
            for m in mod_manager.get_mods() {
                println!("- {}: {}", m.name, m.categories.join(", "));
                for c in &m.categories {
                    *counts.entry(c.as_str()).or_insert(0) += 1;
                }
            }
            let mut summary: Vec<(&str, usize)> = counts.into_iter().collect();
            summary.sort_by(|a, b| b.1.cmp(&a.1));
            println!("Category counts:");
            for (cat, n) in summary {
                println!("  {}: {}", cat, n);
            }
        }
        Some("saves") => {
            let slots = mod_manager.save_slots();
            if slots.is_empty() {
                println!("No save slots found (is the active folder a game's mods folder?).");
            }
            for s in &slots {
                let loadout = mod_manager.save_loadout(&s.dir_name);
                println!(
                    "- Slot {} ({}): map {}, saved {}, played {}, money {} — loadout: {} mods",
                    s.slot,
                    s.dir_name,
                    s.map.as_deref().unwrap_or("?"),
                    s.saved_at.as_deref().unwrap_or("?"),
                    s.play_time_display().as_deref().unwrap_or("?"),
                    s.money
                        .map(|m| format!("${}", m))
                        .unwrap_or_else(|| "?".to_string()),
                    if loadout.is_empty() {
                        "none saved".to_string()
                    } else {
                        format!("{} mods", loadout.len())
                    },
                );
            }
        }
        Some("audit") => {
            if let Some(root) = args.get(1) {
                audit_tree(mod_manager, std::path::Path::new(root));
            } else {
                println!("Usage: cli audit <folder>");
                println!("Lists the mods inside <folder> (searched recursively) that");
                println!("are NOT in the library, so a duplicate folder tree can be");
                println!("checked before anything in it is deleted.");
            }
        }
        Some("help") | _ => {
            print_help();
        }
    }

    Ok(())
}

/// Human-readable byte count for CLI output.
fn fmt_bytes(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, units[i])
    } else {
        format!("{:.1} {}", v, units[i])
    }
}

/// Recursive folder audit (for John's library consolidation):
/// walk a folder tree, treat every .zip and every folder holding
/// a modDesc.xml as one mod, and diff their file stems against
/// the scanned library. Prints what the tree duplicates (safe to
/// delete once the uniques are rescued) and what exists ONLY in
/// the tree. Read-only — nothing is moved or deleted.
fn audit_tree(mod_manager: &mut ModManager, root: &std::path::Path) {
    use std::collections::HashSet;
    if !root.is_dir() {
        println!("Not a folder: {:?}", root);
        return;
    }
    let library: HashSet<String> = mod_manager
        .get_mods()
        .into_iter()
        .map(|m| mod_stem(&m.path))
        .collect();

    fn mod_stem(path: &std::path::Path) -> String {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        name.strip_suffix(".zip")
            .map(|s| s.to_string())
            .unwrap_or(name)
    }

    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.join("modDesc.xml").exists() {
                    out.push(path); // a folder mod — do not descend
                } else {
                    collect(&path, out);
                }
            } else if path
                .extension()
                .map_or(false, |e| e.eq_ignore_ascii_case("zip"))
            {
                out.push(path);
            }
        }
    }

    let mut candidates = Vec::new();
    if root.join("modDesc.xml").exists() {
        candidates.push(root.to_path_buf());
    } else {
        collect(root, &mut candidates);
    }
    candidates.sort();

    let mut dup_count = 0usize;
    let mut dup_bytes = 0u64;
    let mut unique: Vec<(String, String, u64, std::path::PathBuf)> = Vec::new();
    for path in &candidates {
        let (label, version, size) = match mod_manager::Mod::new(path.clone()) {
            Ok(m) => (m.name.clone(), m.version.clone(), m.size),
            Err(_) => (
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                "?".to_string(),
                std::fs::metadata(path).map(|md| md.len()).unwrap_or(0),
            ),
        };
        if library.contains(&mod_stem(path)) {
            dup_count += 1;
            dup_bytes += size;
        } else {
            unique.push((label, version, size, path.clone()));
        }
    }
    let unique_bytes: u64 = unique.iter().map(|u| u.2).sum();

    println!("Audit of {:?}", root);
    println!("Library holds {} mods.", library.len());
    println!("Found {} mods in the tree:", candidates.len());
    println!(
        "- {} already in the library ({}) — duplicates",
        dup_count,
        fmt_bytes(dup_bytes)
    );
    println!(
        "- {} ONLY in this tree ({}) — rescue these before deleting the tree",
        unique.len(),
        fmt_bytes(unique_bytes)
    );
    for (label, version, size, path) in &unique {
        println!(
            "  UNIQUE: {} v{} ({}) at {:?}",
            label,
            version,
            fmt_bytes(*size),
            path
        );
    }
}

fn print_help() {
    println!("FS Mod Manager - Farming Simulator Mod Manager");
    println!("Usage: fs-mod-manager [command] [args]");
    println!("Commands:");
    println!("  cli scan - Scan all directories for mods");
    println!("  cli list - List all mods");
    println!("  cli search <query> - Search mods");
    println!("  cli move <id|name> <target_dir> - Move mod to directory");
    println!("  cli conflicts - Check for mod conflicts");
    println!("  cli icons - Show icon load status for each mod");
    println!("  cli categories - Show the detected category of each mod");
    println!("  cli saves - List savegame slots and their saved loadouts");
    println!("  cli audit <folder> - List mods in a folder tree that are NOT in the library");
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
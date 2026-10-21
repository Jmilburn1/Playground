// Release builds on Windows use the GUI subsystem, so launching the
// finished .exe does not leave a console window behind. Debug builds keep
// the console for diagnostics and CLI work.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use playground::config::Config;
use playground::mod_manager::ModManager;

fn main() -> anyhow::Result<()> {
    // Setup logging
    tracing_subscriber::fmt::init();

    // Load configuration
    let config = Config::load()?;

    // Check if we should use CLI mode
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "cli" {
        let mut mod_manager = ModManager::new(config)?;
        return playground::cli::run_cli(&mut mod_manager, &args[2..]);
    }

    // Start GUI — unscanned: the app loads the library in a background
    // task so the window opens immediately.
    let mod_manager = ModManager::new_unscanned(config)?;
    playground::ui::run(mod_manager)
}

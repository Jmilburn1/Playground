pub mod types;
pub mod manager;
pub mod library_db;
pub mod layout;
pub mod layout_exec;
pub mod conflict;
pub mod moddesc;
pub mod icons;
pub mod categories;
pub mod saves;
pub mod savegame;
pub mod game_files;
pub mod keybindings;

use crate::config::Config;

pub use manager::{ModManager, PageSettings};
pub use types::{Mod, ModState};
pub use conflict::{Conflict, ConflictType};

pub fn init(config: Config) -> anyhow::Result<ModManager> {
    ModManager::new(config)
}
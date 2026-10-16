pub mod types;
pub mod manager;
pub mod conflict;
pub mod moddesc;

use crate::config::Config;

pub use manager::ModManager;
pub use types::{Mod, ModState};
pub use conflict::{Conflict, ConflictType};

pub fn init(config: Config) -> anyhow::Result<ModManager> {
    ModManager::new(config)
}
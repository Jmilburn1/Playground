mod app;
mod components;

use crate::mod_manager::ModManager;

pub fn run(mod_manager: ModManager) -> anyhow::Result<()> {
    app::run(mod_manager)
}

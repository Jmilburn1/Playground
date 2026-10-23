mod app;
mod components;
pub(crate) mod keycaps;
pub(crate) mod theme;

use crate::mod_manager::ModManager;

pub fn run(mod_manager: ModManager) -> anyhow::Result<()> {
    app::run(mod_manager)
}

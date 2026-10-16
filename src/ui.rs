mod app;
mod components;

use crate::mod_manager::ModManager;

pub fn run(mut mod_manager: ModManager) -> anyhow::Result<()> {
    app::App::run(&mut mod_manager)
}
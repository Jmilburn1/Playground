use crate::mod_manager::ModManager;
use iced::alignment::{Alignment, Horizontal, Vertical};
use iced::widget::{button, column, container, row, scrollable, text, text_input, Column, Row};
use iced::{Element, Length, Task};

use crate::ui::components::Message;

pub struct App<'a> {
    mod_manager: &'a ModManager,
    current_tab: usize,
    search_query: String,
    selected_mod: Option<String>,
}

impl<'a> App<'a> {
    pub fn new(mod_manager: &'a ModManager) -> Self {
        Self {
            mod_manager,
            current_tab: 0,
            search_query: String::new(),
            selected_mod: None,
        }
    }

    pub fn run(mod_manager: &'a mut ModManager) -> anyhow::Result<()> {
        let app = Self::new(mod_manager);
        app.run_console()
    }

    fn run_console(&self) -> anyhow::Result<()> {
        println!("FS Mod Manager GUI (Console Mode)");
        println!("===================================");
        self.draw()
    }

    fn draw(&self) -> anyhow::Result<()> {
        println!("\n=== FS Mod Manager ===");
        println!("Tabs: [Browse] [Manage] [Settings] [Exit]");
        println!("\nSearch: {}", self.search_query);
        println!("\nMods Found: {}", self.mod_manager.get_mods().len());
        
        let mods = self.mod_manager.get_mods();
        for m in mods {
            println!("- {} ({:?})", m.name, m.state);
        }
        
        println!("\nArchive: {}", self.mod_manager.get_archive_dir().display());
        println!("Active: {}", self.mod_manager.get_active_dir().display());
        
        Ok(())
    }
}
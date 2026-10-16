use crate::mod_manager::conflict::{Conflict, ConflictDetector};
use crate::mod_manager::types::{Mod, ModState};
use crate::mod_manager::ModManager;
use crate::ui::components::{format_size, mod_card, Message, Tab};
use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Element, Length, Size, Subscription, Task, Theme};

const CARD_W: f32 = 212.0;
const CARD_H: f32 = 178.0;
const GRID_SPACING: f32 = 12.0;
const DETAILS_W: f32 = 344.0;

pub struct App {
    mod_manager: ModManager,
    current_tab: Tab,
    search_query: String,
    favorites_only: bool,
    selected_mod: Option<String>,
    pending_remove: Option<String>,
    conflicts: Vec<Conflict>,
    status: String,
    window_size: Size,
}

impl App {
    pub fn new(mod_manager: ModManager) -> Self {
        let count = mod_manager.get_mods().len();
        let mut app = Self {
            mod_manager,
            current_tab: Tab::Browse,
            search_query: String::new(),
            favorites_only: false,
            selected_mod: None,
            pending_remove: None,
            conflicts: Vec::new(),
            status: format!("Ready — {} mods loaded.", count),
            window_size: Size::new(1280.0, 800.0),
        };
        app.recompute_conflicts();
        app
    }

    fn recompute_conflicts(&mut self) {
        let mods: Vec<Mod> = self.mod_manager.get_mods().into_iter().cloned().collect();
        self.conflicts = ConflictDetector::detect_conflicts(&mods);
    }

    fn filtered_mods(&self) -> Vec<&Mod> {
        let query = self.search_query.trim().to_lowercase();
        let mut mods: Vec<&Mod> = self
            .mod_manager
            .get_mods()
            .into_iter()
            .filter(|m| {
                (query.is_empty()
                    || m.name.to_lowercase().contains(&query)
                    || m.author
                        .as_ref()
                        .map_or(false, |a| a.to_lowercase().contains(&query)))
                    && (!self.favorites_only
                        || self.mod_manager.is_favorite(&m.favorite_key()))
            })
            .collect();
        mods.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        mods
    }

    fn mod_has_conflict(&self, name: &str) -> bool {
        self.conflicts
            .iter()
            .any(|c| c.mod1 == name || c.mod2 == name)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Any action other than a confirming second click disarms removal.
        if !matches!(message, Message::RemoveMod(_)) {
            self.pending_remove = None;
        }

        match message {
            Message::Scan => match self.mod_manager.scan_all() {
                Ok(()) => {
                    self.recompute_conflicts();
                    if let Some(id) = &self.selected_mod {
                        if self.mod_manager.get_mod(id).is_none() {
                            self.selected_mod = None;
                        }
                    }
                    self.status = format!(
                        "Rescanned — {} mods, {} conflicts.",
                        self.mod_manager.get_mods().len(),
                        self.conflicts.len()
                    );
                }
                Err(e) => self.status = format!("Scan failed: {}", e),
            },
            Message::SearchChanged(query) => self.search_query = query,
            Message::TabSelected(tab) => self.current_tab = tab,
            Message::SelectMod(id) => self.selected_mod = Some(id),
            Message::CloseDetails => self.selected_mod = None,
            Message::ToggleFavoritesOnly => {
                self.favorites_only = !self.favorites_only;
            }
            Message::ToggleFavorite(id) => {
                if let Some(m) = self.mod_manager.get_mod(&id).cloned() {
                    let key = m.favorite_key();
                    match self.mod_manager.toggle_favorite(&key) {
                        Ok(true) => self.status = format!("★ {} added to favorites.", m.name),
                        Ok(false) => {
                            self.status = format!("{} removed from favorites.", m.name)
                        }
                        Err(e) => self.status = format!("Favorite failed: {}", e),
                    }
                }
            }
            Message::MoveToArchive(id) => {
                let name = self.mod_manager.get_mod(&id).map(|m| m.name.clone());
                match self.mod_manager.move_to_archive(&id) {
                    Ok(()) => {
                        self.status = format!("Moved {} to archive.", name.unwrap_or_default())
                    }
                    Err(e) => self.status = format!("Move failed: {}", e),
                }
            }
            Message::MoveToActive(id) => {
                let name = self.mod_manager.get_mod(&id).map(|m| m.name.clone());
                match self.mod_manager.move_to_active(&id) {
                    Ok(()) => {
                        self.status = format!("Moved {} to active.", name.unwrap_or_default())
                    }
                    Err(e) => self.status = format!("Move failed: {}", e),
                }
            }
            Message::RemoveMod(id) => {
                if self.pending_remove.as_deref() == Some(id.as_str()) {
                    let name = self.mod_manager.get_mod(&id).map(|m| m.name.clone());
                    match self.mod_manager.remove_mod(&id) {
                        Ok(()) => {
                            if self.selected_mod.as_deref() == Some(id.as_str()) {
                                self.selected_mod = None;
                            }
                            self.pending_remove = None;
                            self.recompute_conflicts();
                            self.status = format!(
                                "Deleted {}.",
                                name.unwrap_or_else(|| "mod".to_string())
                            );
                        }
                        Err(e) => self.status = format!("Delete failed: {}", e),
                    }
                } else {
                    let name = self
                        .mod_manager
                        .get_mod(&id)
                        .map(|m| m.name.clone())
                        .unwrap_or_else(|| "this mod".to_string());
                    self.pending_remove = Some(id);
                    self.status = format!(
                        "Click Remove again to permanently delete {}.",
                        name
                    );
                }
            }
            Message::WindowResized(size) => self.window_size = size,
            Message::Exit => return iced::exit(),
        }

        Task::none()
    }

    fn tab_button(&self, label: String, tab: Tab) -> Element<'_, Message> {
        let style = if self.current_tab == tab {
            button::primary
        } else {
            button::secondary
        };
        button(text(label))
            .style(style)
            .on_press(Message::TabSelected(tab))
            .into()
    }

    fn browse_view(&self) -> Element<'_, Message> {
        let mods = self.filtered_mods();

        let content: Element<'_, Message> = if mods.is_empty() {
            container(
                text(if self.mod_manager.get_mods().is_empty() {
                    "No mods found. Put mod zips or folders into your active or archive folder, then Rescan."
                } else {
                    "No mods match the current search/filter."
                })
                .size(15),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
        } else {
            let pane_w = if self.selected_mod.is_some() {
                DETAILS_W + GRID_SPACING
            } else {
                0.0
            };
            let avail = (self.window_size.width - pane_w - 64.0).max(CARD_W);
            let cols = (((avail + GRID_SPACING) / (CARD_W + GRID_SPACING)).floor() as usize)
                .max(1);

            let mut grid = column![].spacing(GRID_SPACING);
            for chunk in mods.chunks(cols) {
                let mut r = row![].spacing(GRID_SPACING);
                for m in chunk {
                    let fav = self.mod_manager.is_favorite(&m.favorite_key());
                    let selected = self.selected_mod.as_deref() == Some(m.id.as_str());
                    let conflict = self.mod_has_conflict(&m.name);
                    r = r.push(mod_card(m, fav, selected, conflict, CARD_W, CARD_H));
                }
                grid = grid.push(r);
            }
            scrollable(grid)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };

        if let Some(details) = self.details_view() {
            row![content, details].spacing(GRID_SPACING).into()
        } else {
            content
        }
    }

    fn details_view(&self) -> Option<Element<'_, Message>> {
        let id = self.selected_mod.as_ref()?;
        let m = self.mod_manager.get_mod(id)?;

        let fav = self.mod_manager.is_favorite(&m.favorite_key());
        let fav_label = if fav {
            "★ Favorited"
        } else {
            "☆ Add to favorites"
        };

        let mut col = column![
            row![
                text(&m.name).size(20),
                Space::with_width(Length::Fill),
                button(text("✕").size(14)).on_press(Message::CloseDetails),
            ],
            text(format!("Version {}", m.version)).size(14),
        ]
        .spacing(8);

        if let Some(author) = &m.author {
            col = col.push(text(format!("by {}", author)).size(14));
        }

        col = col.push(
            text(m.description
                .as_deref()
                .unwrap_or("No description available."))
            .size(13),
        );

        col = col.push(text(format!("State: {:?}", m.state)).size(13));
        col = col.push(text(format!("Type: {:?}", m.mod_type)).size(13));
        col = col.push(text(format!("Size: {}", format_size(m.size))).size(13));
        col = col.push(text(format!("Folder: {}", m.path.display())).size(12));
        if let Some(icon) = &m.icon_filename {
            col = col.push(text(format!("Icon: {}", icon)).size(12));
        }
        if !m.dependencies.is_empty() {
            col = col.push(text(format!("Depends on: {}", m.dependencies.join(", "))).size(12));
        }

        let my_conflicts: Vec<&Conflict> = self
            .conflicts
            .iter()
            .filter(|c| c.mod1 == m.name || c.mod2 == m.name)
            .collect();
        if !my_conflicts.is_empty() {
            col = col.push(text("Conflicts:").size(13));
            for c in my_conflicts {
                let other = if c.mod1 == m.name { &c.mod2 } else { &c.mod1 };
                col = col.push(
                    text(format!("⚠ {} — {:?}", other, c.conflict_type)).size(12),
                );
            }
        }

        col = col.push(Space::with_height(8));
        col = col.push(
            button(text(fav_label))
                .width(Length::Fill)
                .on_press(Message::ToggleFavorite(m.id.clone())),
        );
        if m.state == ModState::Active {
            col = col.push(
                button(text("Move to Archive"))
                    .width(Length::Fill)
                    .on_press(Message::MoveToArchive(m.id.clone())),
            );
        } else {
            col = col.push(
                button(text("Move to Active"))
                    .width(Length::Fill)
                    .on_press(Message::MoveToActive(m.id.clone())),
            );
        }
        let remove_label = if self.pending_remove.as_deref() == Some(m.id.as_str()) {
            "Confirm delete (permanent)"
        } else {
            "Remove mod"
        };
        col = col.push(
            button(text(remove_label))
                .style(button::danger)
                .width(Length::Fill)
                .on_press(Message::RemoveMod(m.id.clone())),
        );

        Some(
            container(scrollable(col))
                .width(DETAILS_W)
                .height(Length::Fill)
                .padding(14)
                .style(container::bordered_box)
                .into(),
        )
    }

    fn conflicts_view(&self) -> Element<'_, Message> {
        if self.conflicts.is_empty() {
            return container(text("No conflicts detected.").size(15))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();
        }
        let mut col = column![text(format!(
            "{} conflicts found:",
            self.conflicts.len()
        ))
        .size(16)]
        .spacing(8);
        for c in &self.conflicts {
            col = col.push(text(format!(
                "⚠ {}  ×  {}  —  {:?}",
                c.mod1, c.mod2, c.conflict_type
            )));
        }
        scrollable(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn settings_view(&self) -> Element<'_, Message> {
        let total = self.mod_manager.get_mods().len();
        let active = self
            .mod_manager
            .get_mods_by_state(ModState::Active)
            .len();
        let archived = self
            .mod_manager
            .get_mods_by_state(ModState::Archived)
            .len();
        column![
            text("Settings").size(20),
            text(format!(
                "Archive folder: {}",
                self.mod_manager.get_archive_dir().display()
            )),
            text(format!(
                "Active folder: {}",
                self.mod_manager.get_active_dir().display()
            )),
            text(format!(
                "Mods: {} total — {} active, {} archived",
                total, active, archived
            )),
            text(format!("Favorites: {}", self.mod_manager.favorites_count())),
            text("Folder paths are stored in config.toml (fs-mod-manager folder in your user config directory). GUI path editing is a later addition.")
                .size(12),
            button(text("Rescan folders")).on_press(Message::Scan),
        ]
        .spacing(10)
        .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let header = row![
            text("FS Mod Manager").size(24),
            Space::with_width(Length::Fill),
            text_input("Search mods...", &self.search_query)
                .on_input(Message::SearchChanged)
                .padding(8)
                .width(320),
            button(text("Rescan")).on_press(Message::Scan),
            button(text("Exit")).on_press(Message::Exit),
        ]
        .spacing(10);

        let fav_style = if self.favorites_only {
            button::primary
        } else {
            button::secondary
        };
        let tabs = row![
            self.tab_button(
                format!("Browse ({})", self.mod_manager.get_mods().len()),
                Tab::Browse
            ),
            self.tab_button(
                format!("Conflicts ({})", self.conflicts.len()),
                Tab::Conflicts
            ),
            self.tab_button("Settings".to_string(), Tab::Settings),
            Space::with_width(Length::Fill),
            button(text("★ Favorites only"))
                .style(fav_style)
                .on_press(Message::ToggleFavoritesOnly),
        ]
        .spacing(8);

        let content: Element<'_, Message> = match self.current_tab {
            Tab::Browse => self.browse_view(),
            Tab::Conflicts => self.conflicts_view(),
            Tab::Settings => self.settings_view(),
        };

        let status = row![
            text(&self.status).size(13),
            Space::with_width(Length::Fill),
            text(format!("★ {}", self.mod_manager.favorites_count())).size(13),
        ];

        column![header, tabs, content, status]
            .spacing(10)
            .padding(12)
            .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        iced::event::listen_with(|event, _status, _id| match event {
            iced::Event::Window(iced::window::Event::Resized(size)) => {
                Some(Message::WindowResized(size))
            }
            _ => None,
        })
    }
}

pub fn run(mod_manager: ModManager) -> anyhow::Result<()> {
    let result = iced::application("FS Mod Manager", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_: &App| Theme::Dark)
        .window_size((1280.0, 800.0))
        .run_with(move || (App::new(mod_manager), Task::none()));
    match result {
        Ok(()) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("GUI failed to start: {:?}", e)),
    }
}

use crate::mod_manager::conflict::{Conflict, ConflictDetector};
use crate::mod_manager::icons::{load_icon, LoadedIcon};
use crate::mod_manager::types::{Mod, ModState};
use crate::mod_manager::ModManager;
use crate::ui::components::{format_size, mod_card, BrowseView, Message, ScanOutcome, Tab};
use iced::widget::image::Handle as ImageHandle;
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input, Space};
use iced::{Element, Length, Size, Subscription, Task, Theme};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;

const CARD_H: f32 = 250.0;
const GRID_SPACING: f32 = 12.0;
const DETAILS_W: f32 = 344.0;

pub struct App {
    mod_manager: ModManager,
    current_tab: Tab,
    view_filter: BrowseView,
    search_query: String,
    favorites_only: bool,
    selected_mod: Option<String>,
    pending_remove: Option<String>,
    conflicts: Vec<Conflict>,
    icons: HashMap<String, ImageHandle>,
    active_dir_input: String,
    archive_dir_input: String,
    status: String,
    loading: bool,
    window_size: Size,
}

impl App {
    pub fn new(mod_manager: ModManager) -> Self {
        let active_dir_input = mod_manager.get_active_dir().display().to_string();
        let archive_dir_input = mod_manager.get_archive_dir().display().to_string();
        Self {
            mod_manager,
            current_tab: Tab::Browse,
            view_filter: BrowseView::All,
            search_query: String::new(),
            favorites_only: false,
            selected_mod: None,
            pending_remove: None,
            conflicts: Vec::new(),
            icons: HashMap::new(),
            active_dir_input,
            archive_dir_input,
            status: "Loading mods…".to_string(),
            loading: true,
            window_size: Size::new(1280.0, 800.0),
        }
    }

    /// Kick off a background library load — scan both folders and
    /// detect conflicts off the UI thread, then chain into icon
    /// loading. The window stays responsive throughout, however big
    /// the library is.
    fn begin_load(&mut self) -> Task<Message> {
        self.loading = true;
        self.status = "Loading mods in the background…".to_string();
        let (archive, active, parallel) = self.mod_manager.scan_params();
        Task::perform(
            async move { scan_job(archive, active, parallel) },
            Message::ScanLoaded,
        )
    }

    fn begin_icon_load(&self) -> Task<Message> {
        let jobs: Vec<(String, PathBuf, Option<String>)> = self
            .mod_manager
            .get_mods()
            .into_iter()
            .map(|m| (m.id.clone(), m.path.clone(), m.icon_filename.clone()))
            .collect();
        Task::perform(async move { icon_job(jobs) }, Message::IconsLoaded)
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
                    && (match &self.view_filter {
                        BrowseView::All => true,
                        BrowseView::Active => m.state == ModState::Active,
                        BrowseView::Archived => m.state == ModState::Archived,
                        BrowseView::Favorites => {
                            self.mod_manager.is_favorite(&m.favorite_key())
                        }
                        BrowseView::Category(c) => {
                            m.categories.iter().any(|x| x == c)
                        }
                    })
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
            Message::Scan => {
                if self.loading {
                    self.status = "Still loading — give it a moment.".to_string();
                } else {
                    return self.begin_load();
                }
            }
            Message::ScanLoaded(outcome) => match outcome {
                Ok(outcome) => {
                    let count = outcome.mods.len();
                    self.mod_manager.install_mods(
                        outcome
                            .mods
                            .into_iter()
                            .map(|m| (m.id.clone(), m))
                            .collect(),
                    );
                    self.conflicts = outcome.conflicts;
                    if let Some(id) = &self.selected_mod {
                        if self.mod_manager.get_mod(id).is_none() {
                            self.selected_mod = None;
                        }
                    }
                    self.status = format!("Loaded {} mods — loading icons…", count);
                    return self.begin_icon_load();
                }
                Err(e) => {
                    self.loading = false;
                    self.status = format!("Scan failed: {}", e);
                }
            },
            Message::IconsLoaded(loaded) => {
                self.icons.clear();
                for (id, icon) in loaded {
                    self.icons.insert(id, to_handle(icon));
                }
                self.loading = false;
                self.status = format!(
                    "Ready — {} mods, {} conflicts.",
                    self.mod_manager.get_mods().len(),
                    self.conflicts.len()
                );
            }
            Message::SearchChanged(query) => self.search_query = query,
            Message::TabSelected(tab) => self.current_tab = tab,
            Message::BrowseViewChanged(view) => self.view_filter = view,
            Message::SelectMod(id) => self.selected_mod = Some(id),
            Message::CloseDetails => self.selected_mod = None,
            Message::ToggleFavoritesOnly => {
                self.favorites_only = !self.favorites_only;
            }
            Message::ToggleFavorite(id) => {
                if let Some(m) = self.mod_manager.get_mod(&id).cloned() {
                    let key = m.favorite_key();
                    match self.mod_manager.toggle_favorite(&key) {
                        Ok(true) => self.status = format!("{} added to favorites.", m.name),
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
                            self.icons.remove(&id);
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
            Message::ActiveDirChanged(value) => self.active_dir_input = value,
            Message::ArchiveDirChanged(value) => self.archive_dir_input = value,
            Message::SaveDirectories => {
                if self.loading {
                    self.status =
                        "Still loading — wait for it to finish before changing folders."
                            .to_string();
                } else {
                    let active = std::path::PathBuf::from(self.active_dir_input.trim());
                    let archive = std::path::PathBuf::from(self.archive_dir_input.trim());
                    match self.mod_manager.set_directories(active, archive) {
                        Ok(()) => {
                            // Show the folders as saved (tilde expanded).
                            self.active_dir_input =
                                self.mod_manager.get_active_dir().display().to_string();
                            self.archive_dir_input =
                                self.mod_manager.get_archive_dir().display().to_string();
                            return self.begin_load();
                        }
                        Err(e) => {
                            self.status = format!("Could not change folders: {}", e)
                        }
                    }
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

        // One dropdown picks what the grid shows: every mod, one
        // folder's mods, favorites, or one detected category.
        let mut view_options = vec![
            BrowseView::All,
            BrowseView::Active,
            BrowseView::Archived,
            BrowseView::Favorites,
        ];
        let mut cats: Vec<String> = self
            .mod_manager
            .get_mods()
            .into_iter()
            .flat_map(|m| m.categories.iter().cloned())
            .collect();
        cats.sort();
        cats.dedup();
        view_options.extend(cats.into_iter().map(BrowseView::Category));
        let toolbar = row![
            text("Show:"),
            pick_list(
                view_options,
                Some(self.view_filter.clone()),
                Message::BrowseViewChanged
            ),
        ]
        .spacing(8);
        let pane_open = self
            .selected_mod
            .as_ref()
            .map_or(false, |id| self.mod_manager.get_mod(id).is_some());

        let content: Element<'_, Message> = if mods.is_empty() {
            container(
                text(if self.loading {
                    "Loading mods… big libraries take a while. The window stays usable — watch the status bar below."
                } else if self.mod_manager.get_mods().is_empty() {
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
            // Layout rule (John): 5 cards per row with no details pane,
            // still 4 per row with the fly-out open. Card width flexes to
            // make the count exact; on very narrow windows the count steps
            // down so a card never gets narrower than readable.
            let target_cols: usize = if pane_open { 4 } else { 5 };
            let content_w = (self.window_size.width - 24.0).max(150.0);
            let grid_w = if pane_open {
                (content_w - DETAILS_W - GRID_SPACING).max(150.0)
            } else {
                content_w
            };
            let mut cols = target_cols;
            while cols > 1
                && (grid_w - (cols as f32 - 1.0) * GRID_SPACING) / (cols as f32) < 150.0
            {
                cols -= 1;
            }
            let card_w = (grid_w - (cols as f32 - 1.0) * GRID_SPACING) / cols as f32;

            let mut grid = column![].spacing(GRID_SPACING);
            for chunk in mods.chunks(cols) {
                let mut r = row![].spacing(GRID_SPACING);
                for m in chunk {
                    let fav = self.mod_manager.is_favorite(&m.favorite_key());
                    let selected = self.selected_mod.as_deref() == Some(m.id.as_str());
                    let conflict = self.mod_has_conflict(&m.name);
                    r = r.push(mod_card(
                        m,
                        self.icons.get(&m.id),
                        fav,
                        selected,
                        conflict,
                        card_w,
                        CARD_H,
                    ));
                }
                grid = grid.push(r);
            }
            scrollable(grid)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };

        let body: Element<'_, Message> = if let Some(details) = self.details_view() {
            row![content, details]
                .spacing(GRID_SPACING)
                .height(Length::Fill)
                .into()
        } else {
            content
        };

        column![toolbar, body].spacing(8).into()
    }

    fn details_view(&self) -> Option<Element<'_, Message>> {
        let id = self.selected_mod.as_ref()?;
        let m = self.mod_manager.get_mod(id)?;

        let fav = self.mod_manager.is_favorite(&m.favorite_key());
        let fav_label = if fav {
            "Favorited"
        } else {
            "Add to favorites"
        };

        let mut col = column![
            row![
                text(&m.name).size(20),
                Space::with_width(Length::Fill),
                button(text("X").size(14)).on_press(Message::CloseDetails),
            ],
            text(format!("Version {}", m.version)).size(14),
        ]
        .spacing(8);

        if let Some(handle) = self.icons.get(&m.id) {
            col = col.push(
                container(
                    iced::widget::image(handle.clone())
                        .width(180)
                        .height(180)
                        .content_fit(iced::ContentFit::Contain),
                )
                .width(Length::Fill)
                .center_x(Length::Fill),
            );
        }

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
                    text(format!("{} — {:?}", other, c.conflict_type)).size(12),
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
                "{}  ×  {}  —  {:?}",
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
            text("Active folder (mods in the game):"),
            text_input("Active folder path", &self.active_dir_input)
                .on_input(Message::ActiveDirChanged)
                .padding(8),
            text("Archive folder (stored mods):"),
            text_input("Archive folder path", &self.archive_dir_input)
                .on_input(Message::ArchiveDirChanged)
                .padding(8),
            button(text("Save folders")).on_press(Message::SaveDirectories),
            text(format!(
                "Mods: {} total — {} active, {} archived",
                total, active, archived
            )),
            text(format!("Favorites: {}", self.mod_manager.favorites_count())),
            text("Paths are saved to config.toml when you click Save folders.")
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
            button(text("Favorites only"))
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
            text(format!("Favorites: {}", self.mod_manager.favorites_count())).size(13),
        ];

        // The root column MUST be bounded to the window. Without an
        // explicit Fill size it grows with its content, the scrollable
        // inside then has no real viewport to clip to — scrolled rows
        // draw over the header and the status bar is pushed off-screen.
        column![header, tabs, content, status]
            .spacing(10)
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill)
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

fn to_handle(icon: LoadedIcon) -> ImageHandle {
    match icon {
        LoadedIcon::Rgba {
            width,
            height,
            pixels,
        } => ImageHandle::from_rgba(width, height, pixels),
        LoadedIcon::Encoded(bytes) => ImageHandle::from_bytes(bytes),
    }
}

/// Background job: scan both folders and detect conflicts.
fn scan_job(archive: PathBuf, active: PathBuf, parallel: usize) -> Result<ScanOutcome, String> {
    let mods = ModManager::scan_directories(archive, active, parallel)
        .map_err(|e| e.to_string())?;
    let mod_list: Vec<Mod> = mods.into_values().collect();
    let conflicts = ConflictDetector::detect_conflicts(&mod_list);
    Ok(ScanOutcome {
        mods: mod_list,
        conflicts,
    })
}

/// Background job: load every mod's icon, in parallel.
fn icon_job(jobs: Vec<(String, PathBuf, Option<String>)>) -> Vec<(String, LoadedIcon)> {
    jobs.par_iter()
        .filter_map(|(id, path, icon)| {
            load_icon(path, icon.as_deref()).map(|loaded| (id.clone(), loaded))
        })
        .collect()
}

pub fn run(mod_manager: ModManager) -> anyhow::Result<()> {
    let result = iced::application("FS Mod Manager", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_: &App| Theme::Dark)
        .window_size((1280.0, 800.0))
        .run_with(move || {
            let mut app = App::new(mod_manager);
            let task = app.begin_load();
            (app, task)
        });
    match result {
        Ok(()) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("GUI failed to start: {:?}", e)),
    }
}

use crate::mod_manager::conflict::{Conflict, ConflictDetector, ConflictType};
use crate::mod_manager::game_files;
use crate::mod_manager::icons::{load_icon, LoadedIcon};
use crate::mod_manager::saves::SaveSlot;
use crate::mod_manager::types::{Mod, ModState};
use crate::mod_manager::ModManager;
use crate::ui::components::{
    format_size, mod_card, BrowseView, FolderTarget, LogKind, LogLine, LogOutcome,
    Message, ScanOutcome, Tab,
};
use iced::widget::image::Handle as ImageHandle;
use iced::widget::{
    button, checkbox, column, container, pick_list, row, scrollable, text, text_input,
    Space,
};
use iced::{Element, Length, Size, Subscription, Task, Theme};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const CARD_H: f32 = 258.0;
const SIDEBAR_W: f32 = 232.0;
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
    /// Settings: the game install folder display + the console
    /// switch state read from its game.xml (None = unknown).
    game_dir_input: String,
    console_on: Option<bool>,
    /// A status note to show once the current load finishes
    /// (e.g. the folder switcher's gameSettings.xml result,
    /// which a rescan would otherwise overwrite).
    pending_status: Option<String>,
    status: String,
    loading: bool,
    /// Set when a card's star badge is clicked: the same click also
    /// reaches the card's select handler (nested widgets), and this
    /// flag tells it to stand down once.
    just_toggled_favorite: bool,
    /// Configure Save (Saves tab) state.
    save_slots_cache: Vec<SaveSlot>,
    saves_selected: Option<String>,
    saves_draft: HashSet<String>,
    saves_filter: String,
    saves_arm_apply: bool,
    /// Log Reader state.
    log_lines: Vec<LogLine>,
    log_suspects: Vec<String>,
    log_error_count: usize,
    log_warning_count: usize,
    log_total_lines: usize,
    log_errors_only: bool,
    log_note: String,
    window_size: Size,
}

impl App {
    pub fn new(mod_manager: ModManager) -> Self {
        let active_dir_input = mod_manager.get_active_dir().display().to_string();
        let archive_dir_input = mod_manager.get_archive_dir().display().to_string();
        let game_dir_input = mod_manager
            .game_directory()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let console_on = mod_manager
            .game_directory()
            .and_then(|d| game_files::console_enabled(&d));
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
            game_dir_input,
            console_on,
            pending_status: None,
            status: "Loading mods…".to_string(),
            loading: true,
            just_toggled_favorite: false,
            save_slots_cache: Vec::new(),
            saves_selected: None,
            saves_draft: HashSet::new(),
            saves_filter: String::new(),
            saves_arm_apply: false,
            log_lines: Vec::new(),
            log_suspects: Vec::new(),
            log_error_count: 0,
            log_warning_count: 0,
            log_total_lines: 0,
            log_errors_only: false,
            log_note: String::new(),
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

    /// Save the folder inputs and rescan — the Settings tab's
    /// Save button and the Browse pickers share this path.
    fn apply_directories(&mut self) -> Task<Message> {
        if self.loading {
            self.status =
                "Still loading — wait for it to finish before changing folders."
                    .to_string();
            return Task::none();
        }
        let active = std::path::PathBuf::from(self.active_dir_input.trim());
        let archive = std::path::PathBuf::from(self.archive_dir_input.trim());
        match self.mod_manager.set_directories(active, archive) {
            Ok(()) => {
                // Show the folders as saved (tilde expanded).
                self.active_dir_input =
                    self.mod_manager.get_active_dir().display().to_string();
                self.archive_dir_input =
                    self.mod_manager.get_archive_dir().display().to_string();
                self.begin_load()
            }
            Err(e) => {
                self.status = format!("Could not change folders: {}", e);
                Task::none()
            }
        }
    }

    /// Read the game's log.txt in the background (Log Reader).
    fn begin_log_load(&mut self) -> Task<Message> {
        match self.mod_manager.profile_dir() {
            Some(profile) => {
                self.log_note = "Reading log.txt…".to_string();
                let stems: Vec<(String, String)> = self
                    .mod_manager
                    .get_mods()
                    .into_iter()
                    .filter_map(|m| {
                        let file = m.path.file_name()?.to_string_lossy().to_lowercase();
                        let stem =
                            file.strip_suffix(".zip").unwrap_or(&file).to_string();
                        if stem.len() >= 8 {
                            Some((stem, m.name.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();
                Task::perform(
                    async move { log_job(profile, stems) },
                    Message::LogLoaded,
                )
            }
            None => {
                self.log_note = "No game profile folder — the active folder isn't a game's mods folder."
                    .to_string();
                Task::none()
            }
        }
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

    /// Shared favorite-toggle logic (fly-out button and card star badge).
    fn apply_toggle_favorite(&mut self, id: &str) {
        if let Some(m) = self.mod_manager.get_mod(id).cloned() {
            let key = m.favorite_key();
            match self.mod_manager.toggle_favorite(&key) {
                Ok(true) => self.status = format!("{} added to favorites.", m.name),
                Ok(false) => self.status = format!("{} removed from favorites.", m.name),
                Err(e) => self.status = format!("Favorite failed: {}", e),
            }
        }
    }

    fn refresh_save_slots(&mut self) {
        self.save_slots_cache = self.mod_manager.save_slots();
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
                    if let Some(note) = self.pending_status.take() {
                        self.status = note;
                    }
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
                if let Some(note) = self.pending_status.take() {
                    self.status = note;
                }
            }
            Message::SelectSaveSlot(dir_name) => {
                self.saves_draft = self
                    .mod_manager
                    .save_loadout(&dir_name)
                    .into_iter()
                    .collect();
                self.saves_selected = Some(dir_name);
                self.saves_arm_apply = false;
            }
            Message::DraftToggle(key) => {
                if !self.saves_draft.remove(&key) {
                    self.saves_draft.insert(key);
                }
                self.saves_arm_apply = false;
            }
            Message::DraftUseActive => {
                self.saves_draft = self
                    .mod_manager
                    .get_mods()
                    .into_iter()
                    .filter(|m| m.state == ModState::Active)
                    .map(|m| m.favorite_key())
                    .collect();
                self.saves_arm_apply = false;
            }
            Message::DraftEmpty => {
                self.saves_draft.clear();
                self.saves_arm_apply = false;
            }
            Message::SaveLoadout => {
                if let Some(slot) = self.saves_selected.clone() {
                    let keys: Vec<String> = self.saves_draft.iter().cloned().collect();
                    match self.mod_manager.set_save_loadout(&slot, keys) {
                        Ok(()) => {
                            self.status = format!(
                                "Loadout saved for {} ({} mods).",
                                slot,
                                self.saves_draft.len()
                            );
                            self.refresh_save_slots();
                        }
                        Err(e) => self.status = format!("Saving loadout failed: {}", e),
                    }
                }
            }
            Message::ApplyLoadout => {
                if !self.saves_arm_apply {
                    self.saves_arm_apply = true;
                    self.status = "Click Apply again to confirm — this moves mods between Active and Archive. Make sure the game is not running.".to_string();
                } else if let Some(slot) = self.saves_selected.clone() {
                    self.saves_arm_apply = false;
                    let keys: Vec<String> = self.saves_draft.iter().cloned().collect();
                    let _ = self.mod_manager.set_save_loadout(&slot, keys.clone());
                    match self.mod_manager.apply_loadout(&keys) {
                        Ok((to_archive, to_active)) => {
                            self.recompute_conflicts();
                            self.refresh_save_slots();
                            self.status = format!(
                                "Loadout applied for {}: {} moved to archive, {} moved to active.",
                                slot, to_archive, to_active
                            );
                        }
                        Err(e) => self.status = format!("Applying loadout failed: {}", e),
                    }
                }
            }
            Message::SavesFilterChanged(filter) => self.saves_filter = filter,
            Message::SearchChanged(query) => self.search_query = query,
            Message::LibraryFilter(view) => {
                self.current_tab = Tab::Browse;
                self.view_filter = view;
                self.favorites_only = false;
            }
            Message::TabSelected(tab) => {
                if tab == Tab::Saves {
                    self.refresh_save_slots();
                }
                self.current_tab = tab;
                if tab == Tab::Log {
                    return self.begin_log_load();
                }
            }
            Message::BrowseViewChanged(view) => self.view_filter = view,
            Message::SelectMod(id) => {
                // A click on a card's star badge also lands here (the
                // badge is nested in the card) — the favorite toggle
                // sets this flag first, so ignore that one selection.
                if self.just_toggled_favorite {
                    self.just_toggled_favorite = false;
                } else {
                    self.selected_mod = Some(id);
                }
            }
            Message::CloseDetails => self.selected_mod = None,
            Message::ToggleFavoritesOnly => {
                self.favorites_only = !self.favorites_only;
            }
            Message::ToggleFavorite(id) => {
                self.apply_toggle_favorite(&id);
            }
            Message::CardToggleFavorite(id) => {
                // The badge click also reaches the card's select
                // handler right after; flag it to stand down once.
                self.just_toggled_favorite = true;
                self.apply_toggle_favorite(&id);
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
                        "Click Confirm delete to permanently delete {}.",
                        name
                    );
                }
            }
            Message::ActiveDirChanged(value) => self.active_dir_input = value,
            Message::ArchiveDirChanged(value) => self.archive_dir_input = value,
            Message::SaveDirectories => return self.apply_directories(),
            Message::BrowseFolder(target) => {
                let title = match target {
                    FolderTarget::Active => "Choose the game's mods folder",
                    FolderTarget::Archive => "Choose the archive folder",
                    FolderTarget::Game => {
                        "Choose the Farming Simulator install folder (holds game.xml)"
                    }
                };
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title(title)
                            .pick_folder()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    move |picked| Message::FolderPicked(target, picked),
                );
            }
            Message::FolderPicked(target, picked) => {
                let Some(path) = picked else {
                    return Task::none();
                };
                match target {
                    FolderTarget::Active => {
                        if self.loading {
                            self.status = "Still loading — wait for it to finish before changing folders."
                                .to_string();
                            return Task::none();
                        }
                        self.active_dir_input = path.display().to_string();
                        // The folder switcher: also point the GAME
                        // at this folder via gameSettings.xml. That
                        // file lives in the profile folder — prefer
                        // the current profile, else the picked
                        // folder's parent when it's named "mods".
                        let profile = self.mod_manager.profile_dir().or_else(|| {
                            if path
                                .file_name()
                                .map_or(false, |n| n.eq_ignore_ascii_case("mods"))
                            {
                                path.parent().map(|p| p.to_path_buf())
                            } else {
                                None
                            }
                        });
                        self.pending_status = Some(match profile {
                            Some(profile) => {
                                match game_files::set_mods_override(&profile, &path) {
                                    Ok(()) => format!(
                                        "Game mods folder set to {} — gameSettings.xml updated.",
                                        path.display()
                                    ),
                                    Err(e) => format!(
                                        "Folders saved, but gameSettings.xml was not updated: {}",
                                        e
                                    ),
                                }
                            }
                            None => "Folders saved. gameSettings.xml left alone — no game profile folder found."
                                .to_string(),
                        });
                        return self.apply_directories();
                    }
                    FolderTarget::Archive => {
                        self.archive_dir_input = path.display().to_string();
                        return self.apply_directories();
                    }
                    FolderTarget::Game => {
                        match self.mod_manager.set_game_directory(path.clone()) {
                            Ok(()) => {
                                self.game_dir_input = path.display().to_string();
                                self.console_on = game_files::console_enabled(&path);
                                self.status = match self.console_on {
                                    Some(on) => format!(
                                        "Game folder set: {}. Developer console is {}.",
                                        path.display(),
                                        if on { "on" } else { "off" }
                                    ),
                                    None => format!(
                                        "Game folder set: {}, but no game.xml console switch was found there.",
                                        path.display()
                                    ),
                                };
                            }
                            Err(e) => {
                                self.status =
                                    format!("Could not save the game folder: {}", e)
                            }
                        }
                    }
                }
            }
            Message::ToggleConsole(on) => match self.mod_manager.game_directory() {
                Some(dir) => match game_files::set_console(&dir, on) {
                    Ok(()) => {
                        self.console_on = Some(on);
                        self.status = format!(
                            "Developer console {} (game.xml).",
                            if on { "on" } else { "off" }
                        );
                    }
                    Err(e) => {
                        self.status =
                            format!("Could not change the console setting: {}", e)
                    }
                },
                None => {
                    self.status = "Set the game install folder first — the console switch lives in its game.xml."
                        .to_string()
                }
            },
            Message::RefreshLog => return self.begin_log_load(),
            Message::LogLoaded(outcome) => match outcome {
                Ok(o) => {
                    self.log_lines = o.lines;
                    self.log_suspects = o.suspects;
                    self.log_error_count = o.error_count;
                    self.log_warning_count = o.warning_count;
                    self.log_total_lines = o.total_lines;
                    self.log_note = String::new();
                }
                Err(e) => {
                    self.log_lines = Vec::new();
                    self.log_suspects = Vec::new();
                    self.log_error_count = 0;
                    self.log_warning_count = 0;
                    self.log_total_lines = 0;
                    self.log_note = e;
                }
            },
            Message::LogErrorsOnlyToggled(on) => self.log_errors_only = on,
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
        // Stat boxes across the top (John, round 4 — Hub's tiles):
        // installed / favourited / maps / disk usage.
        let all_mods = self.mod_manager.get_mods();
        let total_size: u64 = all_mods.iter().map(|m| m.size).sum();
        let maps_count = all_mods
            .iter()
            .filter(|m| m.categories.iter().any(|c| c == "Maps"))
            .count();
        let tile = |value: String, label: &str| {
            container(
                column![
                    text(value).size(19),
                    text(label.to_string()).size(11),
                ]
                .spacing(2),
            )
            .width(Length::Fill)
            .padding(10)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb(
                    0.13, 0.14, 0.16,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgb(0.30, 0.31, 0.34),
                    width: 1.0,
                    radius: 8.0.into(),
                },
                ..container::Style::default()
            })
        };
        let tiles = row![
            tile(all_mods.len().to_string(), "INSTALLED"),
            tile(self.mod_manager.favorites_count().to_string(), "FAVOURITED"),
            tile(maps_count.to_string(), "MAPS"),
            tile(format_size(total_size), "DISK USAGE"),
        ]
        .spacing(10);

        // No "Favorites only" button here (John): favorites are
        // already one pick in the Show dropdown and in the sidebar.
        // The search box lives on this row too (John) — it moved
        // down from the header to sit with the other browse controls.
        // Both boxes get rounded corners (John).
        let toolbar = row![
            text("Show:"),
            pick_list(
                view_options,
                Some(self.view_filter.clone()),
                Message::BrowseViewChanged
            )
            .style(|theme, status| {
                let mut s = pick_list::default(theme, status);
                s.border.radius = 8.0.into();
                s.border.width = 1.0;
                s.border.color = iced::Color::from_rgb(0.35, 0.36, 0.40);
                s
            }),
            text_input("Search mods...", &self.search_query)
                .on_input(Message::SearchChanged)
                .padding(8)
                .width(300)
                .style(|theme, status| {
                    let mut s = text_input::default(theme, status);
                    s.border.radius = 8.0.into();
                    s.border.width = 1.0;
                    s.border.color = iced::Color::from_rgb(0.35, 0.36, 0.40);
                    s
                }),
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
            // Layout rule (John, round 4, supersedes the old 5/4 rule):
            // FarmSim Hub's cards are about half our old width, so the
            // grid now aims for a ~185px card and lets the column count
            // follow the window width. On very narrow windows the count
            // steps down so a card never gets narrower than readable.
            let content_w = (self.window_size.width - 24.0 - SIDEBAR_W - GRID_SPACING)
                .max(150.0);
            let grid_w = if pane_open {
                (content_w - DETAILS_W - GRID_SPACING).max(150.0)
            } else {
                content_w
            };
            let mut cols =
                (((grid_w + GRID_SPACING) / (185.0 + GRID_SPACING)).floor() as usize)
                    .clamp(3, 14);
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

        column![tiles, toolbar, body].spacing(8).into()
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

        // Cards, like Browse (John): one card per duplicate group.
        // The card shares the mod's icon, every copy is labelled
        // with an Active/Archived pill (same colors as the browse
        // cards), and each copy has its own remove button so John
        // chooses which copy survives. Removal reuses the details
        // view's arm-to-confirm RemoveMod flow and recomputes
        // conflicts afterwards. Other conflict types get a simple
        // card each.
        let card_style = |_: &Theme| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.13, 0.14, 0.16,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgb(0.30, 0.31, 0.34),
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        };
        let pill = |label: &str, bg: iced::Color| {
            container(
                text(label.to_string())
                    .size(10)
                    .wrapping(iced::widget::text::Wrapping::None)
                    .color(iced::Color::from_rgb(0.97, 0.97, 0.98)),
            )
            .padding([2, 6])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: 9.0.into(),
                    ..iced::Border::default()
                },
                ..container::Style::default()
            })
        };

        // Responsive card grid, same approach as Browse: aim for
        // ~440px cards, column count follows the window width.
        let content_w =
            (self.window_size.width - 24.0 - SIDEBAR_W - GRID_SPACING).max(200.0);
        let cols = (((content_w + GRID_SPACING) / (440.0 + GRID_SPACING)).floor()
            as usize)
            .clamp(1, 4);
        let card_w = (content_w - (cols as f32 - 1.0) * GRID_SPACING) / cols as f32;

        let mut dup_names: Vec<String> = Vec::new();
        for c in &self.conflicts {
            if matches!(
                c.conflict_type,
                ConflictType::DuplicateMod | ConflictType::VersionMismatch
            ) && !dup_names.contains(&c.mod1)
            {
                dup_names.push(c.mod1.clone());
            }
        }
        dup_names.sort();

        let mut cards: Vec<Element<'_, Message>> = Vec::new();
        for name in &dup_names {
            let mut copies: Vec<&Mod> = self
                .mod_manager
                .get_mods()
                .into_iter()
                .filter(|m| &m.name == name)
                .collect();
            copies.sort_by_key(|m| m.state != ModState::Active);

            let header_icon: Element<'_, Message> =
                match copies.first().and_then(|m| self.icons.get(&m.id)) {
                    Some(handle) => iced::widget::image(handle.clone())
                        .width(44)
                        .height(44)
                        .into(),
                    None => container(text("")).width(44).height(44).into(),
                };
            let mut card_col = column![
                row![
                    header_icon,
                    column![
                        text(name.clone()).size(15),
                        text(format!("{} copies", copies.len())).size(12),
                    ]
                    .spacing(2),
                ]
                .spacing(8)
            ]
            .spacing(8);
            for m in copies {
                let (loc, bg) = match m.state {
                    ModState::Active => {
                        ("Active", iced::Color::from_rgb(0.16, 0.45, 0.22))
                    }
                    ModState::Archived => {
                        ("Archived", iced::Color::from_rgb(0.55, 0.42, 0.13))
                    }
                    ModState::Conflict => {
                        ("Conflict", iced::Color::from_rgb(0.62, 0.16, 0.13))
                    }
                    ModState::Error => {
                        ("Error", iced::Color::from_rgb(0.62, 0.16, 0.13))
                    }
                };
                let file = m
                    .path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let armed = self.pending_remove.as_deref() == Some(m.id.as_str());
                // Remove control (John): a red circle with a minus,
                // sitting where Browse cards carry the favorite
                // star. First click arms it into a Confirm delete
                // pill; the second click deletes.
                let btn: Element<'_, Message> = if armed {
                    button(text("Confirm delete").size(12))
                        .style(button::danger)
                        .on_press(Message::RemoveMod(m.id.clone()))
                        .into()
                } else {
                    let bar = container(text("")).width(12).height(3).style(|_| {
                        container::Style {
                            background: Some(iced::Background::Color(
                                iced::Color::from_rgb(0.97, 0.97, 0.98),
                            )),
                            border: iced::Border {
                                radius: 2.0.into(),
                                ..iced::Border::default()
                            },
                            ..container::Style::default()
                        }
                    });
                    let circle = container(bar)
                        .width(26)
                        .height(26)
                        .align_x(iced::Alignment::Center)
                        .align_y(iced::Alignment::Center)
                        .style(|_| container::Style {
                            background: Some(iced::Background::Color(
                                iced::Color::from_rgb(0.62, 0.16, 0.13),
                            )),
                            border: iced::Border {
                                radius: 13.0.into(),
                                ..iced::Border::default()
                            },
                            ..container::Style::default()
                        });
                    button(circle)
                        .padding(0)
                        .style(button::text)
                        .on_press(Message::RemoveMod(m.id.clone()))
                        .into()
                };
                card_col = card_col.push(
                    row![
                        pill(loc, bg),
                        column![
                            text(format!("v{} · {}", m.version, format_size(m.size)))
                                .size(12),
                            text(file).size(11),
                        ]
                        .spacing(1),
                        Space::with_width(Length::Fill),
                        btn,
                    ]
                    .spacing(8),
                );
            }
            cards.push(
                container(card_col)
                    .width(card_w)
                    .padding(12)
                    .style(card_style)
                    .into(),
            );
        }

        for c in &self.conflicts {
            if matches!(
                c.conflict_type,
                ConflictType::DuplicateMod | ConflictType::VersionMismatch
            ) {
                continue;
            }
            let card_col = column![
                text(format!("{}  ×  {}", c.mod1, c.mod2)).size(14),
                text(format!("{:?}", c.conflict_type)).size(12),
                text(c.description.clone()).size(12),
            ]
            .spacing(4);
            cards.push(
                container(card_col)
                    .width(card_w)
                    .padding(12)
                    .style(card_style)
                    .into(),
            );
        }

        let mut grid = column![].spacing(GRID_SPACING);
        let mut r = row![].spacing(GRID_SPACING);
        let mut n = 0;
        for card in cards {
            r = r.push(card);
            n += 1;
            if n == cols {
                grid = grid.push(r);
                r = row![].spacing(GRID_SPACING);
                n = 0;
            }
        }
        if n > 0 {
            grid = grid.push(r);
        }

        let page = column![
            text(format!("{} conflicts found:", self.conflicts.len())).size(16),
            text("Duplicates are cards — tap the red minus beside a copy to remove it.")
                .size(13),
            grid,
        ]
        .spacing(8);

        scrollable(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Configure Save: pick a save slot, edit its mod loadout, apply
    /// it (Active is reconciled to the loadout via the move system).
    fn saves_view(&self) -> Element<'_, Message> {
        if self.mod_manager.profile_dir().is_none() {
            return container(
                text("Savegames appear here when the Active folder is a game's mods folder (its parent holds the savegame folders).")
                    .size(14),
            )
            .padding(16)
            .into();
        }

        // ---- Slot list (left) ----
        let mut slots_col = column![text("Savegames").size(17)].spacing(8);
        if self.save_slots_cache.is_empty() {
            slots_col = slots_col.push(text("No save slots found in the game profile folder.").size(13));
        }
        for slot in &self.save_slots_cache {
            let loadout = self.mod_manager.save_loadout(&slot.dir_name);
            let info = format!(
                "{} · played {} · {}",
                slot.map.as_deref().unwrap_or("unknown map"),
                slot.play_time_display().as_deref().unwrap_or("?"),
                slot.money
                    .map(|m| format!("${}", m))
                    .unwrap_or_else(|| "?".to_string())
            );
            let loadout_line = if loadout.is_empty() {
                "No loadout saved".to_string()
            } else {
                format!("Loadout: {} mods", loadout.len())
            };
            let label = column![
                text(format!("Savegame {}", slot.slot)).size(14),
                text(info).size(12),
                text(loadout_line).size(12),
            ]
            .spacing(2);
            let is_selected = self.saves_selected.as_deref() == Some(slot.dir_name.as_str());
            slots_col = slots_col.push(
                button(label)
                    .width(Length::Fill)
                    .style(if is_selected {
                        button::primary
                    } else {
                        button::secondary
                    })
                    .on_press(Message::SelectSaveSlot(slot.dir_name.clone())),
            );
        }
        let slots_panel: Element<'_, Message> = container(scrollable(slots_col))
            .width(320)
            .height(Length::Fill)
            .into();

        // ---- Loadout editor (right) ----
        let editor: Element<'_, Message> = match &self.saves_selected {
            None => container(text("Pick a savegame on the left to edit its loadout.").size(14))
                .padding(16)
                .into(),
            Some(dir_name) => {
                let slot_num = self
                    .save_slots_cache
                    .iter()
                    .find(|s| &s.dir_name == dir_name)
                    .map(|s| s.slot);
                let title = match slot_num {
                    Some(n) => format!("Loadout for Savegame {}", n),
                    None => format!("Loadout for {}", dir_name),
                };
                let filter = self.saves_filter.to_lowercase();
                let mut mods: Vec<&Mod> = self.mod_manager.get_mods().into_iter().collect();
                mods.sort_by(|a, b| {
                    let (ka, kb) = (a.favorite_key(), b.favorite_key());
                    let (ia, ib) = (
                        self.saves_draft.contains(&ka),
                        self.saves_draft.contains(&kb),
                    );
                    ib.cmp(&ia)
                        .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                let visible: Vec<&Mod> = mods
                    .into_iter()
                    .filter(|m| {
                        if !filter.is_empty() {
                            m.name.to_lowercase().contains(&filter)
                        } else {
                            self.saves_draft.contains(&m.favorite_key())
                                || m.state == ModState::Active
                        }
                    })
                    .collect();

                let mut rows = column![].spacing(4);
                for m in &visible {
                    let key = m.favorite_key();
                    let in_draft = self.saves_draft.contains(&key);
                    rows = rows.push(
                        row![
                            button(text(if in_draft { "Remove" } else { "Add" }))
                                .style(if in_draft {
                                    button::danger
                                } else {
                                    button::secondary
                                })
                                .on_press(Message::DraftToggle(key)),
                            text(m.name.clone()).size(13),
                            text(format!("{:?}", m.state)).size(12),
                        ]
                        .spacing(8),
                    );
                }

                let apply_label = if self.saves_arm_apply {
                    "Click again to apply"
                } else {
                    "Apply loadout"
                };
                column![
                    text(title).size(17),
                    text(format!(
                        "{} mods in this loadout. Applying moves mods between Active and Archive to match — don't apply while the game is running.",
                        self.saves_draft.len()
                    ))
                    .size(12),
                    row![
                        button(text("Use current Active set"))
                            .on_press(Message::DraftUseActive),
                        button(text("Empty")).on_press(Message::DraftEmpty),
                        button(text("Save loadout"))
                            .style(button::primary)
                            .on_press(Message::SaveLoadout),
                        button(text(apply_label))
                            .style(button::primary)
                            .on_press(Message::ApplyLoadout),
                    ]
                    .spacing(8),
                    text_input("Filter mods by name…", &self.saves_filter)
                        .on_input(Message::SavesFilterChanged)
                        .padding(8),
                    text("With no filter, the list shows the loadout plus everything currently Active. Filter by name to find archived mods.")
                        .size(12),
                    scrollable(rows),
                ]
                .spacing(10)
                .into()
            }
        };

        row![slots_panel, editor].spacing(14).height(Length::Fill).into()
    }

    /// Log Reader: the game's log.txt from the profile folder,
    /// errors and warnings colored, with the library mods those
    /// lines name called out as suspects at the top.
    fn log_view(&self) -> Element<'_, Message> {
        if self.mod_manager.profile_dir().is_none() {
            return container(
                text("The game log appears here when the Active folder is a game's mods folder (log.txt sits in its profile folder).")
                    .size(14),
            )
            .padding(16)
            .into();
        }
        let mut col = column![
            row![
                text("Game log (log.txt)").size(17),
                Space::with_width(Length::Fill),
                button(text("Refresh")).on_press(Message::RefreshLog),
            ]
            .spacing(8),
            checkbox("Errors and warnings only", self.log_errors_only)
                .on_toggle(Message::LogErrorsOnlyToggled),
        ]
        .spacing(8);
        if !self.log_note.is_empty() {
            col = col.push(text(self.log_note.clone()).size(13));
        } else if self.log_total_lines > 0 {
            col = col.push(
                text(format!(
                    "{} lines · {} errors · {} warnings",
                    self.log_total_lines, self.log_error_count, self.log_warning_count
                ))
                .size(13),
            );
            if self.log_lines.len() < self.log_total_lines {
                col = col.push(
                    text(format!(
                        "Showing the last {} lines.",
                        self.log_lines.len()
                    ))
                    .size(12),
                );
            }
        }
        if !self.log_suspects.is_empty() {
            col = col.push(
                container(
                    column![
                        text("Mods named in errors/warnings:").size(13),
                        text(self.log_suspects.join(", ")).size(12),
                    ]
                    .spacing(2),
                )
                .padding(8)
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgb(
                        0.13, 0.14, 0.16,
                    ))),
                    border: iced::Border {
                        color: iced::Color::from_rgb(0.30, 0.31, 0.34),
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..container::Style::default()
                }),
            );
        }
        let mut lines_col = column![].spacing(1);
        for line in self
            .log_lines
            .iter()
            .filter(|l| !self.log_errors_only || l.kind != LogKind::Normal)
        {
            let color = match line.kind {
                LogKind::Error => iced::Color::from_rgb(0.96, 0.46, 0.40),
                LogKind::Warning => iced::Color::from_rgb(0.95, 0.78, 0.35),
                LogKind::Normal => iced::Color::from_rgb(0.75, 0.76, 0.79),
            };
            lines_col = lines_col.push(
                text(line.text.clone())
                    .size(12)
                    .font(iced::Font::MONOSPACE)
                    .color(color),
            );
        }
        if self.log_lines.is_empty() && self.log_note.is_empty() {
            lines_col = lines_col.push(text("No lines to show.").size(12));
        }
        col = col.push(
            scrollable(lines_col)
                .width(Length::Fill)
                .height(Length::Fill),
        );
        col.into()
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
        let console_row: Element<'_, Message> =
            match (self.mod_manager.game_directory(), self.console_on) {
                (Some(_), Some(on)) => row![
                    text(format!(
                        "Developer console (game.xml): {}",
                        if on { "On" } else { "Off" }
                    )),
                    button(text(if on { "Turn off" } else { "Turn on" }))
                        .on_press(Message::ToggleConsole(!on)),
                ]
                .spacing(8)
                .into(),
                (Some(_), None) => {
                    text("Developer console: no game.xml console switch found in the game folder.")
                        .size(13)
                        .into()
                }
                (None, _) => {
                    text("Developer console: set the game install folder to manage the console switch in game.xml.")
                        .size(13)
                        .into()
                }
            };
        let game_folder_label = if self.game_dir_input.is_empty() {
            "Not set".to_string()
        } else {
            self.game_dir_input.clone()
        };
        column![
            text("Settings").size(20),
            text("Game mods folder (Browse also points the game here via gameSettings.xml):"),
            row![
                text_input("Active folder path", &self.active_dir_input)
                    .on_input(Message::ActiveDirChanged)
                    .padding(8)
                    .width(Length::Fill),
                button(text("Browse")).on_press(Message::BrowseFolder(FolderTarget::Active)),
            ]
            .spacing(8),
            text("Archive folder (stored mods):"),
            row![
                text_input("Archive folder path", &self.archive_dir_input)
                    .on_input(Message::ArchiveDirChanged)
                    .padding(8)
                    .width(Length::Fill),
                button(text("Browse"))
                    .on_press(Message::BrowseFolder(FolderTarget::Archive)),
            ]
            .spacing(8),
            button(text("Save folders")).on_press(Message::SaveDirectories),
            text("Game install folder (holds game.xml):"),
            row![
                text(game_folder_label),
                button(text("Browse")).on_press(Message::BrowseFolder(FolderTarget::Game)),
            ]
            .spacing(8),
            console_row,
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

    /// Left sidebar (John, round 4 — the Hub layout piece): logo and
    /// app name up top, section navigation, then the LIBRARY counts
    /// that jump the Browse grid straight to that view.
    fn sidebar_view(&self) -> Element<'_, Message> {
        let all = self.mod_manager.get_mods();
        let count_cat = |name: &str| {
            all.iter()
                .filter(|m| m.categories.iter().any(|c| c == name))
                .count()
        };
        let lib_row = |label: &str, count: usize, view: BrowseView| {
            button(
                row![
                    text(label.to_string()).size(13),
                    Space::with_width(Length::Fill),
                    text(count.to_string()).size(13),
                ]
                .width(Length::Fill),
            )
            .width(Length::Fill)
            .style(button::secondary)
            .on_press(Message::LibraryFilter(view))
        };
        let nav = |label: String, tab: Tab| {
            button(text(label).size(14))
                .width(Length::Fill)
                .style(if self.current_tab == tab {
                    button::primary
                } else {
                    button::secondary
                })
                .on_press(Message::TabSelected(tab))
        };
        let total_size: u64 = all.iter().map(|m| m.size).sum();

        let content = column![
            row![
                Space::with_width(Length::Fill),
                // Pre-scaled 368px copy of the badge: the app was
                // shrinking the 1254px original live, which made it
                // look pixelated at the 184px display size.
                iced::widget::image(iced::widget::image::Handle::from_bytes(
                    include_bytes!("../../assets/logo_sidebar.png").to_vec(),
                ))
                .filter_method(iced::widget::image::FilterMethod::Linear)
                .width(184)
                .height(184),
                Space::with_width(Length::Fill),
            ],
            nav("Browse".to_string(), Tab::Browse),
            nav(format!("Conflicts ({})", self.conflicts.len()), Tab::Conflicts),
            nav("Configure Save (wip)".to_string(), Tab::Saves),
            nav("Log Reader".to_string(), Tab::Log),
            nav("Settings".to_string(), Tab::Settings),
            text("LIBRARY").size(11),
            lib_row("Favourites", self.mod_manager.favorites_count(), BrowseView::Favorites),
            lib_row("Mods", all.len(), BrowseView::All),
            lib_row("Maps", count_cat("Maps"), BrowseView::Category("Maps".to_string())),
            lib_row(
                "Scripts",
                count_cat("Scripts"),
                BrowseView::Category("Scripts".to_string())
            ),
            Space::with_height(Length::Fill),
            text(format!("Disk usage: {}", format_size(total_size))).size(12),
            text("V1.0.1  © 2026 Farmer Jay.")
                .size(11)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            text("Created with the Assistance of Albert")
                .size(12)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            text("Farmer Jay's Muse AI Agent.")
                .size(12)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
        ]
        .spacing(8);

        container(content)
            .width(SIDEBAR_W)
            .height(Length::Fill)
            .padding(12)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb(
                    0.09, 0.10, 0.12,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgb(0.25, 0.26, 0.30),
                    width: 1.0,
                    radius: 8.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let header = row![
            text("FS Mod Manager").size(24),
            Space::with_width(Length::Fill),
            button(text("Rescan")).on_press(Message::Scan),
            button(text("Exit")).on_press(Message::Exit),
        ]
        .spacing(10);

        let content: Element<'_, Message> = match self.current_tab {
            Tab::Browse => self.browse_view(),
            Tab::Conflicts => self.conflicts_view(),
            Tab::Saves => self.saves_view(),
            Tab::Log => self.log_view(),
            Tab::Settings => self.settings_view(),
        };

        let status = row![
            text(&self.status).size(13),
            Space::with_width(Length::Fill),
            text(format!("Favorites: {}", self.mod_manager.favorites_count())).size(13),
        ];

        // Shell: left sidebar + main column. Both are bounded to the
        // window — an unbounded root lets scrolled grid rows draw
        // over the header and pushes the status bar off-screen.
        let main = column![header, content, status]
            .spacing(10)
            .width(Length::Fill)
            .height(Length::Fill);

        row![self.sidebar_view(), main]
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
/// Classify one log.txt line for the Log Reader's coloring.
fn classify_log_line(line: &str) -> LogKind {
    let t = line.trim_start().to_lowercase();
    if t.contains("error:") || t.starts_with("error") {
        LogKind::Error
    } else if t.contains("warning:") || t.starts_with("warning") {
        LogKind::Warning
    } else {
        LogKind::Normal
    }
}

/// Background job for the Log Reader: read log.txt from the
/// profile folder, classify every line, count errors/warnings
/// over the whole file, keep the last 3000 lines for display,
/// and collect the library mods that error/warning lines name
/// (file-name stems matched against a lowercase line).
fn log_job(
    profile: PathBuf,
    stems: Vec<(String, String)>,
) -> Result<LogOutcome, String> {
    let path = profile.join("log.txt");
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("Could not read {}: {}", path.display(), e))?;
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.lines().collect();
    let total_lines = all.len();
    let mut error_count = 0usize;
    let mut warning_count = 0usize;
    let mut suspects: Vec<String> = Vec::new();
    for line in &all {
        match classify_log_line(line) {
            LogKind::Error => error_count += 1,
            LogKind::Warning => warning_count += 1,
            LogKind::Normal => continue,
        }
        let lower = line.to_lowercase();
        for (stem, name) in &stems {
            if lower.contains(stem.as_str()) && !suspects.contains(name) {
                suspects.push(name.clone());
            }
        }
    }
    suspects.sort();
    let start = total_lines.saturating_sub(3000);
    let lines = all[start..]
        .iter()
        .map(|l| LogLine {
            text: l.to_string(),
            kind: classify_log_line(l),
        })
        .collect();
    Ok(LogOutcome {
        lines,
        suspects,
        error_count,
        warning_count,
        total_lines,
    })
}

fn icon_job(jobs: Vec<(String, PathBuf, Option<String>)>) -> Vec<(String, LoadedIcon)> {
    jobs.par_iter()
        .filter_map(|(id, path, icon)| {
            load_icon(path, icon.as_deref()).map(|loaded| (id.clone(), loaded))
        })
        .collect()
}

pub fn run(mod_manager: ModManager) -> anyhow::Result<()> {
    // Window/taskbar icon from John's Playground badge (the exe's
    // file icon is embedded separately by build.rs on Windows).
    let icon = iced::window::icon::from_file_data(
        include_bytes!("../../assets/icon.png"),
        None,
    )
    .ok();
    let result = iced::application("FS Mod Manager", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_: &App| Theme::Dark)
        .window(iced::window::Settings {
            icon,
            ..iced::window::Settings::default()
        })
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

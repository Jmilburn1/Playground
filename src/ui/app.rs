use crate::mod_manager::conflict::{Conflict, ConflictDetector};
use crate::mod_manager::icons::{load_icon, LoadedIcon};
use crate::mod_manager::saves::SaveSlot;
use crate::mod_manager::types::{Mod, ModState};
use crate::mod_manager::ModManager;
use crate::ui::components::{format_size, mod_card, BrowseView, Message, ScanOutcome, Tab};
use iced::widget::image::Handle as ImageHandle;
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input, Space};
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
            just_toggled_favorite: false,
            save_slots_cache: Vec::new(),
            saves_selected: None,
            saves_draft: HashSet::new(),
            saves_filter: String::new(),
            saves_arm_apply: false,
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
                iced::widget::image(iced::widget::image::Handle::from_bytes(
                    include_bytes!("../../assets/logo.png").to_vec(),
                ))
                .width(184)
                .height(184),
                Space::with_width(Length::Fill),
            ],
            nav("Browse".to_string(), Tab::Browse),
            nav(format!("Conflicts ({})", self.conflicts.len()), Tab::Conflicts),
            nav("Configure Save (wip)".to_string(), Tab::Saves),
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

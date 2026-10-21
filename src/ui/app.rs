use crate::mod_manager::conflict::{Conflict, ConflictDetector, ConflictType};
use crate::mod_manager::game_files;
use crate::mod_manager::icons::{
    icon_cache_dir, icon_cache_key, load_icon_cached, sweep_icon_cache, LoadedIcon,
};
use crate::mod_manager::saves::SaveSlot;
use crate::mod_manager::types::{Mod, ModState};
use crate::mod_manager::ModManager;
use crate::sources::github::GithubSource;
use crate::ui::components::update_types::{UpdateInfo, UpdateState};
use crate::ui::theme;
use crate::update_db::{self, UpdateRow};
use crate::ui::components::{
    format_size, mod_card, BrowseView, FolderTarget, LoadEvent, LoadOutcome,
    LoadPhase, LogKind, LogLine, LogOutcome, Message, ScanOutcome, Tab,
    UpdateDbPresentation, UpdateJudgement,
};
use futures::SinkExt as _;
use iced::widget::image::Handle as ImageHandle;
use iced::widget::{
    button, checkbox, column, container, progress_bar, row, scrollable, stack,
    text, text_input, Space,
};
use iced::{Element, Length, Size, Subscription, Task, Theme};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const CARD_H: f32 = 258.0;
/// Splash window size, px (John, 2026-10-21): the logo at
/// full native 1254 overflowed his screen — half cut off.
/// His sizing call, measured off his own grid: the splash
/// should run about 516 x 392 — roughly 4 grid squares.
/// The logo is square art, so it draws at the window's
/// height (392) and centers, the desktop showing through
/// either side of it.
const SPLASH_W: f32 = 516.0;
const SPLASH_H: f32 = 392.0;
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
    /// True from launch until the first load finishes both its
    /// phases: while it holds, the splash owns the window and
    /// the main UI isn't rendered at all. Later loads (Rescan,
    /// folder changes) never set it again — they report
    /// progress through the status line like before.
    booting: bool,
    /// The running load's progress, fed by LoadEvent ticks:
    /// which phase is working and its real counts.
    load_phase: LoadPhase,
    load_done: usize,
    load_total: usize,
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
    /// Update checks (GitHub): per-mod check state by mod id, and
    /// the details panel's repo link field.
    update_status: HashMap<String, UpdateState>,
    /// Update database (the reader): rows by stem, read
    /// silently from catalog-home.json at every scan load — the
    /// launch answers come from the last fill, never from a
    /// click-time crawl. No database file yet = no rows, and
    /// the app simply has nothing to present.
    db_rows: HashMap<String, UpdateRow>,
    db_generated_at: Option<u64>,
    db_summary: update_db::DbSummary,
    /// Mod ids whose current update state came from the
    /// database. A rescan re-applies the database, and this
    /// set keeps that from overwriting a fresher live-check
    /// result from the same session — every live handler
    /// removes its id from the set when it writes a state.
    db_applied: HashSet<String>,
    link_input: String,
    modhub_input: String,
    /// Chunked ModHub detection pass state: remaining
    /// (stem, file name, title) jobs and running tallies.
    /// Set by a user-pressed Rescan (sidebar or Settings):
    /// when that scan lands, the ModHub detection pass
    /// follows automatically. Launch loads leave it false.
    detect_after_scan: bool,
    detect_jobs: Vec<(String, String, String)>,
    detect_total: usize,
    detect_done: usize,
    detect_found: usize,
    detect_missed: usize,
    window_size: Size,
    /// Browse grid windowing: last reported scroll offset and
    /// viewport height (see the grid build in browse_view).
    grid_offset: f32,
    grid_view_h: f32,
    /// Whether the Show: menu (custom dropdown) is open.
    show_menu_open: bool,
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
            booting: true,
            load_phase: LoadPhase::Mods,
            load_done: 0,
            load_total: 0,
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
            update_status: HashMap::new(),
            db_rows: HashMap::new(),
            db_generated_at: None,
            db_summary: update_db::DbSummary::default(),
            db_applied: HashSet::new(),
            link_input: String::new(),
            modhub_input: String::new(),
            detect_after_scan: false,
            detect_jobs: Vec::new(),
            detect_total: 0,
            detect_done: 0,
            detect_found: 0,
            detect_missed: 0,
            window_size: Size::new(1280.0, 800.0),
            grid_offset: 0.0,
            grid_view_h: 0.0,
            show_menu_open: false,
        }
    }

    /// Kick off a background library load — both phases (scan
    /// the folders, then read and judge the update database)
    /// run on a worker thread that streams real progress ticks
    /// back as it goes, then chain into icon loading when the
    /// load lands. The window stays responsive throughout,
    /// however big the library is.
    fn begin_load(&mut self) -> Task<Message> {
        self.loading = true;
        self.load_phase = LoadPhase::Mods;
        self.load_done = 0;
        self.load_total = 0;
        if !self.booting {
            self.status = "Loading mods in the background…".to_string();
        }
        let (archive, active, parallel) = self.mod_manager.scan_params();
        // A generous buffer, and progress sends that drop when
        // it's full: a tick the UI is too busy to take is a tick
        // it doesn't need — the next one is already coming. The
        // finish event is sent with a blocking send instead, so
        // it can never be dropped.
        let (tx, rx) = futures::channel::mpsc::channel::<LoadEvent>(128);
        std::thread::spawn(move || load_job(archive, active, parallel, tx));
        Task::run(rx, Message::LoadEvent)
    }

    /// Present the update database the load's second phase
    /// read and judged (see `updates_job`) — silently, locally,
    /// with no network. Runs at every load, so a fill that
    /// lands while the app is open is picked up on the next
    /// rescan.
    ///
    /// Rows were judged against the versions actually on disk,
    /// not the versions the fill recorded. A row whose source
    /// offers more becomes an installable UpdateState built
    /// from the row (the existing install flow consumes it
    /// unchanged — the "modhub:" repo marker tells it to
    /// download with the mod page as referer).
    /// Downgrade-blocked rows are NEVER turned into an offer:
    /// no state, no pill, no button — the details panel reads
    /// the row itself and explains the block.
    fn apply_update_presentation(&mut self, presentation: UpdateDbPresentation) {
        if !presentation.present {
            self.db_rows.clear();
            self.db_generated_at = None;
            self.db_summary = update_db::DbSummary::default();
            self.db_applied.clear();
            return;
        }
        self.db_generated_at = presentation.generated_at;
        self.db_rows = presentation.rows;
        self.db_summary = presentation.summary;
        for judgement in presentation.judged {
            let id = judgement.id;
            let Some(version) = self
                .mod_manager
                .get_mod(&id)
                .map(|m| m.version.clone())
            else {
                continue;
            };
            if self.update_status.contains_key(&id)
                && !self.db_applied.contains(&id)
            {
                continue; // a live result from this session stands
            }
            let row = judgement.row;
            match update_db::status_of(&version, row.latest_version.as_deref()) {
                "update_available" => {
                    if let (Some(latest), Some(url)) =
                        (row.latest_version.clone(), row.download_url.clone())
                    {
                        let (repo, tag) = if row.source_kind == "modhub" {
                            (
                                format!(
                                    "modhub:{}",
                                    row.file_id.clone().unwrap_or_default()
                                ),
                                "ModHub".to_string(),
                            )
                        } else {
                            (
                                row.source_ref.clone().unwrap_or_default(),
                                format!("v{}", latest),
                            )
                        };
                        self.update_status.insert(
                            id.clone(),
                            UpdateState::Available(UpdateInfo {
                                version: latest,
                                tag,
                                download_url: url,
                                asset_name: row.file_name.clone(),
                                repo,
                            }),
                        );
                        self.db_applied.insert(id.clone());
                    } else {
                        // The database knows a newer version
                        // exists but recorded no downloadable
                        // asset for it — visible in the details
                        // panel via the row, never a dead button.
                        self.update_status.remove(&id);
                        self.db_applied.remove(&id);
                    }
                }
                "up_to_date" => {
                    self.update_status
                        .insert(id.clone(), UpdateState::UpToDate);
                    self.db_applied.insert(id.clone());
                }
                _ => {
                    // ahead_blocked / unknown: no state, no
                    // pill, no offer. The details panel reads
                    // the row and says why.
                    self.update_status.remove(&id);
                    self.db_applied.remove(&id);
                }
            }
        }
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
                        BrowseView::Updates => matches!(
                            self.update_status.get(&m.id),
                            Some(UpdateState::Available(_))
                        ),
                        BrowseView::Category(c) => {
                            m.categories.iter().any(|x| x == c)
                        }
                    })
            })
            .collect();
        mods.sort_by_cached_key(|m| m.name.to_lowercase());
        mods
    }

    fn mod_has_conflict(&self, m: &Mod) -> bool {
        // Duplicate/version conflicts are keyed by exact file
        // name; dependency conflicts still carry display names.
        let file = m.file_name();
        self.conflicts.iter().any(|c| {
            c.mod1 == m.name || c.mod2 == m.name || c.mod1 == file || c.mod2 == file
        })
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
                    // Detection is part of Rescan now (John,
                    // 2026-10-18): when this scan lands, the
                    // ModHub detection pass follows on its own.
                    self.detect_after_scan = true;
                    return self.begin_load();
                }
            }
            Message::LoadEvent(event) => match event {
                LoadEvent::Progress { phase, done, total } => {
                    self.load_phase = phase;
                    self.load_done = done;
                    self.load_total = total;
                }
                LoadEvent::Finished(Ok(outcome)) => {
                    let count = outcome.scan.mods.len();
                    self.mod_manager.install_mods(
                        outcome
                            .scan
                            .mods
                            .into_iter()
                            .map(|m| (m.id.clone(), m))
                            .collect(),
                    );
                    self.conflicts = outcome.scan.conflicts;
                    // The reader's results, computed in the
                    // load's second phase: present them against
                    // the library just loaded — the Updates
                    // counts are already true from this moment.
                    self.apply_update_presentation(outcome.updates);
                    if let Some(id) = &self.selected_mod {
                        if self.mod_manager.get_mod(id).is_none() {
                            self.selected_mod = None;
                        }
                    }
                    // Both phases are done: the splash hands
                    // over to the main window — which first
                    // gets its ordinary shape back (decorated,
                    // 1280x800; see restore_window). (Icons
                    // keep loading behind it, as before.)
                    // The handover belongs to the boot only:
                    // a rescan's load finishing must never
                    // touch the window (the decorations
                    // toggle is stateful — firing it per load
                    // would flip the frame off again).
                    let was_booting = self.booting;
                    self.booting = false;
                    self.status = format!("Loaded {} mods — loading icons…", count);
                    let icons = self.begin_icon_load();
                    if was_booting {
                        return Task::batch(vec![
                            icons,
                            Self::restore_window(),
                            Self::verify_window_restored(),
                        ]);
                    }
                    return icons;
                }
                LoadEvent::Finished(Err(e)) => {
                    let was_booting = self.booting;
                    self.loading = false;
                    self.booting = false;
                    self.status = format!("Scan failed: {}", e);
                    if let Some(note) = self.pending_status.take() {
                        self.status = note;
                    }
                    // Even a failed boot hands over: the main
                    // window (carrying the error in its status
                    // line) gets its ordinary shape back too.
                    if was_booting {
                        return Task::batch(vec![
                            Self::restore_window(),
                            Self::verify_window_restored(),
                        ]);
                    }
                    return Task::none();
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
                if let Some(when) = self.db_generated_at {
                    self.status += &format!(
                        " Update database: {} updates available (filled {}).",
                        self.db_summary.updates,
                        fmt_time(when)
                    );
                }
                if let Some(note) = self.pending_status.take() {
                    self.status = note;
                }
                if self.detect_after_scan {
                    self.detect_after_scan = false;
                    return self.start_detect();
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
            Message::LinkInputChanged(value) => self.link_input = value,
            Message::SaveModLink(id) => {
                if let Some(m) = self.mod_manager.get_mod(&id).cloned() {
                    let stem = ModManager::mod_stem(&m.path);
                    let input = self.link_input.trim().to_string();
                    if input.is_empty() {
                        match self.mod_manager.set_mod_link(&stem, None) {
                            Ok(()) => {
                                self.update_status.remove(&id);
                                self.status =
                                    format!("Update link removed for {}.", m.name);
                            }
                            Err(e) => self.status = format!("Saving the link failed: {}", e),
                        }
                    } else if let Some(repo) = crate::sources::github::normalize_repo(&input) {
                        match self.mod_manager.set_mod_link(&stem, Some(repo.clone())) {
                            Ok(()) => {
                                self.link_input = repo.clone();
                                self.status =
                                    format!("{} linked to GitHub repo {}.", m.name, repo);
                            }
                            Err(e) => self.status = format!("Saving the link failed: {}", e),
                        }
                    } else {
                        self.status =
                            "That doesn't look like a GitHub repo — use owner/repo or the repo's URL."
                                .to_string();
                    }
                }
            }
            Message::ModHubInputChanged(value) => self.modhub_input = value,
            Message::SaveModHubLink(id) => {
                if let Some(m) = self.mod_manager.get_mod(&id).cloned() {
                    let stem = ModManager::mod_stem(&m.path);
                    let input = self.modhub_input.trim().to_string();
                    if input.is_empty() {
                        match self.mod_manager.set_modhub_link(&stem, None) {
                            Ok(()) => {
                                self.status =
                                    format!("ModHub link removed for {}.", m.name);
                            }
                            Err(e) => self.status = format!("Saving the link failed: {}", e),
                        }
                    } else if let Some(mod_id) = crate::sources::mod_hub::normalize_modhub(&input) {
                        match self.mod_manager.set_modhub_link(&stem, Some(mod_id.clone())) {
                            Ok(()) => {
                                self.modhub_input = mod_id.clone();
                                self.status = format!(
                                    "{} linked to ModHub mod {}.",
                                    m.name, mod_id
                                );
                            }
                            Err(e) => self.status = format!("Saving the link failed: {}", e),
                        }
                    } else {
                        self.status =
                            "That doesn't look like a ModHub link — paste the mod's ModHub page URL or its numeric id."
                                .to_string();
                    }
                }
            }
            Message::CheckUpdate(id) => {
                if let Some(m) = self.mod_manager.get_mod(&id).cloned() {
                    let stem = ModManager::mod_stem(&m.path);
                    let gh = self.mod_manager.mod_link(&stem).map(|s| s.to_string());
                    let mh = self.mod_manager.modhub_link(&stem).map(|s| s.to_string());
                    if gh.is_some() || mh.is_some() {
                        self.db_applied.remove(&id);
                        self.update_status.insert(id.clone(), UpdateState::Checking);
                        let (base, token) = self.mod_manager.github_endpoint();
                        let current = m.version.clone();
                        return Task::perform(
                            async move {
                                check_any_job(base, token, gh, mh, stem, current).await
                            },
                            move |result| Message::UpdateChecked(id.clone(), result),
                        );
                    }
                }
            }
            Message::UpdateChecked(id, result) => {
                self.db_applied.remove(&id);
                let name = self
                    .mod_manager
                    .get_mod(&id)
                    .map(|m| m.name.clone())
                    .unwrap_or_default();
                match result {
                    Ok(Some(info)) => {
                        self.status = format!(
                            "Update available for {}: v{}.",
                            name, info.version
                        );
                        self.update_status.insert(id, UpdateState::Available(info));
                    }
                    Ok(None) => {
                        self.status = format!("{} is up to date.", name);
                        self.update_status.insert(id, UpdateState::UpToDate);
                    }
                    Err(e) => {
                        self.status = format!("Update check failed for {}: {}", name, e);
                        self.update_status.insert(id, UpdateState::Failed(e));
                    }
                }
            }
            Message::InstallUpdate(id) => {
                let info = match self.update_status.get(&id) {
                    Some(UpdateState::Available(info)) => Some(info.clone()),
                    _ => None,
                };
                if let (Some(info), Some(m)) =
                    (info, self.mod_manager.get_mod(&id).cloned())
                {
                    self.db_applied.remove(&id);
                    self.update_status.insert(id.clone(), UpdateState::Installing);
                    self.status = format!("Downloading {} v{}…", m.name, info.version);
                    let (_base, token) = self.mod_manager.github_endpoint();
                    let stem = ModManager::mod_stem(&m.path);
                    let dest = m
                        .path
                        .parent()
                        .map(|p| p.join(format!(".playground-dl-{}.zip", stem)))
                        .unwrap_or_else(|| PathBuf::from(".playground-dl.zip"));
                    let url = info.download_url.clone();
                    if let Some(mh_id) = info.repo.strip_prefix("modhub:") {
                        // ModHub's CDN wants the mod page as the
                        // referer; the GitHub flow doesn't need one.
                        let page = crate::sources::mod_hub::page_url(mh_id);
                        let dest2 = dest.clone();
                        return Task::perform(
                            async move {
                                crate::sources::mod_hub::ModHubSource::new()
                                    .download_to(&url, &page, &dest2)
                                    .await
                                    .map(|_| dest2)
                                    .map_err(|e| e.to_string())
                            },
                            move |result| Message::UpdateInstalled(id.clone(), result),
                        );
                    }
                    return Task::perform(
                        async move {
                            let src = GithubSource::new(token);
                            src.download_to(&url, &dest)
                                .await
                                .map(|_| dest)
                                .map_err(|e| e.to_string())
                        },
                        move |result| Message::UpdateInstalled(id.clone(), result),
                    );
                }
            }
            Message::UpdateInstalled(id, result) => {
                self.db_applied.remove(&id);
                let name = self
                    .mod_manager
                    .get_mod(&id)
                    .map(|m| m.name.clone())
                    .unwrap_or_default();
                match result {
                    Ok(downloaded) => {
                        match self.mod_manager.replace_mod_file(&id, &downloaded) {
                            Ok(change) => {
                                self.update_status.insert(id.clone(), UpdateState::UpToDate);
                                self.recompute_conflicts();
                                self.status = format!(
                                    "Updated {} ({}).",
                                    name, change
                                );
                            }
                            Err(e) => {
                                self.status = format!("Installing the update for {} failed: {}", name, e);
                                self.update_status
                                    .insert(id, UpdateState::Failed(e.to_string()));
                            }
                        }
                        let _ = std::fs::remove_file(&downloaded);
                    }
                    Err(e) => {
                        self.status = format!("Download for {} failed: {}", name, e);
                        self.update_status.insert(id, UpdateState::Failed(e));
                    }
                }
                return self.begin_icon_load();
            }
            Message::DetectCatalogReady(catalog_pairs) => {
                if !self.detect_jobs.is_empty() {
                    return Task::none(); // a pass is already running
                }
                let catalog: std::collections::HashMap<String, String> =
                    catalog_pairs.into_iter().collect();
                // One job per unique .zip file stem that has no
                // ModHub link yet: (stem, file name, title).
                let mut jobs: Vec<(String, String, String)> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for m in self.mod_manager.get_mods() {
                    let stem = ModManager::mod_stem(&m.path);
                    if self.mod_manager.modhub_link(&stem).is_some()
                        || !seen.insert(stem.clone())
                    {
                        continue;
                    }
                    let Some(file_name) = m
                        .path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                    else {
                        continue;
                    };
                    if !file_name.to_lowercase().ends_with(".zip") {
                        continue;
                    }
                    jobs.push((stem, file_name, m.name.clone()));
                }
                if jobs.is_empty() {
                    self.status =
                        "Every .zip mod already has a ModHub link — nothing to detect."
                            .to_string();
                    return Task::none();
                }
                // Classify every job by the run order
                // (owner, 2026-10-20): Stevie mods are never
                // searched and mods homed in catalog-other
                // are not ModHub candidates — both leave the
                // pass here; catalog hits link instantly;
                // remembered misses are settled without a
                // search; only the residue — present in NO
                // store — goes live.
                use crate::sources::catalog_miss::Decision;
                let mut instant = 0usize;
                let mut known_missed = 0usize;
                let mut live: Vec<(String, String, String)> = Vec::new();
                for (stem, file_name, title) in jobs {
                    match self.mod_manager.detect_decision(
                        &file_name,
                        &stem,
                        catalog.contains_key(&stem),
                    ) {
                        Decision::InCatalog => {
                            let _ = self.mod_manager.set_modhub_link(
                                &stem,
                                Some(catalog[&stem].clone()),
                            );
                            self.mod_manager.forget_miss(&file_name, &stem);
                            instant += 1;
                        }
                        Decision::KnownMiss => known_missed += 1,
                        Decision::Stevie | Decision::InOther => {}
                        Decision::Search => live.push((stem, file_name, title)),
                    }
                }
                let total = instant + known_missed + live.len();
                if live.is_empty() {
                    self.status = if total == 0 {
                        "ModHub auto-detect done — nothing to search: every unlinked .zip mod is homed in catalog-other or a Stevie manual install."
                            .to_string()
                    } else if known_missed == 0 {
                        format!(
                            "ModHub auto-detect done — all {} mods linked from the catalog, no live searching needed. Run Check for updates to see what's newer.",
                            instant
                        )
                    } else if instant == 0 {
                        format!(
                            "ModHub auto-detect done — {} mods already known not to be on ModHub (remembered misses), and nothing else needed a search. Run Check for updates to see what's newer.",
                            known_missed
                        )
                    } else {
                        format!(
                            "ModHub auto-detect done — {} mods linked from the catalog, {} already known not to be on ModHub (remembered misses); no live searching needed. Run Check for updates to see what's newer.",
                            instant, known_missed
                        )
                    };
                    return Task::none();
                }
                self.detect_total = total;
                self.detect_done = instant + known_missed;
                self.detect_found = instant;
                self.detect_missed = known_missed;
                self.detect_jobs = live;
                self.status = if known_missed == 0 {
                    format!(
                        "Detecting ModHub links — {} of {} checked ({} linked instantly from the catalog). Links save every batch, so closing the app loses nothing.",
                        self.detect_done, self.detect_total, instant
                    )
                } else {
                    format!(
                        "Detecting ModHub links — {} of {} checked ({} linked instantly from the catalog, {} remembered misses skipped). Links save every batch, so closing the app loses nothing.",
                        self.detect_done, self.detect_total, instant, known_missed
                    )
                };
                return self.next_detect_batch();
            }
            Message::DetectBatchDone {
                found,
                missed,
                missed_keys,
                processed,
                throttled,
            } => {
                // Save this batch's links immediately, tally,
                // drop the processed jobs, and chain the next
                // batch (or finish).
                for (stem, id) in &found {
                    let _ = self.mod_manager.set_modhub_link(stem, Some(id.clone()));
                    // Linked after all: no miss record may
                    // shadow it.
                    if let Some((_, file_name, _)) =
                        self.detect_jobs.iter().find(|(s, _, _)| s == stem)
                    {
                        let file_name = file_name.clone();
                        self.mod_manager.forget_miss(&file_name, stem);
                    }
                }
                // Write down the true not-founds: a miss is
                // remembered like a link, so none of these
                // is ever searched again.
                for (stem, file_name) in &missed_keys {
                    self.mod_manager.record_miss(file_name, stem);
                }
                self.detect_found += found.len();
                self.detect_missed += missed;
                self.detect_done += processed;
                self.detect_jobs.drain(..processed.min(self.detect_jobs.len()));
                if throttled {
                    // THE TRIPWIRE (owner, 2026-10-19): GIANTS
                    // answered 403/429 — the pass stops here.
                    // The jobs not yet searched are dropped
                    // unsearched; the links and misses saved
                    // so far stand.
                    self.detect_jobs.clear();
                    self.status = format!(
                        "ModHub auto-detect HALTED — GIANTS throttled the pass (HTTP 403/429). {} mods linked before the halt; the rest were not searched. Links and misses already saved stand; run Rescan later to retry the rest.",
                        self.detect_found
                    );
                    self.detect_total = 0;
                    self.detect_done = 0;
                } else if self.detect_jobs.is_empty() {
                    self.status = format!(
                        "ModHub auto-detect done — {} mods linked, {} not found on ModHub. Run Check for updates to see what's newer.",
                        self.detect_found, self.detect_missed
                    );
                    self.detect_total = 0;
                    self.detect_done = 0;
                } else {
                    self.status = format!(
                        "Detecting ModHub links — {} of {} checked, {} linked so far…",
                        self.detect_done, self.detect_total, self.detect_found
                    );
                    return self.next_detect_batch();
                }
            }
            Message::ModHubLinksDetected { .. } => {
                // Superseded by the chunked batch flow; kept so
                // older in-flight tasks land harmlessly.
            }
            Message::SearchChanged(query) => {
                self.show_menu_open = false;
                self.search_query = query;
                self.grid_offset = 0.0;
                return iced::widget::scrollable::scroll_to(
                    iced::widget::scrollable::Id::new("grid"),
                    iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                );
            }
            Message::LibraryFilter(view) => {
                self.show_menu_open = false;
                self.current_tab = Tab::Browse;
                self.view_filter = view;
                self.favorites_only = false;
                self.grid_offset = 0.0;
                return iced::widget::scrollable::scroll_to(
                    iced::widget::scrollable::Id::new("grid"),
                    iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                );
            }
            Message::TabSelected(tab) => {
                self.show_menu_open = false;
                if tab == Tab::Saves {
                    self.refresh_save_slots();
                }
                self.current_tab = tab;
                if tab == Tab::Log {
                    return self.begin_log_load();
                }
            }
            Message::BrowseViewChanged(view) => {
                self.show_menu_open = false;
                self.view_filter = view;
                self.grid_offset = 0.0;
                return iced::widget::scrollable::scroll_to(
                    iced::widget::scrollable::Id::new("grid"),
                    iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                );
            }
            Message::SelectMod(id) => {
                self.show_menu_open = false;
                // A click on a card's star badge also lands here (the
                // badge is nested in the card) — the favorite toggle
                // sets this flag first, so ignore that one selection.
                if self.just_toggled_favorite {
                    self.just_toggled_favorite = false;
                } else {
                    if let Some(m) = self.mod_manager.get_mod(&id) {
                        let stem = ModManager::mod_stem(&m.path);
                        self.link_input = self
                            .mod_manager
                            .mod_link(&stem)
                            .map(|s| s.to_string())
                            .unwrap_or_default();
                        self.modhub_input = self
                            .mod_manager
                            .modhub_link(&stem)
                            .map(|s| s.to_string())
                            .unwrap_or_default();
                    }
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
            Message::ToggleShowMenu => {
                self.show_menu_open = !self.show_menu_open
            }
            Message::GridScrolled(viewport) => {
                self.grid_offset = viewport.absolute_offset().y;
                self.grid_view_h = viewport.bounds().height;
            }
        }

        Task::none()
    }

    fn tab_button(&self, label: String, tab: Tab) -> Element<'_, Message> {
        // Palette pills, like the sidebar's menu (John,
        // 2026-10-21) — this helper predates the sidebar
        // layout and currently has no callers, but it speaks
        // the same style so the two never drift apart.
        button(text(label))
            .style(theme::nav_pill(self.current_tab == tab))
            .on_press(Message::TabSelected(tab))
            .into()
    }

    fn browse_view(
        &self,
    ) -> (
        Element<'_, Message>,
        Option<Element<'_, Message>>,
        Element<'_, Message>,
    ) {
        let mods = self.filtered_mods();

        // One dropdown picks what the grid shows: every mod, one
        // folder's mods, favorites, or one detected category.
        let mut view_options = vec![
            BrowseView::All,
            BrowseView::Active,
            BrowseView::Archived,
            BrowseView::Favorites,
            BrowseView::Updates,
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
        // installed / favourited / maps / available updates. The
        // fourth tile was DISK USAGE until 2026-10-18: that
        // figure already lives in the sidebar, so John swapped
        // the tile to the update count from the database —
        // the number that changes something.
        let all_mods = self.mod_manager.get_mods();
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
            tile(
                match self.db_generated_at {
                    // The tile counts the SAME live map the
                    // Updates filter reads (John, 2026-10-20):
                    // it used to read the database summary,
                    // which is only recomputed when the
                    // database is applied, so applying an
                    // update never moved the number until a
                    // rescan or a restart. Counting the live
                    // states makes the tile drop the moment an
                    // update lands — and the tile can never
                    // disagree with the list it names again.
                    Some(_) => self
                        .update_status
                        .values()
                        .filter(|st| matches!(
                            st,
                            crate::ui::components::update_types::UpdateState::Available(_)
                        ))
                        .count()
                        .to_string(),
                    None => "–".to_string(),
                },
                "AVAILABLE UPDATES",
            ),
        ]
        .spacing(10);

        // No "Favorites only" button here (John): favorites are
        // already one pick in the Show dropdown and in the sidebar.
        // The search box lives on this row too (John) — it moved
        // down from the header to sit with the other browse controls.
        // Both boxes get rounded corners (John).
        let toolbar = row![
            text("Show:"),
            // Custom dropdown (John, 2026-10-20): the pick_list's
            // overlay menu leaked clicks through to the card under
            // the menu, popping its details open on every pick.
            // The replacement is a plain button here plus a menu
            // of real buttons drawn in the stack's top layer (see
            // the end of this function) — buttons capture their
            // own clicks, so nothing reaches the grid beneath.
            button(
                row![
                    text(format!("{}", self.view_filter)),
                    Space::with_width(Length::Fill),
                    text("\u{25be}").size(12),
                ]
                .spacing(8),
            )
            .width(200)
            .padding(8)
            .on_press(Message::ToggleShowMenu)
            .style(|_theme, status| {
                let bg = match status {
                    iced::widget::button::Status::Hovered
                    | iced::widget::button::Status::Pressed => {
                        iced::Color::from_rgb(0.45, 0.45, 0.46)
                    }
                    _ => iced::Color::from_rgb(0.396, 0.4, 0.4),
                };
                iced::widget::button::Style {
                    background: Some(iced::Background::Color(bg)),
                    text_color: iced::Color::WHITE,
                    border: iced::Border {
                        color: iced::Color::from_rgb(0.35, 0.36, 0.40),
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                }
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

            // Windowed grid (John's return-to-Browse hang,
            // 2026-10-20): the whole library's cards used to be
            // built and measured on every frame — at ~2,900 mods
            // returning to this tab froze for seconds. Cards are
            // a fixed height, so the scrollable now only builds
            // the rows around the visible window (plus a buffer)
            // and pads the rest with spacers; the total height —
            // and therefore the scrollbar — is unchanged. The
            // last viewport arrives via Message::GridScrolled.
            let rows_total = (mods.len() + cols - 1) / cols;
            let pitch = CARD_H + GRID_SPACING;
            let view_h = if self.grid_view_h > 0.0 {
                self.grid_view_h
            } else {
                (self.window_size.height - 216.0).max(CARD_H)
            };
            const BUFFER_ROWS: usize = 3;
            let first_row = ((self.grid_offset / pitch).floor() as usize)
                .saturating_sub(BUFFER_ROWS)
                .min(rows_total);
            let visible_rows = (view_h / pitch).ceil() as usize + 1;
            let end_row = (first_row + visible_rows + 2 * BUFFER_ROWS).min(rows_total);

            let mut grid = column![].spacing(GRID_SPACING);
            if first_row > 0 {
                grid = grid.push(Space::with_height(
                    first_row as f32 * pitch - GRID_SPACING,
                ));
            }
            for chunk in mods.chunks(cols).skip(first_row).take(end_row - first_row) {
                let mut r = row![].spacing(GRID_SPACING);
                for m in chunk {
                    let fav = self.mod_manager.is_favorite(&m.favorite_key());
                    let selected = self.selected_mod.as_deref() == Some(m.id.as_str());
                    let conflict = self.mod_has_conflict(m);
                    let update_available = matches!(
                        self.update_status.get(&m.id),
                        Some(crate::ui::components::update_types::UpdateState::Available(_))
                    );
                    r = r.push(mod_card(
                        m,
                        self.icons.get(&m.id),
                        fav,
                        selected,
                        conflict,
                        update_available,
                        card_w,
                        CARD_H,
                    ));
                }
                grid = grid.push(r);
            }
            if end_row < rows_total {
                grid = grid.push(Space::with_height(
                    (rows_total - end_row) as f32 * pitch - GRID_SPACING,
                ));
            }
            scrollable(grid)
                .id(iced::widget::scrollable::Id::new("grid"))
                .on_scroll(Message::GridScrolled)
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

        // The Show: menu, when open: real buttons in a row set
        // into the stack's top layer by view() (outside the
        // header zone's opaque slab, so the grid shows around
        // it). Aligned under the field button: the "Show:" label
        // plus the toolbar spacing puts the field at x = 52.
        let menu: Option<Element<'_, Message>> = if self.show_menu_open {
            let option_count = view_options.len();
            let mut options_col = column![].spacing(0).width(200);
            for opt in view_options {
                let selected = opt == self.view_filter;
                options_col = options_col.push(
                    button(text(format!("{}", opt)))
                        .width(200)
                        .padding([6, 10])
                        .on_press(Message::BrowseViewChanged(opt))
                        .style(move |_theme, status| {
                            let bg = if selected {
                                iced::Color::from_rgb(0.2, 0.349, 0.855)
                            } else {
                                match status {
                                    iced::widget::button::Status::Hovered
                                    | iced::widget::button::Status::Pressed => {
                                        iced::Color::from_rgb(0.45, 0.45, 0.46)
                                    }
                                    _ => iced::Color::from_rgb(0.396, 0.4, 0.4),
                                }
                            };
                            iced::widget::button::Style {
                                background: Some(iced::Background::Color(bg)),
                                text_color: iced::Color::WHITE,
                                ..Default::default()
                            }
                        }),
                );
            }
            let menu_inner: Element<'_, Message> =
                if option_count > 12 {
                    scrollable(options_col).height(336).into()
                } else {
                    options_col.into()
                };
            Some(
                row![
                    Space::with_width(52),
                    container(menu_inner).style(|_| container::Style {
                        background: Some(iced::Background::Color(
                            iced::Color::from_rgb(0.396, 0.4, 0.4),
                        )),
                        border: iced::Border {
                            color: iced::Color::from_rgb(0.35, 0.36, 0.40),
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    }),
                ]
                .into(),
            )
        } else {
            None
        };

        // The tiles/toolbar and the body are returned separately:
        // view() draws them in different stack layers (see there).
        (
            column![tiles, toolbar].spacing(8).into(),
            menu,
            body,
        )
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

        let my_file = m.file_name();
        let my_conflicts: Vec<&Conflict> = self
            .conflicts
            .iter()
            .filter(|c| {
                c.mod1 == m.name || c.mod2 == m.name || c.mod1 == my_file || c.mod2 == my_file
            })
            .collect();
        if !my_conflicts.is_empty() {
            col = col.push(text("Conflicts:").size(13));
            for c in my_conflicts {
                let other = if c.mod1 == m.name || c.mod1 == my_file {
                    &c.mod2
                } else {
                    &c.mod1
                };
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

        // Updates (GitHub): link this mod to its repo, check the
        // latest release, and install over the old file in place.
        let stem = ModManager::mod_stem(&m.path);
        let linked = self.mod_manager.mod_link(&stem).map(|s| s.to_string());
        // Update database: what the last fill recorded for
        // this mod — its source and the version direction,
        // read straight from the row. This is the substance
        // behind the card's pill; the downgrade block shows
        // here as an explanation, never as an offer.
        if let Some(row) = self.db_rows.get(&stem) {
            col = col.push(text("Update database:").size(13));
            let source_line = match row.source_kind.as_str() {
                "modhub" => match &row.file_id {
                    Some(fid) => format!("Source: ModHub (mod {})", fid),
                    None => "Source: ModHub".to_string(),
                },
                "github" => format!(
                    "Source: GitHub — {}",
                    row.source_ref.clone().unwrap_or_default()
                ),
                "kingmods" => format!(
                    "Source: KingMods — {}",
                    row.source_ref.clone().unwrap_or_default()
                ),
                "itch" => format!(
                    "Source: itch.io — {}",
                    row.source_ref.clone().unwrap_or_default()
                ),
                _ => "Source: none recorded".to_string(),
            };
            col = col.push(text(source_line).size(12));
            if let Some(latest) = &row.latest_version {
                col = col.push(
                    text(format!(
                        "Installed v{} → latest v{}",
                        m.version, latest
                    ))
                    .size(12),
                );
                let note = match update_db::status_of(&m.version, Some(latest.as_str())) {
                    "update_available" => {
                        "Update available (from the database)."
                    }
                    "ahead_blocked" => {
                        "Installed is newer than the source — update blocked; a downgrade is never offered."
                    }
                    "up_to_date" => "Up to date (per the database).",
                    _ => "",
                };
                if !note.is_empty() {
                    col = col.push(text(note).size(12));
                }
                if update_db::is_changed(row) {
                    col = col.push(
                        text("Latest version changed at the last fill.").size(12),
                    );
                }
            } else if row.source_kind != "none" {
                col = col.push(
                    text("Source recorded; no version on file yet.").size(12),
                );
            }
            if let Some(when) = self.db_generated_at {
                col = col.push(
                    text(format!("Database filled {}.", fmt_time(when))).size(12),
                );
            }
        } else if let Some(other) = self
            .mod_manager
            .other_row_for_file(&m.file_name())
            .or_else(|| self.mod_manager.other_row(&stem))
        {
            // No fill has recorded this mod yet, but the
            // catalog-other store knows its home — show the
            // link straight from the store, no pasting.
            let kind_label = match other.source_kind.as_str() {
                "github" => "GitHub",
                "kingmods" => "KingMods",
                "itch" => "itch.io",
                other_kind => other_kind,
            };
            col = col.push(text("Other site:").size(13));
            col = col.push(
                text(format!("Source: {} — {}", kind_label, other.source_ref)).size(12),
            );
            col = col.push(
                text("Not in the update database yet — it lands at the next fill.").size(12),
            );
        }
        col = col.push(text("Updates (GitHub):").size(13));
        col = col.push(
            row![
                text_input("owner/repo or repo URL", &self.link_input)
                    .on_input(Message::LinkInputChanged)
                    .size(13),
                button(text(if linked.is_some() { "Save link" } else { "Link" }).size(13))
                    .on_press(Message::SaveModLink(m.id.clone())),
            ]
            .spacing(6),
        );
        // Updates (ModHub): link by page URL or numeric mod_id.
        // The check button below serves both sources.
        let mh_linked = self
            .mod_manager
            .modhub_link(&stem)
            .map(|s| s.to_string());
        col = col.push(text("Updates (ModHub):").size(13));
        col = col.push(
            row![
                text_input("ModHub page URL or mod id", &self.modhub_input)
                    .on_input(Message::ModHubInputChanged)
                    .size(13),
                button(text(if mh_linked.is_some() { "Save link" } else { "Link" }).size(13))
                    .on_press(Message::SaveModHubLink(m.id.clone())),
            ]
            .spacing(6),
        );
        if linked.is_some() || mh_linked.is_some() {
            let state_line = match self.update_status.get(&m.id) {
                Some(UpdateState::Checking) => "Checking for updates…".to_string(),
                Some(UpdateState::UpToDate) => "Up to date.".to_string(),
                Some(UpdateState::Available(info)) => {
                    format!("Update available: {} ({})", info.version, info.tag)
                }
                Some(UpdateState::Installing) => "Downloading and installing…".to_string(),
                Some(UpdateState::Failed(e)) => format!("Check failed: {}", e),
                None => "Not checked yet.".to_string(),
            };
            col = col.push(text(state_line).size(12));
            col = col.push(
                button(text("Check for update"))
                    .width(Length::Fill)
                    .on_press(Message::CheckUpdate(m.id.clone())),
            );
            if let Some(UpdateState::Available(info)) = self.update_status.get(&m.id) {
                let label = format!("Download && install v{}", info.version);
                col = col.push(
                    button(text(label))
                        .width(Length::Fill)
                        .on_press(Message::InstallUpdate(m.id.clone())),
                );
            }
            if m.mod_type != crate::mod_manager::types::ModType::Zip {
                col = col.push(
                    text("Update installs work on .zip mods; this is a folder mod.").size(12),
                );
            }
        } else if let Some(UpdateState::Available(info)) =
            self.update_status.get(&m.id)
        {
            // Not linked by hand, but the update database
            // knows this mod's source and a newer version —
            // the install runs off the database row.
            let info = info.clone();
            col = col.push(
                text(format!(
                    "Update available: {} ({}) — from the update database",
                    info.version, info.tag
                ))
                .size(12),
            );
            let label = format!("Download && install v{}", info.version);
            col = col.push(
                button(text(label))
                    .width(Length::Fill)
                    .on_press(Message::InstallUpdate(m.id.clone())),
            );
            if m.mod_type != crate::mod_manager::types::ModType::Zip {
                col = col.push(
                    text("Update installs work on .zip mods; this is a folder mod.").size(12),
                );
            }
        } else {
            col = col.push(
                text("Link this mod to its GitHub repo to check it for updates.").size(12),
            );
        }
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

        // Groups are keyed by the conflict's mod1, which for
        // duplicate/version conflicts is now the EXACT file
        // name — copies are the loaded entries with that file
        // name, never every mod sharing a title.
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
                .filter(|m| m.file_name() == *name)
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
            let title = copies
                .first()
                .map(|m| m.name.clone())
                .unwrap_or_else(|| name.clone());
            let mut card_col = column![
                row![
                    header_icon,
                    column![
                        text(title).size(15),
                        text(format!("{} copies - {}", copies.len(), name)).size(12),
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
                text("Savegames appear here once FS25 has written its profile folder (Documents/My Games/FarmingSimulator2025).")
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
                text("The game log appears here once FS25 has written log.txt in its profile folder (Documents/My Games/FarmingSimulator2025).")
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
        // The sidebar's menu wears the palette now (John,
        // 2026-10-21: pill-shaped buttons, and "the color is
        // bad currently"): every row below is a pill from
        // ui::theme — the library row for the view on screen
        // and the current page's nav pill wear the accent,
        // the rest sit in petrol.
        let lib_row = |label: &str, count: usize, view: BrowseView| {
            let selected =
                self.current_tab == Tab::Browse && self.view_filter == view;
            button(
                row![
                    text(label.to_string()).size(13),
                    Space::with_width(Length::Fill),
                    text(count.to_string()).size(13),
                ]
                .width(Length::Fill),
            )
            .width(Length::Fill)
            .style(theme::nav_pill(selected))
            .on_press(Message::LibraryFilter(view))
        };
        let nav = |label: String, tab: Tab| {
            button(text(label).size(14))
                .width(Length::Fill)
                // The current page's pill is the accent
                // (John, 2026-10-21) — supersedes the 2026-
                // 10-18 all-grey ruling: in the palette the
                // accent is quiet enough to mark the page
                // without shouting, and Rescan (above) is
                // told apart by wearing the warning red.
                .style(theme::nav_pill(self.current_tab == tab))
                .on_press(Message::TabSelected(tab))
        };
        // The update count used to live here too, until the
        // stat tiles took it over (John, 2026-10-18): one
        // home per number — updates in the tiles, disk
        // usage here.
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
            // Rescan leads the sidebar (John, 2026-10-18) —
            // and detection rides along with every rescan
            // now (see Message::Scan). It wears the danger
            // pill (John, 2026-10-21): red, to warn against
            // the one heavy action and stand out at the same
            // time. There is no Browse pill (John, 2026-10-
            // 21): the LIBRARY rows below ARE the grid's
            // navigation — Mods was working overtime doing
            // Browse's job, so it holds the job officially
            // now, from its place under LIBRARY.
            button(text("Rescan").size(14))
                .width(Length::Fill)
                .style(theme::danger_pill())
                .on_press(Message::Scan),
            nav(format!("Conflicts ({})", self.conflicts.len()), Tab::Conflicts),
            nav("Configure Save (wip)".to_string(), Tab::Saves),
            // Favourites sits in the features block, under
            // Configure Save (John, 2026-10-21 — moved out
            // of LIBRARY, brought down two): it is a view of
            // the library, reached like a feature, and it
            // lights when that view is on screen.
            lib_row("Favourites", self.mod_manager.favorites_count(), BrowseView::Favorites),
            nav("Log Reader".to_string(), Tab::Log),
            nav("Settings".to_string(), Tab::Settings),
            text("LIBRARY").size(11),
            lib_row("Mods", all.len(), BrowseView::All),
            lib_row("Maps", count_cat("Maps"), BrowseView::Category("Maps".to_string())),
            lib_row(
                "Scripts",
                count_cat("Scripts"),
                BrowseView::Category("Scripts".to_string())
            ),
            // Disk usage rides with the library block, right
            // under the Scripts row (John, 2026-10-18) — the
            // empty stretch belongs between it and the
            // copyright footer pinned at the bottom.
            text(format!("Disk usage: {}", format_size(total_size))).size(12),
            Space::with_height(Length::Fill),
            text("V1.3.1  © 2026 Farmer Jay.")
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
            .style(theme::sidebar_container)
            .into()
    }

    /// Kick off the next chunk of the ModHub detection pass:
    /// up to 25 mods, checked with the shared finder. The
    /// DetectBatchDone handler saves the batch's links and
    /// chains the next chunk, so progress is never lost.
    fn next_detect_batch(&mut self) -> Task<Message> {
        // Small batches on purpose: the counter ticks every
        // few seconds, so the pass visibly breathes instead of
        // sitting at 0 while a big batch chews.
        const BATCH: usize = 8;
        let batch: Vec<(String, String, String)> =
            self.detect_jobs.iter().take(BATCH).cloned().collect();
        if batch.is_empty() {
            return Task::none();
        }
        Task::perform(
            async move {
                // Several mods in flight at once (narrow
                // per-mod candidate width keeps the total
                // request pressure browser-like), tallied per
                // group so partial progress still reports.
                let src = crate::sources::mod_hub::ModHubSource::new();
                let mut found = Vec::new();
                let mut missed = 0usize;
                let mut missed_keys: Vec<(String, String)> = Vec::new();
                let mut throttled = false;
                for group in batch.chunks(4) {
                    let results = futures::future::join_all(
                        group
                            .iter()
                            .map(|(stem, file_name, title)| {
                                let src = &src;
                                async move {
                                    (
                                        stem.clone(),
                                        file_name.clone(),
                                        src.find_mod_id(file_name, title, 2).await,
                                    )
                                }
                            }),
                    )
                    .await;
                    for (stem, file_name, res) in results {
                        match res {
                            Ok(Some(id)) => found.push((stem, id)),
                            // Searched, and ModHub had
                            // nothing: a true miss, written
                            // down so it is never searched
                            // again.
                            Ok(None) => {
                                missed += 1;
                                missed_keys.push((stem, file_name));
                            }
                            // A failed search found nothing
                            // out: tallied as missed, as
                            // always, but NOT written down —
                            // the next pass retries it. A
                            // THROTTLE failure (403/429) is
                            // the tripwire: it also raises
                            // the flag that stops the pass.
                            Err(e) => {
                                missed += 1;
                                if crate::sources::throttle::is_throttled(&e) {
                                    throttled = true;
                                }
                            }
                        }
                    }
                }
                (found, missed, missed_keys, batch.len(), throttled)
            },
            |(found, missed, missed_keys, processed, throttled)| Message::DetectBatchDone {
                found,
                missed,
                missed_keys,
                processed,
                throttled,
            },
        )
    }

    /// The ModHub detection pass — formerly the header's
    /// Detect button. Since John's 2026-10-18 ruling it is
    /// part of Rescan: it starts from IconsLoaded whenever
    /// the scan that just finished was user-triggered.
    fn start_detect(&mut self) -> Task<Message> {
        // Run order, step 1 (owner, 2026-10-20): reconcile
        // the miss store against catalog-other — a mod
        // that gained a link there loses its miss record.
        // Runs every time the detect path opens, whether
        // or not a search follows.
        self.mod_manager.prune_misses();
        if !self.detect_jobs.is_empty() {
            self.status =
                "ModHub detection is already running — watch the counter; every batch is saved as it lands."
                    .to_string();
            return Task::none();
        }
        // Consult the catalog first: one download
        // answers most mods instantly, and only the
        // stems it doesn't know fall through to live
        // searching.
        self.status = "Checking the ModHub catalog…".to_string();
        let url = self.mod_manager.catalog_url();
        Task::perform(
            async move {
                let pairs = match crate::sources::catalog::ensure(&url).await {
                    Some(cat) => cat
                        .mods
                        .iter()
                        .map(|e| (e.stem.clone(), e.modhub_id.clone()))
                        .collect(),
                    None => Vec::new(),
                };
                pairs
            },
            Message::DetectCatalogReady,
        )
    }

    /// Hand the window back its ordinary shape when the
    /// boot ends. The launch window is borderless (see
    /// `run`), so: re-decorate it, and resize it to the
    /// app's standard 1280x800. Both are runtime window
    /// actions Iced 0.13 implements — the winit shell
    /// answers ToggleDecorations with
    /// `set_decorations(!is_decorated())` and Resize with
    /// `request_inner_size` — and nothing toggles the
    /// decorations between launch and this call, so the
    /// toggle lands deterministically on decorated.
    /// Transparency needs no undo: it is a creation-time
    /// property (window::Settings::transparent), and the
    /// moment `booting` drops, the theme closure in `run`
    /// returns Theme::Dark again and the shell's clear
    /// color goes back to Dark's opaque background — no
    /// trace of the clear window remains in the main one.
    /// The splash launched centered (John, 2026-10-21), so
    /// the main window is re-centered on the same point:
    /// inheriting the splash's top-left would push a
    /// 1280x800 window off the right and bottom edges.
    ///
    /// STRUCTURE IS THE FIX (field bug, 2026-10-21: every
    /// launch on the owner's Windows stayed splash-shaped
    /// and borderless with the main UI inside it): the
    /// frame and size are built DIRECTLY in get_latest's
    /// continuation — the shell answers GetLatest
    /// unconditionally, so that continuation always runs.
    /// They must never again be built downstream of
    /// `window::get_position`: it is a oneshot task, and
    /// if the shell's answer never arrives the chained
    /// continuation silently never executes (the runtime's
    /// oneshot stream ends with no output), taking the
    /// whole restore with it and logging nothing. The
    /// centering move therefore rides its own independent
    /// chain (see center_main_window): its worst case is
    /// a main window that keeps the splash's top-left —
    /// never a window that keeps the splash's shape.
    fn restore_window() -> Task<Message> {
        iced::window::get_latest().then(|id| match id {
            Some(id) => Task::batch(vec![
                iced::window::toggle_decorations(id),
                iced::window::resize(id, Size::new(1280.0, 800.0)),
                Self::center_main_window(id),
            ]),
            None => Task::none(),
        })
    }

    /// Re-center the (now 1280x800) main window on the
    /// splash's center point. Independent of the frame
    /// restore by design: the position read is the one
    /// fallible step in the handover, so only the move is
    /// allowed to ride on it. A lost answer costs the
    /// centering and nothing else.
    fn center_main_window(id: iced::window::Id) -> Task<Message> {
        iced::window::get_position(id).then(move |pos| match pos {
            Some(pos) => {
                let center_x = pos.x + SPLASH_W / 2.0;
                let center_y = pos.y + SPLASH_H / 2.0;
                iced::window::move_to(
                    id,
                    iced::Point::new(center_x - 640.0, center_y - 400.0),
                )
            }
            None => Task::none(),
        })
    }

    /// One-shot safety net behind restore_window, run
    /// once per boot handover: read the window's size
    /// back, and if it is still exactly the splash's size
    /// the restore batch demonstrably never landed, so
    /// issue it once more. Any other size — restored, or
    /// already resized by hand — leaves the window
    /// strictly alone; in particular the stateful
    /// decorations toggle is only ever re-issued when
    /// the coupled resize is provably missing too, so
    /// the net cannot flip a restored window's frame
    /// back off. There is no retry loop: one check, at
    /// most one re-issue, per boot.
    fn verify_window_restored() -> Task<Message> {
        iced::window::get_latest().then(|id| match id {
            Some(id) => iced::window::get_size(id).then(move |size| {
                if Self::still_splash_sized(size) {
                    Task::batch(vec![
                        iced::window::toggle_decorations(id),
                        iced::window::resize(id, Size::new(1280.0, 800.0)),
                        Self::center_main_window(id),
                    ])
                } else {
                    Task::none()
                }
            }),
            None => Task::none(),
        })
    }

    /// The safety net's trigger, kept pure for testing:
    /// the window still wears the splash's exact size
    /// (within a pixel — the shell reports logical size,
    /// so a landed resize reads exactly 1280x800).
    fn still_splash_sized(size: Size) -> bool {
        (size.width - SPLASH_W).abs() < 1.0 && (size.height - SPLASH_H).abs() < 1.0
    }

    /// The launch splash (John, 2026-10-21): the Playground
    /// logo itself at his measured splash size (SPLASH_W x
    /// SPLASH_H — full native 1254 overflowed his screen),
    /// floating
    /// on the desktop in its own shape — the window is
    /// launched borderless and transparent (see `run`) and
    /// the shell wears the transparent splash theme while
    /// `booting` holds (see ui::theme::splash_theme), so
    /// nothing shows but the artwork and the readout laid
    /// over its lower edge: the version in light steel, the
    /// bar steel-teal on petrol, the phase counts in pale
    /// ice, all from the palette in ui::theme. It owns the
    /// window until the first load finishes BOTH phases —
    /// mods, then updates judged — so the main window never
    /// opens onto numbers that are still settling.
    fn splash_view(&self) -> Element<'_, Message> {
        let logo = iced::widget::image(iced::widget::image::Handle::from_bytes(
            include_bytes!("../../assets/logo.png").to_vec(),
        ))
        .filter_method(iced::widget::image::FilterMethod::Linear)
        .width(SPLASH_H)
        .height(SPLASH_H);
        let label = match self.load_phase {
            LoadPhase::Mods => "Loading mods…",
            LoadPhase::Updates => "Loading updates…",
        };
        let counts = if self.load_total > 0 {
            format!("{}  {} / {}", label, self.load_done, self.load_total)
        } else {
            label.to_string()
        };
        let value = if self.load_total > 0 {
            self.load_done as f32 / self.load_total as f32
        } else {
            0.0
        };
        // The readout rides the logo's lower edge: mostly
        // clear glass and dark art down there, so the pale
        // counts read cleanly with no plate behind them.
        let readout = column![
            text(format!("Playground v{}", env!("CARGO_PKG_VERSION")))
                .size(15)
                .color(theme::TEXT_DIM),
            progress_bar(0.0..=1.0, value)
                .width(260)
                .height(14)
                .style(theme::splash_bar),
            text(counts).size(16).color(theme::TEXT),
        ]
        .spacing(10)
        .align_x(iced::Alignment::Center);
        let overlay = column![
            Space::with_height(Length::Fill),
            readout,
            Space::with_height(18),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::Alignment::Center);
        // The square logo centers in the wider window:
        // Fill spaces either side let the desktop show
        // through where the artwork isn't.
        let stage = row![
            Space::with_width(Length::Fill),
            logo,
            Space::with_width(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        stack![stage, overlay]
            .width(SPLASH_W)
            .height(SPLASH_H)
            .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        if self.booting {
            return self.splash_view();
        }
        // Header (John, 2026-10-18): his FS25 banner alone —
        // no text beside it, and no buttons at all. The old
        // header actions are gone by his ruling: Detect is
        // folded into Rescan, Check for updates was
        // superseded by the update-database reader, Exit by
        // the window's own close button, and Rescan moved to
        // the sidebar above Browse. The banner names the
        // game this build manages — when Game Switching
        // lands, this is where the current game will show.
        // Pre-scaled to 2x its 40px display height (the
        // sidebar badge's live-shrink pixelation lesson).
        let header = iced::widget::image(iced::widget::image::Handle::from_bytes(
            include_bytes!("../../assets/fs25_banner.png").to_vec(),
        ))
        .filter_method(iced::widget::image::FilterMethod::Linear)
        .height(40);

        // The main area is a stack of two columns — the scroll-clip
        // fix (John, live, 2026-10-18: card ICONS slid over the
        // toolbar and the status bar while the card boxes clipped
        // correctly). Root cause, proven in a local repro: in iced
        // 0.13's renderers, image and mesh primitives inside a
        // scrollable are not cut by the scrollable's clip the way
        // quads and text are — they paint past its edges at their
        // natural positions. So the tab content lives in the base
        // layer with empty holes where the header zone and status
        // bar sit, and the header zone (banner + the Browse tiles
        // and toolbar) and the status bar are drawn in the stack's
        // top layer: a stack child after the first gets its own
        // layer, painted over any escaped pixels, and opaque
        // window-colored slabs behind them close their gaps too.
        // The hole heights match the top layer's fixed segments:
        // header 40 + spacing 10 (+ Browse tiles 63 + spacing 8 +
        // toolbar 36 + spacing 8) at the top, status 17 + spacing
        // 10 at the bottom; the slabs' extra padding (8 bottom /
        // 10 top) covers the spacing gaps at the grid's edges.
        let (top_controls, menu, content, top_hole): (
            Element<'_, Message>,
            Option<Element<'_, Message>>,
            Element<'_, Message>,
            f32,
        ) = match self.current_tab {
            Tab::Browse => {
                let (top, menu, body) = self.browse_view();
                (top, menu, body, 165.0)
            }
            Tab::Conflicts => (
                Space::with_height(0).into(),
                None,
                self.conflicts_view(),
                50.0,
            ),
            Tab::Saves => (
                Space::with_height(0).into(),
                None,
                self.saves_view(),
                50.0,
            ),
            Tab::Log => (Space::with_height(0).into(), None, self.log_view(), 50.0),
            Tab::Settings => (
                Space::with_height(0).into(),
                None,
                self.settings_view(),
                50.0,
            ),
        };

        let status = row![
            text(&self.status).size(13),
            Space::with_width(Length::Fill),
            text(format!("Favorites: {}", self.mod_manager.favorites_count())).size(13),
        ];

        let window_bg = iced::Color::from_rgb(0.125, 0.133, 0.145);
        let base = column![
            Space::with_height(top_hole),
            content,
            Space::with_height(27),
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        let top_layer = column![
            container(column![header, top_controls].spacing(10))
                .padding(iced::Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: 8.0,
                    left: 0.0,
                })
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(window_bg)),
                    ..Default::default()
                }),
            // The Show: menu floats here when open (Browse only):
            // its own buttons over a transparent row, so the grid
            // stays visible and clickable everywhere it doesn't
            // cover.
            match menu {
                Some(m) => m,
                None => Space::with_height(0).into(),
            },
            Space::with_height(Length::Fill),
            container(status)
                .padding(iced::Padding {
                    top: 10.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                })
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(window_bg)),
                    ..Default::default()
                }),
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        let main = iced::widget::stack![base, top_layer]
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

/// One GitHub update check: the repo's latest release, resolved
/// to an installable update when it is newer than the mod's
/// current version. Ok(None) means "no update" (no releases, or
/// already current); a newer release with no .zip asset is an
/// error the user should see, not a silent "up to date".
async fn check_with(
    src: &GithubSource,
    base: &str,
    repo: &str,
    stem: &str,
    current: &str,
) -> Result<Option<UpdateInfo>, String> {
    let release = src
        .latest_release(base, repo)
        .await
        .map_err(|e| e.to_string())?;
    let Some(release) = release else {
        return Ok(None);
    };
    if !crate::sources::github::is_newer(&release.version, current) {
        return Ok(None);
    }
    match crate::sources::github::pick_asset(&release, stem) {
        Some(asset) => Ok(Some(UpdateInfo {
            version: release.version.clone(),
            tag: release.tag.clone(),
            download_url: asset.browser_download_url.clone(),
            asset_name: asset.name.clone(),
            repo: repo.to_string(),
        })),
        None => Err(format!(
            "Release {} is newer but has no .zip asset to install",
            release.tag
        )),
    }
}

/// One ModHub update check: the mod's page, resolved to an
/// installable update when ModHub's version is newer than the
/// mod's current version. Ok(None) means "no update" (page
/// gone, or already current). The UpdateInfo's repo carries a
/// "modhub:" prefix so the install step knows to download with
/// the mod page as referer.
async fn check_modhub_with(
    src: &crate::sources::mod_hub::ModHubSource,
    mod_id: &str,
    current: &str,
) -> Result<Option<UpdateInfo>, String> {
    let info = src
        .fetch_info(mod_id)
        .await
        .map_err(|e| e.to_string())?;
    let Some(info) = info else {
        return Ok(None);
    };
    if !crate::sources::github::is_newer(&info.version, current) {
        return Ok(None);
    }
    Ok(Some(UpdateInfo {
        version: info.version.clone(),
        tag: "ModHub".to_string(),
        download_url: info.download_url.clone(),
        asset_name: info.file_name.clone(),
        repo: format!("modhub:{}", mod_id),
    }))
}


/// Background-task wrapper for one mod's combined update
/// check: GitHub first when linked, then ModHub; the first
/// source reporting an update wins.
async fn check_any_job(
    base: String,
    token: Option<String>,
    gh_repo: Option<String>,
    mh_id: Option<String>,
    stem: String,
    current: String,
) -> Result<Option<UpdateInfo>, String> {
    if let Some(repo) = &gh_repo {
        let src = GithubSource::new(token.clone());
        if let Some(info) = check_with(&src, &base, repo, &stem, &current).await? {
            return Ok(Some(info));
        }
    }
    if let Some(id) = &mh_id {
        let src = crate::sources::mod_hub::ModHubSource::new();
        if let Some(info) = check_modhub_with(&src, id, &current).await? {
            return Ok(Some(info));
        }
    }
    Ok(None)
}

/// "Oct 18, 11:31 AM" — a database fill time the way John
/// reads clocks: 12-hour, local.
fn fmt_time(secs: u64) -> String {
    match chrono::DateTime::from_timestamp(secs as i64, 0) {
        Some(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%b %d, %I:%M %p")
            .to_string(),
        None => "an unknown time".to_string(),
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

/// Background job for one full library load, run on a worker
/// thread: phase 1 scans both folders (and detects conflicts),
/// phase 2 reads the update database and judges it against the
/// library just scanned. Progress streams to the app over the
/// channel as the work happens — real counts, ticking — and
/// the finished load follows at the end. Progress sends may
/// drop when the channel is full (the next tick is already
/// coming); the finish event uses a blocking send, so the
/// result itself can never be lost.
fn load_job(
    archive: PathBuf,
    active: PathBuf,
    parallel: usize,
    mut tx: futures::channel::mpsc::Sender<LoadEvent>,
) {
    // The senders the progress closures share: futures'
    // try_send wants &mut, the scan's callback wants a shared
    // &Sync — a mutex bridges the two.
    let progress_tx = std::sync::Arc::new(std::sync::Mutex::new(tx.clone()));
    let scan = {
        let progress_tx = progress_tx.clone();
        ModManager::scan_directories_with_progress(archive, active, parallel, &move |done, total| {
            if let Ok(mut sender) = progress_tx.lock() {
                let _ = sender.try_send(LoadEvent::Progress {
                    phase: LoadPhase::Mods,
                    done,
                    total,
                });
            }
        })
    };
    let finished = match scan {
        Ok(mods) => {
            let mod_list: Vec<Mod> = mods.into_values().collect();
            let conflicts = ConflictDetector::detect_conflicts(&mod_list);
            let updates = updates_job(&mod_list, &|done, total| {
                if let Ok(mut sender) = progress_tx.lock() {
                    let _ = sender.try_send(LoadEvent::Progress {
                        phase: LoadPhase::Updates,
                        done,
                        total,
                    });
                }
            });
            Ok(LoadOutcome {
                scan: ScanOutcome {
                    mods: mod_list,
                    conflicts,
                },
                updates,
            })
        }
        Err(e) => Err(e.to_string()),
    };
    let _ = futures::executor::block_on(tx.send(LoadEvent::Finished(finished)));
}

/// The load's second phase: read the update database (if a
/// fill has landed one) and judge it against the library —
/// the summary that feeds the Updates tile and filter, and
/// the per-mod rows the app turns into update states. This is
/// the computation that used to run on the UI thread when a
/// scan landed; running it here is what lets the splash
/// promise — and prove — that the counts are already true at
/// handoff.
///
/// Progress covers the whole phase under one total: every
/// database row examined (the summary pass), then every mod
/// matched against its row (the judgement pass).
fn updates_job(
    mods: &[Mod],
    report: &dyn Fn(usize, usize),
) -> UpdateDbPresentation {
    let Some(db) = update_db::load_default() else {
        report(1, 1);
        return UpdateDbPresentation {
            present: false,
            rows: HashMap::new(),
            generated_at: None,
            summary: update_db::DbSummary::default(),
            judged: Vec::new(),
        };
    };
    let rows_total = db.rows.len();
    let total = rows_total + mods.len();
    let installed: HashMap<String, String> = mods
        .iter()
        .map(|m| (ModManager::mod_stem(&m.path), m.version.clone()))
        .collect();
    let summary = update_db::summarize_with_progress(&db, &installed, &|done, _| {
        report(done, total)
    });
    let rows: HashMap<String, UpdateRow> = db
        .rows
        .iter()
        .map(|r| (r.stem.clone(), r.clone()))
        .collect();
    let mut judged = Vec::new();
    for (i, m) in mods.iter().enumerate() {
        let stem = ModManager::mod_stem(&m.path);
        if let Some(row) = rows.get(&stem) {
            judged.push(UpdateJudgement {
                id: m.id.clone(),
                row: row.clone(),
            });
        }
        report(rows_total + i + 1, total);
    }
    UpdateDbPresentation {
        present: true,
        rows,
        generated_at: Some(db.generated_at),
        summary,
        judged,
    }
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
    // Decoded icons are cached on disk (see icons.rs): the
    // first pass pays the archive opens and decodes, later
    // launches read ready-made thumbnails. The sweep after
    // the pass drops entries for mods no longer present.
    let cache_dir = icon_cache_dir();
    let keep: HashSet<String> = jobs
        .iter()
        .filter_map(|(_, path, _)| icon_cache_key(path))
        .collect();
    let out: Vec<(String, LoadedIcon)> = jobs
        .par_iter()
        .filter_map(|(id, path, icon)| {
            load_icon_cached(path, icon.as_deref(), cache_dir.as_deref())
                .map(|loaded| (id.clone(), loaded))
        })
        .collect();
    if let Some(dir) = &cache_dir {
        sweep_icon_cache(dir, &keep);
    }
    out
}

pub fn run(mod_manager: ModManager) -> anyhow::Result<()> {
    // Window/taskbar icon from John's Playground badge (the exe's
    // file icon is embedded separately by build.rs on Windows).
    let icon = iced::window::icon::from_file_data(
        include_bytes!("../../assets/icon.png"),
        None,
    )
    .ok();
    let result = iced::application(
        |_: &App| format!("Playground v{}", env!("CARGO_PKG_VERSION")),
        App::update,
        App::view,
    )
        .subscription(App::subscription)
        .theme(|app: &App| {
            // While the boot load runs, the shell wears the
            // splash theme — a transparent background, so
            // the window the logo floats in is truly clear
            // (the shell clears with the theme's background
            // color). The moment `booting` drops, this
            // returns Dark and the clear goes opaque with
            // it: the main window is the ordinary app.
            if app.booting {
                theme::splash_theme()
            } else {
                Theme::Dark
            }
        })
        .window(iced::window::Settings {
            // The launch window is the splash's stage
            // (John, 2026-10-21): SPLASH_W x SPLASH_H, his
            // measured size (about 4 grid squares — the
            // logo's native 1254 overflowed his screen),
            // borderless and transparent, so the
            // artwork floats on the desktop in its own
            // shape. At handoff it is resized to 1280x800,
            // re-decorated, and re-centered on the same
            // screen center (see App::restore_window) —
            // the main window's size and decorations are
            // the pre-splash ones, arrived at by restore
            // rather than by launch.
            size: Size::new(SPLASH_W, SPLASH_H),
            // Dead center of the screen (John, 2026-10-21):
            // the splash used to come up wherever the OS
            // defaulted it — mid-left on his screen.
            position: iced::window::Position::Centered,
            decorations: false,
            transparent: true,
            icon,
            ..iced::window::Settings::default()
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The restore safety net fires ONLY on the splash's
    /// own size: a landed restore (1280x800) and any
    /// hand-resized window must both leave the stateful
    /// decorations toggle untouched.
    #[test]
    fn splash_size_trigger_matches_only_the_splash() {
        assert!(App::still_splash_sized(Size::new(SPLASH_W, SPLASH_H)));
        assert!(App::still_splash_sized(Size::new(
            SPLASH_W + 0.5,
            SPLASH_H - 0.5
        )));
        assert!(!App::still_splash_sized(Size::new(1280.0, 800.0)));
        assert!(!App::still_splash_sized(Size::new(1000.0, 700.0)));
        assert!(!App::still_splash_sized(Size::new(SPLASH_W, 800.0)));
    }
}

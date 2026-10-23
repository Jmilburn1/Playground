use crate::mod_manager::conflict::Conflict;
use crate::mod_manager::gamescreens::GsPreset;
use crate::mod_manager::icons::LoadedIcon;
use crate::mod_manager::types::{Mod, ModState};
use crate::ui::theme;
use crate::update_db::{DbSummary, UpdateRow};
use iced::widget::{button, column, container, row, text};
use iced::{Color, Element, Length};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Browse,
    Conflicts,
    Saves,
    Layout,
    Log,
    Downloads,
    Keybindings,
    GameScreens,
    Settings,
}

/// The Settings page's own tabs (Round 2, owner
/// 2026-10-22 — Silo's tab shape in Playground's
/// vocabulary). Distinct from the app's page `Tab`: these
/// switch inside the one Settings page, and every tab's
/// values commit through the page's one explicit save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    ModsFolder,
    Library,
    Accounts,
    Diagnostics,
    Maintenance,
    About,
}

/// Where one tracked mod download stands (owner, 2026-10-22:
/// the Downloads section — "shows which mods are being
/// downloaded"). The registry walks each job Queued ->
/// Downloading -> Installing -> Done, or -> Failed from any
/// working state; the transitions are the registry's own
/// methods, so no caller can skip a step or resurrect a
/// finished job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadJobState {
    Queued,
    Downloading,
    Installing,
    Done,
    Failed,
}

/// One mod download, tracked from the click that started it
/// to the install that ends it. Byte counts are the
/// download stream's own: `total` is what the server stated
/// (Content-Length) and stays None when it stated nothing —
/// the page shows bytes alone then, never an invented
/// total. Times are monotonic instants, for ordering and
/// duration only; nothing here is persisted — the registry
/// is this session's history by design.
#[derive(Debug, Clone)]
pub struct DownloadJob {
    pub id: u64,
    /// The mod's app id (its library identity).
    pub mod_id: String,
    /// The mod's display name.
    pub mod_name: String,
    /// The mod's exact file name — the app's identity for it.
    pub file_name: String,
    /// Where the update came from ("GitHub", "ModHub",
    /// "KingMods", "itch.io" — from the update record).
    pub source: String,
    /// The version being installed.
    pub version: String,
    pub state: DownloadJobState,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// The plain-language reason when state is Failed.
    pub fail_reason: Option<String>,
    pub started_at: Option<std::time::Instant>,
    pub finished_at: Option<std::time::Instant>,
}

impl DownloadJob {
    /// Whole percent downloaded, when a total is known.
    /// None when it isn't (or is zero) — callers show the
    /// byte count alone rather than a fabricated figure.
    pub fn percent(&self) -> Option<f32> {
        let total = self.total?;
        if total == 0 {
            return None;
        }
        Some(((self.downloaded as f64 / total as f64) * 100.0).min(100.0) as f32)
    }

    /// Still in play: queued or working.
    pub fn is_live(&self) -> bool {
        matches!(
            self.state,
            DownloadJobState::Queued
                | DownloadJobState::Downloading
                | DownloadJobState::Installing
        )
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.state, DownloadJobState::Done | DownloadJobState::Failed)
    }
}

/// The session's download jobs, in the order they were
/// started. One download runs at a time: a start requested
/// while another is working registers as Queued and begins
/// when the running one finishes or fails — the queue the
/// registry's `next_queued` + `has_running` pair drives.
/// Any future starter (bulk actions, one-click installs)
/// registers here the same way the details panel does.
#[derive(Debug, Clone, Default)]
pub struct DownloadRegistry {
    jobs: Vec<DownloadJob>,
    next_id: u64,
}

impl DownloadRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a download that wants to start. The job is
    /// Queued; `start` moves it to Downloading when its
    /// turn comes. Returns the job's id.
    pub fn register(
        &mut self,
        mod_id: String,
        mod_name: String,
        file_name: String,
        source: String,
        version: String,
    ) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.jobs.push(DownloadJob {
            id,
            mod_id,
            mod_name,
            file_name,
            source,
            version,
            state: DownloadJobState::Queued,
            downloaded: 0,
            total: None,
            fail_reason: None,
            started_at: None,
            finished_at: None,
        });
        id
    }

    pub fn jobs(&self) -> &[DownloadJob] {
        &self.jobs
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Jobs still in play (queued or working) — the sidebar
    /// badge's count.
    pub fn live_count(&self) -> usize {
        self.jobs.iter().filter(|j| j.is_live()).count()
    }

    pub fn has_finished(&self) -> bool {
        self.jobs.iter().any(|j| j.is_finished())
    }

    /// A download is on the wire or installing — the queue's
    /// gate: while this holds, new starts stay Queued.
    pub fn has_running(&self) -> bool {
        self.jobs.iter().any(|j| {
            matches!(
                j.state,
                DownloadJobState::Downloading | DownloadJobState::Installing
            )
        })
    }

    /// The oldest job still waiting for its turn.
    pub fn next_queued(&self) -> Option<&DownloadJob> {
        self.jobs
            .iter()
            .find(|j| j.state == DownloadJobState::Queued)
    }

    /// The newest live job for one mod — the details panel's
    /// readout (its progress line reads the same job state
    /// the Downloads page shows).
    pub fn live_job_for_mod(&self, mod_id: &str) -> Option<&DownloadJob> {
        self.jobs
            .iter()
            .rev()
            .find(|j| j.mod_id == mod_id && j.is_live())
    }

    /// Queued -> Downloading. The start instant is stamped
    /// here, when the download truly begins — a queued job
    /// has not started, however long it waits.
    pub fn start(&mut self, id: u64) {
        if let Some(job) = self.job_mut(id) {
            if job.state == DownloadJobState::Queued {
                job.state = DownloadJobState::Downloading;
                job.started_at = Some(std::time::Instant::now());
            }
        }
    }

    /// A progress sample from the running download. Only a
    /// Downloading job takes samples; a late sample for a
    /// finished job is dropped, not recorded.
    pub fn progress(&mut self, id: u64, downloaded: u64, total: Option<u64>) {
        if let Some(job) = self.job_mut(id) {
            if job.state == DownloadJobState::Downloading {
                job.downloaded = downloaded;
                if total.is_some() {
                    job.total = total;
                }
            }
        }
    }

    /// Downloading -> Installing: the bytes are all down and
    /// the manager is swapping the file in.
    pub fn begin_install(&mut self, id: u64) {
        if let Some(job) = self.job_mut(id) {
            if job.state == DownloadJobState::Downloading {
                job.state = DownloadJobState::Installing;
            }
        }
    }

    /// End a working job: Ok -> Done, Err -> Failed with the
    /// reason kept in the failure's own words. Only a job
    /// that actually started (Downloading / Installing) can
    /// finish — a Queued job has nothing to finish, and a
    /// finished job stays finished. The finish instant is
    /// stamped here.
    pub fn finish(&mut self, id: u64, result: Result<(), String>) {
        if let Some(job) = self.job_mut(id) {
            if matches!(
                job.state,
                DownloadJobState::Downloading | DownloadJobState::Installing
            ) {
                match result {
                    Ok(()) => job.state = DownloadJobState::Done,
                    Err(reason) => {
                        job.state = DownloadJobState::Failed;
                        job.fail_reason = Some(reason);
                    }
                }
                job.finished_at = Some(std::time::Instant::now());
            }
        }
    }

    /// Sweep the finished rows (Done / Failed). Live jobs —
    /// queued ones included — are never swept.
    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| !j.is_finished());
    }

    /// The finished jobs as history entries, for the
    /// Downloads history file (Settings > Maintenance,
    /// "keep download history"): only Done / Failed rows
    /// persist — a queued or running job belongs to its
    /// session alone.
    pub fn history_entries(&self) -> Vec<DownloadHistoryEntry> {
        self.jobs
            .iter()
            .filter(|j| j.is_finished())
            .map(DownloadHistoryEntry::from_job)
            .collect()
    }

    /// Bring a previous session's history back as finished
    /// rows. Ids are re-assigned from the registry's own
    /// counter, and no instants are restored — a restored
    /// row keeps its outcome, not its timing.
    pub fn restore_history(&mut self, entries: Vec<DownloadHistoryEntry>) {
        for entry in entries {
            self.next_id += 1;
            self.jobs.push(DownloadJob {
                id: self.next_id,
                mod_id: entry.mod_id,
                mod_name: entry.mod_name,
                file_name: entry.file_name,
                source: entry.source,
                version: entry.version,
                state: if entry.failed {
                    DownloadJobState::Failed
                } else {
                    DownloadJobState::Done
                },
                downloaded: entry.downloaded,
                total: entry.total,
                fail_reason: entry.fail_reason,
                started_at: None,
                finished_at: None,
            });
        }
    }

    fn job_mut(&mut self, id: u64) -> Option<&mut DownloadJob> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }
}

/// One finished download, as stored in the Downloads
/// history file (downloads-history.json in the data
/// folder): the row's facts without its session timing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DownloadHistoryEntry {
    pub mod_id: String,
    pub mod_name: String,
    pub file_name: String,
    pub source: String,
    pub version: String,
    pub failed: bool,
    pub fail_reason: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
}

impl DownloadHistoryEntry {
    fn from_job(job: &DownloadJob) -> Self {
        Self {
            mod_id: job.mod_id.clone(),
            mod_name: job.mod_name.clone(),
            file_name: job.file_name.clone(),
            source: job.source.clone(),
            version: job.version.clone(),
            failed: job.state == DownloadJobState::Failed,
            fail_reason: job.fail_reason.clone(),
            downloaded: job.downloaded,
            total: job.total,
        }
    }
}

/// Progress events from a running mod download, streamed to
/// the app like the load's — real byte counts as the stream
/// lands (the download itself gates how often, see
/// sources::ProgressGate).
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Progress {
        job: u64,
        downloaded: u64,
        total: Option<u64>,
    },
}

/// Which folder a Browse picker is choosing (Settings tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderTarget {
    /// The game's mods folder — the folder switcher; picking one
    /// also writes the gameSettings.xml override.
    Active,
    /// The archive folder — picker convenience only.
    Archive,
    /// The game's profile folder, where game.xml lives.
    Game,
    /// The game's true install folder, where the `shared`
    /// folder lives (Game Screens). The pick is validated
    /// to hold a `shared` folder before it is saved.
    GameInstall,
}

/// Which configured source list a Settings page toggle
/// belongs to (Catalog & Sources section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceList {
    ModHub,
    GitHub,
    KingMods,
}

/// What the browse grid is showing: everything, one folder's mods,
/// favorites, or (once category parsing lands) one category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseView {
    All,
    Active,
    Archived,
    Favorites,
    /// Mods with an update available (reader state — from
    /// the update database at launch, or a live check).
    Updates,
    Category(String),
}

impl std::fmt::Display for BrowseView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowseView::All => write!(f, "All mods"),
            BrowseView::Active => write!(f, "Active mods"),
            BrowseView::Archived => write!(f, "Archived mods"),
            BrowseView::Favorites => write!(f, "Favorite mods"),
            BrowseView::Updates => write!(f, "Updates"),
            BrowseView::Category(c) => write!(f, "{}", c),
        }
    }
}

/// Result of a background library scan: the mods found and the
/// conflicts detected among them.
#[derive(Debug, Clone)]
pub struct ScanOutcome {
    pub mods: Vec<Mod>,
    pub conflicts: Vec<Conflict>,
}

/// One phase of the two-phase library load. Phase 1 loads the
/// mods (the folder scan); phase 2 loads the updates (the
/// update database read and judged against the library). The
/// launch splash's bar spans both, so the handoff to the main
/// window happens only when both are done — and the Updates
/// tile count is already true at that moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPhase {
    Mods,
    Updates,
}

/// One mod's update-database judgement, computed in the load's
/// second phase: the mod's id and the database row it was
/// judged against. The app turns these into per-mod update
/// states when the load lands.
#[derive(Debug, Clone)]
pub struct UpdateJudgement {
    pub id: String,
    pub row: UpdateRow,
}

/// The update database, read and judged in the load's second
/// phase — the same presentation the app used to compute
/// synchronously on the UI thread when a scan landed. When
/// `present` is false there is no database file (yet), and the
/// app simply has nothing to present.
#[derive(Debug, Clone)]
pub struct UpdateDbPresentation {
    pub present: bool,
    pub rows: HashMap<String, UpdateRow>,
    pub generated_at: Option<u64>,
    pub summary: DbSummary,
    pub judged: Vec<UpdateJudgement>,
}

/// Everything one full load produces: the scan and, judged
/// against it, the update database.
#[derive(Debug, Clone)]
pub struct LoadOutcome {
    pub scan: ScanOutcome,
    pub updates: UpdateDbPresentation,
}

/// Progress and completion events from the background load
/// job, streamed to the app while the load runs. Progress
/// counts are real work counts (files processed of files
/// found, rows judged of rows total) — never estimates.
#[derive(Debug, Clone)]
pub enum LoadEvent {
    Progress {
        phase: LoadPhase,
        done: usize,
        total: usize,
    },
    Finished(Result<LoadOutcome, String>),
}

/// How a Layout page migration run ended, as the page
/// reports it: the executor's outcomes, with a failed
/// run's reversal called out as its own case — it is
/// NOT a success, and the page must never dress it as
/// one (design: "a failed apply's auto-reversal is
/// reported plainly").
#[derive(Debug, Clone)]
pub enum LayoutRunOutcome {
    /// The plan held no work: already in layout form.
    AlreadyLaidOut,
    Migrated {
        moves: usize,
        links: usize,
        /// The pre-layout game folder, kept whole.
        backup: String,
        /// The per-run log file.
        log: String,
    },
    /// The migration failed partway and everything it
    /// did was reversed from the operation log. The
    /// string is the failure's own words.
    FailedReversed(String),
}

/// Progress and completion events from the Layout
/// page's background jobs, streamed over a channel like
/// the load's (real op counts, ticking; the finish event
/// is sent with a blocking send so it cannot be lost).
#[derive(Debug, Clone)]
pub enum LayoutEvent {
    Progress { done: usize, total: usize },
    ApplyFinished(Result<LayoutRunOutcome, String>),
    /// Ok carries the page-ready summary line.
    RollbackFinished(Result<String, String>),
}

/// How a game-log line is classified for the Log Reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Normal,
    Warning,
    Error,
}

/// One classified line of the game's log.txt.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub text: String,
    pub kind: LogKind,
}

/// Result of reading the game log in the background: the last
/// lines (capped for display), counts over the whole file, and
/// the names of library mods that error/warning lines mention.
#[derive(Debug, Clone)]
pub struct LogOutcome {
    pub lines: Vec<LogLine>,
    pub suspects: Vec<String>,
    pub error_count: usize,
    pub warning_count: usize,
    pub total_lines: usize,
}

pub mod update_types {
    /// A newer release found by an update check.
    #[derive(Debug, Clone)]
    pub struct UpdateInfo {
        /// Normalized version of the latest release ("1.2.0").
        pub version: String,
        /// The release tag as published ("v1.2.0").
        pub tag: String,
        /// Download URL of the chosen mod .zip asset.
        pub download_url: String,
        /// The asset's file name.
        pub asset_name: String,
        /// The repo the update came from ("owner/repo").
        pub repo: String,
    }

    /// Per-mod update-check state, keyed by mod id in the app.
    #[derive(Debug, Clone)]
    pub enum UpdateState {
        Checking,
        UpToDate,
        Available(UpdateInfo),
        Installing,
        Failed(String),
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Scan,
    /// One event from the background load job: a progress tick
    /// for the splash bar, or the finished load.
    LoadEvent(LoadEvent),
    IconsLoaded(Vec<(String, LoadedIcon)>),
    SearchChanged(String),
    TabSelected(Tab),
    /// Sidebar library shortcut: jump to Browse showing this view.
    LibraryFilter(BrowseView),
    BrowseViewChanged(BrowseView),
    SelectMod(String),
    CloseDetails,
    ToggleFavorite(String),
    /// Favorite toggle from a card's star badge (also sets the
    /// select-suppression flag in the app).
    CardToggleFavorite(String),
    /// Activate/deactivate from a card's slider switch (also
    /// sets the select-suppression flag in the app).
    CardToggleState(String),
    ToggleFavoritesOnly,
    MoveToArchive(String),
    MoveToActive(String),
    /// Hold-to-delete (feel pass, 2026-10-23): a routine
    /// single-mod delete no longer arms on one click and
    /// fires on the next — the Remove control must be
    /// HELD. Started on press, Released on release/exit
    /// (a cancel), Tick drives the hold clock.
    RemoveHoldStarted(String),
    RemoveHoldReleased,
    RemoveHoldTick,
    /// The feel pass's shared animation clock (2026-10-23):
    /// one tick drives the switch glides, the rolling
    /// counts, the easing progress bars and the
    /// indeterminate pulse. Subscribed only while an
    /// animation is live or settling.
    AnimTick,
    /// A browse card's hover state (feel pass, 2026-10-23):
    /// Some(id) on enter, None on exit — the hovered card
    /// lifts.
    CardHover(Option<String>),
    SaveDirectories,
    /// Settings page, Library section: the conflict-
    /// checking switch and the two numeric fields, staged
    /// on the page and committed by SaveDirectories (the
    /// page's one explicit save).
    SettingsCheckConflictsToggled(bool),
    SettingsParallelScansChanged(String),
    SettingsCacheTtlChanged(String),
    /// Settings page, Catalog & Sources section: the
    /// catalog URL field and one source's enabled toggle,
    /// staged the same way.
    SettingsCatalogUrlChanged(String),
    /// Settings page, General > Theme: a theme was
    /// picked (themes, 2026-10-23). Applies live and
    /// persists at once — it does not ride the page's
    /// Save settings.
    ThemeSelected(String),
    SettingsSourceToggled(SourceList, usize, bool),
    BrowseFolder(FolderTarget),
    FolderPicked(FolderTarget, Option<PathBuf>),
    RefreshLog,
    LogLoaded(Result<LogOutcome, String>),
    LogErrorsOnlyToggled(bool),
    /// Update checks (GitHub): the details panel's repo field.
    LinkInputChanged(String),
    SaveModLink(String),
    /// Update checks (ModHub): the details panel's ModHub field.
    ModHubInputChanged(String),
    SaveModHubLink(String),
    /// The catalog arrived (or not) for a detection pass:
    /// stem -> mod_id pairs the catalog could answer
    /// instantly, before any live searching happens.
    DetectCatalogReady(Vec<(String, String)>),
    ModHubLinksDetected {
        found: Vec<(String, String)>,
        missed: usize,
    },
    /// One batch of a chunked detection pass (the pass saves
    /// every batch, so closing the app mid-pass loses nothing).
    /// `missed` counts everything the batch did not link
    /// (not-found and failed searches alike, as the tally
    /// always has); `missed_keys` carries only the true
    /// not-founds — (stem, file name) — whose outcome is
    /// written to the miss store so they are never
    /// searched again.
    DetectBatchDone {
        found: Vec<(String, String)>,
        missed: usize,
        missed_keys: Vec<(String, String)>,
        processed: usize,
        /// GIANTS throttled this batch (HTTP 403/429): the
        /// tripwire. The handler stops the pass here — the
        /// remaining jobs are dropped unsearched; this
        /// batch's links and misses were already saved.
        throttled: bool,
    },
    CheckUpdate(String),
    UpdateChecked(String, Result<Option<update_types::UpdateInfo>, String>),
    InstallUpdate(String),
    UpdateInstalled(String, Result<std::path::PathBuf, String>),
    SelectSaveSlot(String),
    DraftToggle(String),
    DraftUseActive,
    DraftEmpty,
    SavesFilterChanged(String),
    /// Configure Save wizard: jump to a step (0-based:
    /// 0 Select, 1 Settings, 2 Loadout, 3 Review,
    /// 4 Results). Navigation past Select needs a slot.
    WizardStep(u8),
    /// Configure Save wizard: a boolean settings field set
    /// by its key (see ui::app's settings key table).
    WizardSetBool(String, bool),
    /// Configure Save wizard: an enum/number settings field
    /// set by key, value in the game's own storage shape.
    WizardSetValue(String, String),
    /// Configure Save wizard: the save-name text field.
    WizardNameChanged(String),
    /// Configure Save wizard: cash / loan exact-value
    /// fields, and the preset chips that adjust them.
    WizardCashChanged(String),
    WizardLoanChanged(String),
    WizardCashChip(i64),
    WizardLoanChip(i64),
    /// Configure Save wizard: reseed the loadout draft from
    /// the save's own recorded mod list.
    WizardUseSaveList,
    /// Configure Save wizard: select/deselect a whole
    /// loadout group ("required" / "favourites" / "library").
    WizardGroupToggle(String, bool),
    /// Configure Save wizard: collapse/expand a loadout
    /// group on the page.
    WizardCollapseGroup(String),
    /// Configure Save wizard: the Auto Drive section's
    /// Browse button and its outcome.
    WizardBrowseAutoDrive,
    WizardAutoDrivePicked(Option<PathBuf>),
    WizardClearAutoDrive,
    /// Configure Save wizard: the Review page's apply
    /// (arm-to-confirm, like the Layout page's).
    WizardApply,
    /// Configure Save wizard: leave the results and start
    /// over at Select with a clean draft.
    WizardReset,
    /// Layout page: build (or rebuild) the read-only
    /// plan preview for the current folders.
    LayoutBuildPlan,
    /// Layout page: arm, then (second press) execute the
    /// previewed plan in the background.
    LayoutApply,
    /// Layout page: arm, then (second press) roll the
    /// recorded migration back in the background.
    LayoutRollback,
    /// Layout page, Reclaim section: take the read-only
    /// census of reclaimable space (backups beyond the
    /// newest, exact duplicate copies).
    ReclaimMeasure,
    /// Layout page, Reclaim section: arm, then (second
    /// press) run the reclaim the census describes.
    ReclaimRun,
    /// One event from a Layout background job.
    LayoutEvent(LayoutEvent),
    WindowResized(iced::Size),
    /// Boot handover (splash -> main window): the window
    /// id read answered. The handover collects this fact
    /// and the splash's position, then fires frame + size
    /// + centering as ONE batched shape change.
    RestoreWindowId(Option<iced::window::Id>),
    /// Boot handover: the splash position read answered.
    RestoreWindowPosition(Option<iced::Point>),
    /// Boot handover: the clock tick driving the position
    /// deadline and the verify net's schedule. The
    /// subscription runs only while a handover is live.
    RestoreTick,
    /// Boot handover: the verify net's size read-back.
    RestoreVerifySize(iced::Size),
    GridScrolled(iced::widget::scrollable::Viewport),
    ToggleShowMenu,
    /// App self-update: the launch check answered. Some
    /// carries the newer release; None covers up-to-date,
    /// offline, refused and every other quiet outcome —
    /// the check is silent unless it finds something.
    AppUpdateCheckDone(Option<crate::app_update::AppRelease>),
    /// App self-update: the notice's Install press.
    InstallAppUpdate,
    /// App self-update: the notice dismissed for this
    /// launch (nothing is persisted; the next launch
    /// checks again).
    DismissAppUpdate,
    /// App self-update: the download+verify+stage job
    /// answered. Ok carries the staging folder holding
    /// the new binaries; the app then hands the swap to
    /// update.exe and exits.
    AppUpdateStaged(Result<PathBuf, String>),
    /// Downloads section: one progress event from a running
    /// mod download (see DownloadEvent).
    DownloadEvent(DownloadEvent),
    /// Downloads section: sweep the finished rows (Done /
    /// Failed) off the page; live jobs stay.
    ClearFinishedDownloads,
    /// Settings page (Round 2): switch the page's tab.
    /// Every tab stages into the same save.
    SettingsTabSelected(SettingsTab),
    /// Settings page (Round 2): collapse or expand one of
    /// the page's boxes, by its stable id.
    SettingsToggleSection(String),
    /// Settings, General > Updates: the start-up update
    /// check switch, staged for the page save.
    SettingsStartupCheckToggled(bool),
    /// Settings, General > Updates: the self-update switch,
    /// staged for the page save.
    SettingsSelfUpdateToggled(bool),
    /// Settings, General > Updates: run the self-update
    /// check on demand (works whichever way the switch
    /// sits — it is an explicit ask).
    CheckAppUpdateNow,
    /// Settings, General > Behavior: the Browse view a
    /// fresh launch opens with, staged for the page save.
    SettingsBrowseDefaultChanged(BrowseView),
    /// Settings, General > Behavior: start maximized,
    /// staged for the page save.
    SettingsStartMaximizedToggled(bool),
    /// Settings, General > Launch Configuration: one of
    /// the launch settings flipped, staged for the page
    /// save (the Developer Console stages too — it writes
    /// game.xml when the page saves, like everything else
    /// on the page).
    SettingsLaunchToggled(LaunchSetting, bool),
    /// Settings, General > Launch Configuration: the
    /// savegame slot field (text; empty = unset), staged
    /// for the page save.
    SettingsLaunchSlotChanged(String),
    /// Settings, Library > Auto-Fill, staged for the save.
    SettingsAutoFileToggled(bool),
    /// Settings, Maintenance > Downloads history switch,
    /// staged for the page save.
    SettingsKeepHistoryToggled(bool),
    /// Settings, Maintenance > Fill log keep-count field,
    /// staged for the page save.
    SettingsFillLogKeepChanged(String),
    /// Settings, Diagnostics > Log Reader: the
    /// errors-only-on-open default, staged for the save.
    SettingsLogErrorsOnlyDefaultToggled(bool),
    /// Settings, Diagnostics > Log Reader: the lines-cap
    /// field, staged for the page save.
    SettingsLogLinesCapChanged(String),
    /// Settings: the scheduled-fill task query answered
    /// (None = the query could not run on this platform).
    FillTaskStatus(Option<bool>),
    /// Open a folder in the OS file explorer (Settings path
    /// rows' Open folder; Diagnostics' Open log folder).
    OpenFolder(PathBuf),
    /// Open a URL in the browser (About links).
    OpenUrl(String),
    /// Settings, Diagnostics: copy the diagnostics summary
    /// to the clipboard.
    CopyDiagnostics,
    CopyLaunchFlags,
    /// Settings, Diagnostics: save the diagnostics summary
    /// to a text file the user picks.
    ExportDiagnostics,
    /// Settings, Diagnostics: the export dialog answered.
    DiagnosticsExported(Option<PathBuf>),
    /// Settings, Maintenance: forget the library database's
    /// fingerprints and rescan, re-reading every mod.
    RereadEveryMod,
    /// Keybindings: switch the device tab.
    KeybindingsDeviceSelected(String),
    /// Keybindings: the search field.
    KeybindingsSearchChanged(String),
    /// Keybindings: show unassigned actions only.
    KeybindingsUnassignedOnlyToggled(bool),
    /// Keybindings: show conflicted actions only.
    KeybindingsConflictsOnlyToggled(bool),
    /// Keybindings: include archived mods' actions in
    /// the groups and findings (off = active mods only,
    /// the default).
    KeybindingsShowArchivedToggled(bool),
    /// Keybindings: expand (true) or collapse (false)
    /// every group at once.
    KeybindingsExpandAll(bool),
    /// Keybindings: collapse or expand one group, by name.
    KeybindingsToggleGroup(String),
    /// Keybindings: start key capture to REPLACE one
    /// binding (the action's binding at this position in
    /// the staged document).
    KeybindingsCaptureStart(String, usize),
    /// Keybindings: start key capture to ADD a binding to
    /// an unassigned action (keyboard tab).
    KeybindingsAddBinding(String),
    /// Keybindings: one key press while capture is armed.
    KeybindingsCaptureKey(
        iced::keyboard::Key,
        iced::keyboard::Location,
        iced::keyboard::Modifiers,
    ),
    /// Keybindings: remove one binding (the action's
    /// binding at this position in the staged document).
    KeybindingsClearBinding(String, usize),
    /// Keybindings: one group's Auto-Resolve press.
    KeybindingsAutoResolve(String),
    /// Keybindings: commit the staged edits (a backup of
    /// the live file is taken first; a failed backup
    /// aborts the save).
    KeybindingsSave,
    /// Keybindings: throw the staged edits away.
    KeybindingsDiscard,
    /// Keybindings: restore one backup over the live file
    /// (the live file is backed up first).
    KeybindingsRestoreBackup(PathBuf),
    /// Keybindings: reset to game defaults — arm on the
    /// first press, execute on the second.
    KeybindingsResetDefaults,
    /// Keybindings: purge the bindings of archived mods —
    /// the first press counts the candidates and arms, the
    /// second stages the removals into the document.
    KeybindingsPurgePressed,
    /// Game Screens: a preset was chosen in the presets
    /// row (the stock choice and custom upload are their
    /// own messages).
    GameScreensPresetSelected(GsPreset),
    /// Game Screens: the FS25 Standard (stock) choice in
    /// the presets row.
    GameScreensStandardSelected,
    /// Game Screens: open the file picker for a custom
    /// image.
    GameScreensPickCustom,
    /// Game Screens: the custom-image picker answered.
    GameScreensCustomPicked(Option<PathBuf>),
    /// Game Screens: build the chosen screens and write
    /// them (the current pair is captured first; the stock
    /// choice restores the oldest capture instead).
    GameScreensApply,
    /// Game Screens: the background build/write finished.
    /// Carries (applied preset, backup folder, splash
    /// hash) on success.
    GameScreensApplied(Result<(Option<GsPreset>, String, String), String>),
    /// Game Screens: restore one listed backup over the
    /// live files (the live files are captured first).
    GameScreensRestoreBackup(PathBuf),
    /// KingMods manual path: open the mod's KingMods page
    /// in the browser.
    OpenKingModsPage(String),
    /// KingMods manual path: pick the file the user
    /// downloaded by hand for this mod's update.
    ImportKingModsFile(String),
    /// KingMods manual path: the picker answered; the
    /// chosen file rides the ordinary install funnel.
    KingModsFilePicked(String, Option<PathBuf>),
}

/// One of the Launch Configuration's switches (Settings,
/// General). The Developer Console rides along: it is a
/// game-start setting in the same box, staged and saved
/// with the rest even though it acts through game.xml
/// rather than the composed command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchSetting {
    Console,
    SkipModUpdateDialog,
    Cheats,
    AutoStart,
    SkipStartVideos,
}

pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

/// Two-letter monogram used when a mod has no loadable icon image.
fn initials(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(2)
        .filter_map(|w| w.chars().next())
        .map(|c| c.to_uppercase().next().unwrap_or(c))
        .collect()
}

/// Muted accent color per category, Hub-pill style: the same
/// category always hashes to the same color from this palette.
fn category_color(name: &str) -> Color {
    const PALETTE: [(f32, f32, f32); 10] = [
        (0.13, 0.42, 0.45), // teal
        (0.55, 0.42, 0.13), // gold
        (0.38, 0.28, 0.55), // purple
        (0.62, 0.33, 0.13), // rust orange
        (0.25, 0.42, 0.25), // forest
        (0.25, 0.35, 0.55), // steel blue
        (0.50, 0.22, 0.30), // burgundy
        (0.35, 0.45, 0.15), // olive
        (0.45, 0.25, 0.45), // plum
        (0.30, 0.33, 0.38), // slate
    ];
    let hash: usize = name.bytes().map(|b| b as usize).sum();
    let (r, g, b) = PALETTE[hash % PALETTE.len()];
    Color::from_rgb(r, g, b)
}

/// One mod card for the browse grid (Hub-style, John's round-4 spec):
/// the mod icon fills the TOP of the card with the category as a
/// colored pill overlaid top-left, and a dark bottom panel carries
/// the light text — title, author, version/state. The whole card is
/// the click target for the details fly-out. Favorites use grey
/// (John's pick): a thicker grey border and a grey "Favorite" chip.
/// The star badge on the image is gold when favorited; if the star
/// glyph renders as a box on John's build (some glyphs did in the
/// default Windows font), it gets reverted.
pub fn mod_card<'a>(
    mod_data: &'a Mod,
    icon: Option<&iced::widget::image::Handle>,
    is_favorite: bool,
    is_selected: bool,
    has_conflict: bool,
    update_available: bool,
    width: f32,
    height: f32,
    is_hovered: bool,
    switch_t: f32,
) -> Element<'a, Message> {
    const ICON_H: f32 = 100.0;
    let icon_img: Element<'a, Message> = match icon {
        Some(handle) => iced::widget::image(handle.clone())
            .width(Length::Fill)
            .height(ICON_H)
            .content_fit(iced::ContentFit::Cover)
            .into(),
        None => container(text(initials(&mod_data.name)).size(30).color(Color::from_rgb(0.85, 0.86, 0.9)))
            .width(Length::Fill)
            .height(ICON_H)
            .center_x(Length::Fill)
            .center_y(ICON_H)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.22, 0.23, 0.26))),
                ..container::Style::default()
            })
            .into(),
    };
    // Overlay on the image: category pill top-left, star badge in a
    // dark circular "ribbon" top-right (Hub-style favorite marker;
    // clicking it toggles the favorite without opening details). The
    // star is DRAWN on a canvas (StarIcon below), not text: every
    // star glyph tried — U+2605, U+2606, and U+22C6 at John's
    // suggestion — rendered as a hollow box in this toolkit's
    // default font on his Windows build. A drawn star needs no font.
    let star_badge = button(
        container(
            iced::widget::canvas(StarIcon { filled: is_favorite })
                .width(16)
                .height(16),
        )
        .width(26)
        .height(26)
        .center_x(26)
        .center_y(26),
    )
    .width(26)
    .height(26)
    .padding(0)
    .style(|_, _| button::Style {
        background: Some(iced::Background::Color(theme::panel())),
        border: iced::Border {
            radius: 13.0.into(),
            ..iced::Border::default()
        },
        ..button::Style::default()
    })
    .on_press(Message::CardToggleFavorite(mod_data.id.clone()));

    let mut overlay_row = row![].spacing(6);
    if let Some(category) = mod_data.categories.first() {
        let pill = container(
            text(category.clone())
                .size(11)
                .wrapping(iced::widget::text::Wrapping::None)
                .color(Color::from_rgb(0.95, 0.95, 0.97)),
        )
        .padding([2, 7])
        .style({
            let color = category_color(category);
            move |_| container::Style {
                background: Some(iced::Background::Color(color)),
                border: iced::Border {
                    radius: 9.0.into(),
                    ..iced::Border::default()
                },
                ..container::Style::default()
            }
        });
        overlay_row = overlay_row.push(pill);
    }
    overlay_row = overlay_row
        .push(iced::widget::Space::with_width(Length::Fill))
        .push(star_badge);
    let overlay: Element<'a, Message> = container(overlay_row)
        .width(Length::Fill)
        .height(ICON_H)
        .padding(7)
        .into();
    // Inset the image a few px inside the card: the card's border is
    // rounded but containers don't clip their children, so an image
    // flush with the edge paints over the border (John, live: "the
    // modIcon is bleeding over the border of the box").
    let icon_area: Element<'a, Message> = container(iced::widget::stack![icon_img, overlay])
        .width(Length::Fill)
        .padding(iced::Padding {
            top: 3.0,
            right: 3.0,
            bottom: 0.0,
            left: 3.0,
        })
        .into();

    let mut bottom = column![
        text(mod_data.name.clone())
            .size(14)
            .color(Color::from_rgb(0.94, 0.94, 0.96)),
    ]
    .spacing(3)
    .height(Length::Fill);
    if let Some(author) = &mod_data.author {
        bottom = bottom.push(
            text(author.clone())
                .size(12)
                .color(Color::from_rgb(0.62, 0.63, 0.67)),
        );
    }
    bottom = bottom.push(
        text(format!("v{}", mod_data.version))
            .size(12)
            .color(Color::from_rgb(0.62, 0.63, 0.67)),
    );

    // Status row of pills (John's round-4 tweak): the mod's state as
    // a pill, then Favorite (grey), then Has conflicts (red pill,
    // white text) — all the same rounded shape.
    let pill = |label: String, bg: Color| {
        container(
            text(label)
                .size(10)
                .wrapping(iced::widget::text::Wrapping::None)
                .color(Color::from_rgb(0.97, 0.97, 0.98)),
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
    let state_bg = match mod_data.state {
        ModState::Active => Color::from_rgb(0.16, 0.45, 0.22),
        ModState::Archived => Color::from_rgb(0.55, 0.42, 0.13),
        ModState::Conflict | ModState::Error => Color::from_rgb(0.62, 0.16, 0.13),
    };
    // Pills in one row when the card is wide enough — John prefers
    // the third pill to stay on the row (with slightly smaller pill
    // text) over wrapping. On narrow cards they flow two per row:
    // an overflowing row gets its children flex-shrunk to slivers
    // (seen live as colored dashes).
    let mut pill_items: Vec<Element<'a, Message>> =
        vec![pill(format!("{:?}", mod_data.state), state_bg).into()];
    if is_favorite {
        pill_items.push(
            pill("Favorite".to_string(), Color::from_rgb(0.32, 0.33, 0.37)).into(),
        );
    }
    if has_conflict {
        pill_items.push(
            pill("Has conflicts".to_string(), Color::from_rgb(0.62, 0.16, 0.13)).into(),
        );
    }
    if update_available {
        pill_items.push(
            pill("Update".to_string(), Color::from_rgb(0.25, 0.35, 0.55)).into(),
        );
    }
    if width >= 195.0 {
        let mut r = row![].spacing(6);
        for p in pill_items {
            r = r.push(p);
        }
        bottom = bottom.push(r);
    } else {
        let mut pill_block = column![].spacing(4);
        let mut iter = pill_items.into_iter().peekable();
        while iter.peek().is_some() {
            let mut r = row![].spacing(6);
            for _ in 0..2 {
                if let Some(p) = iter.next() {
                    r = r.push(p);
                }
            }
            pill_block = pill_block.push(r);
        }
        bottom = bottom.push(pill_block);
    }

    // The activate slider (John, 2026-10-22 — Silo's toggles,
    // admired live the same night: they look nice and they are
    // effective): a drawn switch on the card's bottom panel.
    // Flipping it activates or deactivates the mod without
    // opening the details panel — the same move operations the
    // panel's buttons run. Only Active/Archived mods carry
    // one; a conflicted or errored mod's state is not a
    // two-position question. Drawn on a canvas like the star:
    // no font glyph or stock widget is trusted to look the
    // same on every build.
    if matches!(mod_data.state, ModState::Active | ModState::Archived) {
        let switch = button(
            container(iced::widget::canvas(SwitchIcon { t: switch_t }).width(34).height(19))
                .width(34)
                .height(19)
                .center_x(34)
                .center_y(19),
        )
        .width(34)
        .height(19)
        .padding(0)
        .style(|_, _| button::Style {
            background: None,
            ..button::Style::default()
        })
        .on_press(Message::CardToggleState(mod_data.id.clone()));
        // Bottom RIGHT of the card, and wordless (owner
        // ruling, 2026-10-22, from the Playground room): the
        // switch alone at the panel's right edge — "the pill
        // will tell us"; the state pill above already carries
        // the word, so the toggle is just the switch.
        // (v1.4.1) The row is anchored to the panel's BOTTOM
        // too: the Fill-height spacer above it drops it into
        // the card's bottom-right corner. Before, the row
        // ended the content stack wherever the pills ended
        // and floated with dead panel below it — measured in
        // the rig at ~55px above the card's bottom border.
        bottom = bottom.push(iced::widget::Space::with_height(Length::Fill));
        bottom = bottom.push(
            row![iced::widget::Space::with_width(Length::Fill), switch].spacing(7),
        );
    }

    let panel: Element<'a, Message> = container(bottom)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(9)
        .into();

    let content = column![icon_area, panel].spacing(0);

    // The card root is a container inside a mouse_area rather than a
    // button, so the star badge can be a real button of its own.
    let card = container(content)
        .width(width)
        .height(height)
        .style(move |_| {
            // Hover lift (feel pass, 2026-10-23): the
            // hovered card rises off the grid — its ground
            // lightens a step toward the panel, its edge
            // leans toward the accent, and a soft shadow
            // appears under it. Selected still wins.
            let lifted = is_hovered && !is_selected;
            container::Style {
                background: Some(iced::Background::Color(if is_selected {
                    Color::from_rgb(0.20, 0.21, 0.24)
                } else if lifted {
                    Color::from_rgb(0.11, 0.13, 0.15)
                } else {
                    theme::base()
                })),
                border: iced::Border {
                    color: if is_selected {
                        Color::from_rgb(0.82, 0.83, 0.87)
                    } else if lifted {
                        theme::accent()
                    } else if is_favorite {
                        Color::from_rgb(0.58, 0.59, 0.63)
                    } else {
                        Color::from_rgb(0.30, 0.31, 0.34)
                    },
                    width: if is_selected || is_favorite { 2.0 } else { 1.0 },
                    radius: 8.0.into(),
                },
                shadow: if lifted {
                    iced::Shadow {
                        color: Color { a: 0.5, ..theme::base() },
                        offset: iced::Vector::new(0.0, 3.0),
                        blur_radius: 10.0,
                    }
                } else {
                    iced::Shadow::default()
                },
                ..container::Style::default()
            }
        });

    iced::widget::mouse_area(card)
        .on_release(Message::SelectMod(mod_data.id.clone()))
        .on_enter(Message::CardHover(Some(mod_data.id.clone())))
        .on_exit(Message::CardHover(None))
        .into()
}

/// A five-pointed star drawn with canvas paths — gold and filled for
/// a favorited mod, grey outline otherwise. Drawn rather than typed
/// because the UI font has no star glyph (see mod_card).
pub struct StarIcon {
    pub filled: bool,
}

impl<Message> iced::widget::canvas::Program<Message> for StarIcon {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        _renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        use iced::widget::canvas::{Frame, Path, Stroke};
        let mut frame = Frame::new(_renderer, bounds.size());
        let center = frame.center();
        let r_outer = bounds.width.min(bounds.height) / 2.0 * 0.92;
        let r_inner = r_outer * 0.42;
        let star = Path::new(|b| {
            for i in 0..10 {
                let r = if i % 2 == 0 { r_outer } else { r_inner };
                let angle = -std::f32::consts::FRAC_PI_2
                    + i as f32 * std::f32::consts::PI / 5.0;
                let p = iced::Point::new(
                    center.x + r * angle.cos(),
                    center.y + r * angle.sin(),
                );
                if i == 0 {
                    b.move_to(p);
                } else {
                    b.line_to(p);
                }
            }
            b.close();
        });
        if self.filled {
            frame.fill(&star, Color::from_rgb(1.0, 0.84, 0.35));
        } else {
            frame.stroke(
                &star,
                Stroke::default()
                    .with_color(Color::from_rgb(0.78, 0.79, 0.83))
                    .with_width(1.4),
            );
        }
        vec![frame.into_geometry()]
    }
}

/// An activate/deactivate slider drawn with canvas paths —
/// the Silo-style switch (John, 2026-10-22): a pill track and
/// a round knob, knob right on the steel-teal accent track
/// when the mod is active, knob left on a dim track when it
/// is not. The colors are the app palette's own values (see
/// ui::theme), so the switch wears the layout's colors.
pub struct SwitchIcon {
    /// Knob position, 0.0 (off, left) to 1.0 (on, right).
    /// The feel pass (2026-10-23) glides this between the
    /// ends instead of snapping; at rest it sits exactly
    /// on an end.
    pub t: f32,
}

impl<Message> iced::widget::canvas::Program<Message> for SwitchIcon {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        _renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        use iced::widget::canvas::{Frame, Path};
        let mut frame = Frame::new(_renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        let t = self.t.clamp(0.0, 1.0);
        let blend = |a: Color, b: Color| Color {
            r: a.r + (b.r - a.r) * t,
            g: a.g + (b.g - a.g) * t,
            b: a.b + (b.b - a.b) * t,
            a: 1.0,
        };
        // The CURRENT theme's accent lit, a neutral dim
        // grey at rest — blended by the glide (the switch
        // follows the theme like everything else).
        let track_color = blend(Color::from_rgb(0.30, 0.31, 0.34), theme::accent());
        let track = Path::rounded_rectangle(
            iced::Point::new(0.0, 0.0),
            iced::Size::new(w, h),
            (h / 2.0).into(),
        );
        frame.fill(&track, track_color);
        let knob_r = h / 2.0 - 2.5;
        let knob_center = iced::Point::new(h / 2.0 + t * (w - h), h / 2.0);
        // The theme's full text lit, its dim text at rest.
        let knob_color = blend(theme::text_dim(), theme::text());
        let knob = Path::circle(knob_center, knob_r);
        frame.fill(&knob, knob_color);
        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod download_registry_tests {
    use super::{DownloadJobState, DownloadRegistry};

    fn registry_with(names: &[&str]) -> (DownloadRegistry, Vec<u64>) {
        let mut reg = DownloadRegistry::new();
        let ids = names
            .iter()
            .map(|n| {
                reg.register(
                    format!("id-{}", n),
                    n.to_string(),
                    format!("FS25_{}.zip", n),
                    "GitHub".to_string(),
                    "2.0.0.0".to_string(),
                )
            })
            .collect();
        (reg, ids)
    }

    fn state_of(reg: &DownloadRegistry, id: u64) -> DownloadJobState {
        reg.jobs().iter().find(|j| j.id == id).unwrap().state
    }

    #[test]
    fn job_walks_the_full_happy_path() {
        let (mut reg, ids) = registry_with(&["Alpha"]);
        let id = ids[0];
        assert_eq!(state_of(&reg, id), DownloadJobState::Queued);
        assert!(reg.jobs()[0].started_at.is_none());
        reg.start(id);
        assert_eq!(state_of(&reg, id), DownloadJobState::Downloading);
        assert!(reg.jobs()[0].started_at.is_some());
        reg.progress(id, 40, Some(100));
        assert_eq!(reg.jobs()[0].downloaded, 40);
        assert_eq!(reg.jobs()[0].total, Some(100));
        reg.begin_install(id);
        assert_eq!(state_of(&reg, id), DownloadJobState::Installing);
        reg.finish(id, Ok(()));
        assert_eq!(state_of(&reg, id), DownloadJobState::Done);
        assert!(reg.jobs()[0].finished_at.is_some());
        assert!(reg.jobs()[0].fail_reason.is_none());
    }

    #[test]
    fn job_fails_with_its_reason_kept() {
        let (mut reg, ids) = registry_with(&["Alpha"]);
        let id = ids[0];
        reg.start(id);
        reg.finish(id, Err("Download interrupted".to_string()));
        assert_eq!(state_of(&reg, id), DownloadJobState::Failed);
        assert_eq!(
            reg.jobs()[0].fail_reason.as_deref(),
            Some("Download interrupted")
        );
        assert!(reg.jobs()[0].finished_at.is_some());
    }

    #[test]
    fn transitions_refuse_to_skip_or_resurrect() {
        let (mut reg, ids) = registry_with(&["Alpha"]);
        let id = ids[0];
        // Installing out of Queued is not a step.
        reg.begin_install(id);
        assert_eq!(state_of(&reg, id), DownloadJobState::Queued);
        // A finished job stays finished: progress, install
        // and finish alike are dropped after Done.
        reg.start(id);
        reg.finish(id, Ok(()));
        reg.progress(id, 10, Some(10));
        assert_eq!(reg.jobs()[0].downloaded, 0);
        reg.begin_install(id);
        assert_eq!(state_of(&reg, id), DownloadJobState::Done);
        reg.finish(id, Err("too late".to_string()));
        assert_eq!(state_of(&reg, id), DownloadJobState::Done);
        // A queued job cannot finish before it starts, and
        // a done job cannot be started back up.
        let (mut reg2, ids2) = registry_with(&["Beta"]);
        reg2.finish(ids2[0], Ok(()));
        assert_eq!(state_of(&reg2, ids2[0]), DownloadJobState::Queued);
        reg2.start(ids2[0]);
        reg2.finish(ids2[0], Ok(()));
        assert_eq!(state_of(&reg2, ids2[0]), DownloadJobState::Done);
        reg2.start(ids2[0]);
        assert_eq!(state_of(&reg2, ids2[0]), DownloadJobState::Done);
    }

    #[test]
    fn percent_math_needs_a_real_total() {
        let (mut reg, ids) = registry_with(&["Alpha"]);
        let id = ids[0];
        reg.start(id);
        // No total yet: bytes alone, no percent.
        reg.progress(id, 500, None);
        assert_eq!(reg.jobs()[0].percent(), None);
        reg.progress(id, 50, Some(200));
        assert_eq!(reg.jobs()[0].percent(), Some(25.0));
        // An over-run (server understated its length) clamps.
        reg.progress(id, 250, Some(200));
        assert_eq!(reg.jobs()[0].percent(), Some(100.0));
        // A zero total is no total.
        let (mut reg2, ids2) = registry_with(&["Beta"]);
        reg2.start(ids2[0]);
        reg2.progress(ids2[0], 0, Some(0));
        assert_eq!(reg2.jobs()[0].percent(), None);
    }

    #[test]
    fn second_registration_queues_and_dequeues_on_completion() {
        let (mut reg, ids) = registry_with(&["Alpha", "Beta"]);
        let (a, b) = (ids[0], ids[1]);
        reg.start(a);
        assert!(reg.has_running());
        // Beta waits: it is the queue's head.
        assert_eq!(state_of(&reg, b), DownloadJobState::Queued);
        assert_eq!(reg.next_queued().map(|j| j.id), Some(b));
        assert_eq!(reg.live_count(), 2);
        // Alpha finishing opens the gate for Beta.
        reg.finish(a, Ok(()));
        assert!(!reg.has_running());
        assert_eq!(reg.next_queued().map(|j| j.id), Some(b));
        reg.start(b);
        assert_eq!(state_of(&reg, b), DownloadJobState::Downloading);
        assert!(reg.next_queued().is_none());
    }

    #[test]
    fn a_failed_download_also_releases_the_queue() {
        let (mut reg, ids) = registry_with(&["Alpha", "Beta"]);
        reg.start(ids[0]);
        assert!(reg.has_running());
        reg.finish(ids[0], Err("Download was refused".to_string()));
        assert!(!reg.has_running());
        assert_eq!(reg.next_queued().map(|j| j.id), Some(ids[1]));
    }

    #[test]
    fn clear_finished_sweeps_only_finished_rows() {
        let (mut reg, ids) = registry_with(&["Alpha", "Beta", "Gamma"]);
        reg.start(ids[0]);
        reg.finish(ids[0], Ok(()));
        reg.start(ids[1]);
        reg.finish(ids[1], Err("nope".to_string()));
        // Gamma never got a turn: it is still queued, and
        // the sweep must leave it (and any live job) alone.
        reg.clear_finished();
        let remaining: Vec<u64> = reg.jobs().iter().map(|j| j.id).collect();
        assert_eq!(remaining, vec![ids[2]]);
        assert_eq!(state_of(&reg, ids[2]), DownloadJobState::Queued);
    }

    #[test]
    fn live_job_for_mod_finds_the_newest_live_one() {
        let (mut reg, ids) = registry_with(&["Alpha"]);
        reg.start(ids[0]);
        reg.finish(ids[0], Ok(()));
        // The finished job is history, not a live readout.
        assert!(reg.live_job_for_mod("id-Alpha").is_none());
        let again = reg.register(
            "id-Alpha".to_string(),
            "Alpha".to_string(),
            "FS25_Alpha.zip".to_string(),
            "ModHub".to_string(),
            "3.0.0.0".to_string(),
        );
        assert_eq!(reg.live_job_for_mod("id-Alpha").map(|j| j.id), Some(again));
    }
}

/// A small filled down-triangle drawn with canvas paths — the
/// Show: dropdown field's tick. Drawn rather than typed because
/// the UI font has no ▾ glyph (U+25BE rendered as a hollow box on
/// John's Windows build, the same failure as the star — see
/// mod_card). White, matching the field button's text color.
pub struct DropdownTick;

impl<Message> iced::widget::canvas::Program<Message> for DropdownTick {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        _renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        use iced::widget::canvas::{Frame, Path};
        let mut frame = Frame::new(_renderer, bounds.size());
        // A squat triangle like the ▾ glyph: full width, roughly
        // three-fifths of the height, centered vertically.
        let w = bounds.width;
        let h = bounds.height * 0.62;
        let top = (bounds.height - h) / 2.0;
        let triangle = Path::new(|b| {
            b.move_to(iced::Point::new(0.0, top));
            b.line_to(iced::Point::new(w, top));
            b.line_to(iced::Point::new(w / 2.0, top + h));
            b.close();
        });
        frame.fill(&triangle, Color::WHITE);
        vec![frame.into_geometry()]
    }
}

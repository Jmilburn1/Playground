use crate::app_update::{self, AppRelease};
use crate::config::ModSources;
use crate::mod_manager::conflict::{Conflict, ConflictDetector, ConflictType};
use crate::mod_manager::game_files;
use crate::mod_manager::gamescreens::{self, ActiveIdentity, GsPreset, RgbImage};
use crate::mod_manager::keybindings::{self, BindingsFile, KbAction};
use crate::mod_manager::icons::{
    icon_cache_dir, icon_cache_key, load_icon_cached, sweep_icon_cache, LoadedIcon,
};
use crate::mod_manager::layout::{self, LayoutPlan};
use crate::mod_manager::layout_exec::{
    self, ApplyOutcome, LayoutState, MigrationStatus, RollbackOutcome,
};
use crate::mod_manager::savegame::{self, CareerSettings, FarmFinances, RecordedMod};
use crate::mod_manager::saves::SaveSlot;
use crate::mod_manager::types::{Mod, ModState};
use crate::mod_manager::{ModManager, PageSettings};
use crate::sources::github::GithubSource;
use crate::ui::components::update_types::{UpdateInfo, UpdateState};
use crate::ui::theme;
use crate::update_db::{self, UpdateRow};
use crate::ui::components::{
    format_size, mod_card, BrowseView, DownloadEvent, DownloadHistoryEntry,
    DownloadJob, DownloadJobState, DownloadRegistry, DownloadsTab, DropdownTick,
    FolderTarget,
    LaunchSetting, LayoutEvent, LayoutRunOutcome, LoadEvent, LoadOutcome,
    LoadPhase, LogKind, LogLine, LogOutcome, Message, ScanOutcome,
    OtherLinkRow, SettingsTab, SourceList, Tab, UpdateDbPresentation,
    UpdateJudgement,
};
use futures::SinkExt as _;
use iced::widget::image::Handle as ImageHandle;
use iced::widget::{
    button, checkbox, column, container, mouse_area, pick_list, progress_bar, row,
    scrollable, stack, text, text_input, Scrollable, Space,
};
use iced::{Element, Length, Size, Subscription, Task, Theme};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// The Game Screens presets' master art, bundled with
/// the app (the two owner-approved Farmer Jay pairs,
/// productized from the hand-built splashes of
/// 2026-10-22).
const DAYLIGHT_MASTER: &[u8] = include_bytes!("../../assets/farmerjay_daylight.png");
const DARK_MASTER: &[u8] = include_bytes!("../../assets/farmerjay_dark.png");

/// What the Game Screens page's Apply would write: the
/// stock screens (a restore of the oldest capture), one
/// of the bundled presets, or the user's own image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GsChoice {
    Standard,
    Preset(GsPreset),
    Custom,
}

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
/// How long the boot handover waits for the splash's
/// position read before restoring shape without the
/// centering move. A healthy shell answers within a
/// frame or two; the deadline exists because the read
/// rides a oneshot task whose stream can end silently —
/// the wait must be bounded, or the never-answer case
/// would take the whole restore with it (the v1.2.1
/// field bug; see restore_window).
const RESTORE_POSITION_WAIT: std::time::Duration =
    std::time::Duration::from_millis(500);
/// How long after the batched restore fires before the
/// verify net reads the window's size back: long enough
/// that a landed restore demonstrably reads 1280x800,
/// so a splash-sized read means the restore truly
/// never landed.
const RESTORE_VERIFY_DELAY: std::time::Duration =
    std::time::Duration::from_millis(600);
/// The boot handover's clock interval while it is live.
const RESTORE_TICK: std::time::Duration = std::time::Duration::from_millis(50);
/// Hold-to-delete (feel pass, 2026-10-23): how long a
/// routine Remove control must be held before the delete
/// fires, and the hold clock's tick while one is live.
/// Every collapsible Settings section's id (ST15,
/// cosmetic round): entering the Settings page seeds
/// the collapsed set with all of them, so the page
/// always arrives fully closed.
const SETTINGS_SECTION_IDS: &[&str] = &[
    "general-theme",
    "general-updates",
    "general-behavior",
    "general-launch",
    "general-graphics",
    "folders-paths",
    "library-autofill",
    "library-catalog",
    "library-layout",
    "library-folder",
    "accounts-github",
    "diagnostics-summary",
    "diagnostics-log-reader",
    "maintenance-reread",
    "maintenance-downloads",
    "maintenance-fill-log",
];

const REMOVE_HOLD_TIME: std::time::Duration = std::time::Duration::from_millis(700);
const REMOVE_HOLD_TICK: std::time::Duration = std::time::Duration::from_millis(25);
/// The feel pass's animation constants (2026-10-23):
/// one 16ms clock drives every micro-animation; a card
/// switch glides over 160ms; the clock stays alive for
/// a 900ms tail after the last kick so settles finish.
const ANIM_TICK: std::time::Duration = std::time::Duration::from_millis(16);
const SWITCH_ANIM_DUR: std::time::Duration = std::time::Duration::from_millis(160);
const ANIM_TAIL: std::time::Duration = std::time::Duration::from_millis(900);

/// One card switch's glide (feel pass, 2026-10-23):
/// the knob eases from `from` to `to` (the 0.0/1.0
/// ends) over SWITCH_ANIM_DUR from `started`.
struct SwitchAnim {
    from: f32,
    to: f32,
    started: std::time::Instant,
}
const SIDEBAR_W: f32 = 232.0;
const GRID_SPACING: f32 = 12.0;
const DETAILS_W: f32 = 344.0;
/// The right margin every page keeps between its content
/// and the scrollbar / window edge (John, 2026-10-22): it
/// mirrors the shell's 10px sidebar-to-content spacing
/// (see view()), so a page breathes the same on both
/// sides. No full-bleed exceptions — the details pane and
/// the banners keep it too.
const PAGE_MARGIN: f32 = 10.0;

/// Iced's default scrollbar band: 10px wide, painted
/// INSIDE the scrollable's right edge with no layout
/// space reserved for it. A bare PAGE_MARGIN of pad is
/// therefore the bar's own band, not clearance beside
/// it — surfaces that must sit clear of the bar allow
/// PAGE_MARGIN + SCROLLBAR_W on the right, so the
/// visible gap between content and bar is a true
/// PAGE_MARGIN. (Field diagnosis, 2026-10-22: at
/// PAGE_MARGIN alone the Settings panels and the card
/// grids sat flush against the bar.)
const SCROLLBAR_W: f32 = 10.0;

/// A page's scrollable with PAGE_MARGIN of room on the
/// content's right (John's app-wide margin rule). The
/// shell's row spacing already gives every page the same
/// 10px on its left, so only the right side needs the
/// pad — and it goes on the scrollable's child, never on
/// the shell's main stack (padding the shell would shift
/// the header and status slabs and still leave the
/// computed-width grids wrong). With the pad inside, the
/// scrollbar sits at the padded edge, clear of the
/// content it used to paint over. Surfaces that must
/// clear the bar itself use page_scroll_pad with a
/// deeper pad (see SCROLLBAR_W).
fn page_scroll<'a>(content: impl Into<Element<'a, Message>>) -> Scrollable<'a, Message> {
    page_scroll_pad(content, PAGE_MARGIN)
}

/// page_scroll with a caller-chosen right pad.
fn page_scroll_pad<'a>(
    content: impl Into<Element<'a, Message>>,
    right_pad: f32,
) -> Scrollable<'a, Message> {
    scrollable(
        container(content)
            .padding(iced::Padding {
                right: right_pad,
                ..Default::default()
            })
            .width(Length::Fill),
    )
}

/// A lone button label, centered across the button's full
/// width (John, 2026-10-21: button text centered, app-wide).
/// Iced 0.13 lays a button's content out from its top-left
/// corner, so a single label in a wide button hugs the left
/// edge; wrapping the label in a Fill-width container that
/// centers its child is what puts it in the middle. Only
/// wide buttons need the wrap — a shrink-width button hugs
/// its label with symmetric padding, so its label is
/// already centered by construction (and a Fill wrap inside
/// one would balloon its width).
fn centered_label<'a>(label: iced::widget::text::Text<'a>) -> Element<'a, Message> {
    container(label)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

/// The Browse tiles' counts, tallied from the record set
/// itself: total records, and how many sit in each of the
/// two states the Browse filters name. A record in any
/// other state (Conflict, Error) counts toward the total
/// alone — the tiles never fudge it into either column.
fn tally_states(states: impl Iterator<Item = ModState>) -> (usize, usize, usize) {
    let (mut total, mut active, mut archived) = (0usize, 0usize, 0usize);
    for state in states {
        total += 1;
        match state {
            ModState::Active => active += 1,
            ModState::Archived => archived += 1,
            _ => {}
        }
    }
    (total, active, archived)
}

/// Configure Save: money with thousands separators —
/// "$181,992". The wizard's cash and loan speak in whole
/// amounts; a negative keeps its sign.
fn fmt_money(v: i64) -> String {
    let digits = v.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{}${}", if v < 0 { "-" } else { "" }, grouped)
}

/// The wizard's panel: petrol fill, soft corners — the
/// app's current language (theme palette), used for the
/// wizard's sections and summary blocks.
fn wizard_panel(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme::panel())),
        border: iced::Border {
            color: iced::Color::TRANSPARENT,
            width: 0.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// The wizard's inset box (a path display inside a panel):
/// the near-black base, so it reads as a well in the panel.
fn wizard_inset(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme::base())),
        border: iced::Border {
            color: iced::Color::TRANSPARENT,
            width: 0.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// The section box's own style: wizard_panel's shape
/// (soft corners, no border) on the near-black base —
/// the left menu's background (John, 2026-10-22: the
/// boxes match the menu). wizard_panel itself keeps
/// the petrol fill: it also dresses non-section boxes
/// (the wizard's missing-mods and results blocks),
/// which this ruling does not reach.
fn wizard_section_box(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme::base())),
        border: iced::Border {
            color: iced::Color::TRANSPARENT,
            width: 0.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// One titled section of the Settings step.
fn wizard_section<'a>(
    title: &'a str,
    body: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut col = column![text(title).size(15).color(theme::text())].spacing(7);
    for el in body {
        col = col.push(el);
    }
    container(col)
        .width(Length::Fill)
        .padding(11)
        .style(wizard_section_box)
        .into()
}

/// A settings row that may not exist in this save: the
/// control when the save carries the field, the honest
/// absence line when it doesn't.
fn wizard_opt_row<'a>(
    label: &'a str,
    present: bool,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    if present {
        wizard_row(label, control)
    } else {
        wizard_absent(label)
    }
}

/// A labelled settings row: the label takes a fixed
/// column so the controls line up down the page.
fn wizard_row<'a>(label: &'a str, control: Element<'a, Message>) -> Element<'a, Message> {
    row![
        text(label).size(13).color(theme::text()).width(200),
        control,
    ]
    .spacing(10)
    .into()
}

/// A settings row for a field the save doesn't carry:
/// named, and honestly marked — never given a control
/// that would invent it (owner ruling, 2026-10-21).
fn wizard_absent<'a>(label: &'a str) -> Element<'a, Message> {
    row![
        text(label).size(13).color(theme::text_dim()).width(200),
        text("Not in this save yet").size(12).color(theme::text_dim()),
    ]
    .spacing(10)
    .into()
}

/// Two pills for a boolean field, with the player's own
/// words for the ends ("Off"/"On", "Off"/"Buy").
fn wizard_bool_pills<'a>(
    key: &str,
    value: bool,
    off_label: &'a str,
    on_label: &'a str,
) -> Element<'a, Message> {
    let key = key.to_string();
    row![
        button(text(off_label).size(12))
            .style(theme::nav_pill(!value))
            .on_press(Message::WizardSetBool(key.clone(), false)),
        button(text(on_label).size(12))
            .style(theme::nav_pill(value))
            .on_press(Message::WizardSetBool(key, true)),
    ]
    .spacing(4)
    .into()
}

/// A row of option pills for an enum/number field.
/// Options are (stored value, label); the current value
/// is lit. A current value outside the option set is
/// added as its own lit pill, so the save's real value
/// is never hidden from its owner.
fn wizard_segment<'a>(
    key: &str,
    options: Vec<(String, String)>,
    current: &str,
) -> Element<'a, Message> {
    let mut options = options;
    if !options.iter().any(|(v, _)| v == current) {
        options.push((current.to_string(), current.to_string()));
    }
    let key = key.to_string();
    let mut r = row![].spacing(4);
    for (value, label) in options {
        let selected = value == current;
        r = r.push(
            button(text(label).size(12))
                .style(theme::nav_pill(selected))
                .on_press(Message::WizardSetValue(key.clone(), value)),
        );
    }
    r.into()
}

/// A small rounded label chip, in the mod card's pill
/// shape but the theme palette.
fn wizard_pill(
    label: String,
    bg: iced::Color,
    fg: iced::Color,
) -> Element<'static, Message> {
    container(text(label).size(10).color(fg))
        .padding([2, 7])
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(bg)),
            border: iced::Border {
                color: iced::Color::TRANSPARENT,
                width: 0.0,
                radius: 9.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// The boot handover (splash -> main window) as a small
/// state machine, driven by the Restore* messages and a
/// short-lived clock subscription. It exists so the
/// frame, the 1280x800 resize, and the centering move
/// can be collected first and fired as ONE batched shape
/// change (owner, 2026-10-21: the previous sequence
/// landed as three visible steps — the frame appeared
/// at splash size, the window blew out to full size
/// wherever it sat, then hopped to center).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreState {
    /// No handover in flight: before the boot load
    /// lands, during every later load, and once a
    /// handover has fully settled.
    Idle,
    /// The window id is in; the splash position read is
    /// in flight. When it lands, the batched restore
    /// fires whole. If it has not landed by the deadline,
    /// the restore fires without the centering move
    /// instead: the read is a oneshot that can end
    /// silently, so it is never load-bearing for the
    /// window's shape (the v1.2.1 field bug).
    Collecting {
        id: iced::window::Id,
        deadline: std::time::Instant,
    },
    /// The batched restore has been issued; the verify
    /// net's size read comes due at `check_at`.
    Verifying {
        id: iced::window::Id,
        check_at: std::time::Instant,
    },
    /// The verify net's size read is in flight.
    VerifyRead { id: iced::window::Id },
    /// The handover is over: the net read the size back
    /// and acted (or found nothing to do).
    Settled,
}

impl RestoreState {
    /// Whether the handover clock should be running:
    /// only the timed states need it.
    fn ticking(&self) -> bool {
        matches!(
            self,
            RestoreState::Collecting { .. } | RestoreState::Verifying { .. }
        )
    }
}

/// Which Layout background job is in flight, if any
/// (Roadmap #2, Phase 4). The page disables its actions
/// while one runs and shows the apply job's op counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayoutRun {
    Apply,
    Rollback,
}

/// One mod as the Configure Save loadout step shows it:
/// the card's facts, plus the draft key it toggles by.
#[derive(Debug, Clone)]
struct WizardModEntry {
    key: String,
    id: String,
    name: String,
    author: Option<String>,
    version: String,
    category: Option<String>,
    required: bool,
}

/// A money field's text as a whole number: thousands
/// separators, spaces, and a stray currency symbol are
/// forgiven; anything else doesn't parse.
fn parse_amount(text: &str) -> Option<i64> {
    let cleaned: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    if cleaned.is_empty() || cleaned == "-" {
        return None;
    }
    cleaned.parse::<i64>().ok()
}

/// Everything needed to actually start one registered
/// download job when its turn comes: the update offer it
/// was clicked from, where the bytes land, and the GitHub
/// token in force at registration. Kept beside the job
/// registry (which holds the display state) keyed by job
/// id, and consumed when the job starts.
struct PendingDownload {
    mod_id: String,
    info: UpdateInfo,
    dest: PathBuf,
    token: Option<String>,
}

/// The stored key for a Browse default view (Settings,
/// General > Behavior) and back. Unknown keys — and a
/// category key with no name — fall back to All: a
/// hand-edited config can never strand a launch on a
/// view that doesn't exist.
fn browse_view_from_key(key: &str) -> BrowseView {
    match key {
        "active" => BrowseView::Active,
        "archived" => BrowseView::Archived,
        "favorites" => BrowseView::Favorites,
        "updates" => BrowseView::Updates,
        _ => match key.strip_prefix("category:") {
            Some(name) if !name.is_empty() => {
                BrowseView::Category(name.to_string())
            }
            _ => BrowseView::All,
        },
    }
}

/// `browse_view_from_key`'s other direction.
fn browse_view_key(view: &BrowseView) -> String {
    match view {
        BrowseView::All => "all".to_string(),
        BrowseView::Active => "active".to_string(),
        BrowseView::Archived => "archived".to_string(),
        BrowseView::Favorites => "favorites".to_string(),
        BrowseView::Updates => "updates".to_string(),
        BrowseView::Category(name) => format!("category:{}", name),
    }
}

/// Where the Downloads history file lives: beside the
/// other stores in the data folder.
fn download_history_path() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("downloads-history.json"))
}

/// Read the stored Downloads history, when the file
/// exists and parses. A missing or unreadable file is
/// simply no history — never an error worth a launch.
fn load_download_history() -> Option<Vec<DownloadHistoryEntry>> {
    let path = download_history_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Replace the user's home path with its masked form
/// ("C:\Users\<user>\...", "/home/<user>/...") everywhere
/// it appears — the Diagnostics summary is text a user
/// may paste somewhere public, and it prints real paths
/// (data folder, catalog cache). Pure, for testing.
fn scrub_home_prefix(text: &str, home: &std::path::Path) -> String {
    let home = home.to_string_lossy();
    if home.is_empty() {
        return text.to_string();
    }
    let masked = match std::path::Path::new(home.as_ref()).parent() {
        Some(parent) => format!(
            "{}{}{}",
            parent.to_string_lossy(),
            std::path::MAIN_SEPARATOR,
            "<user>"
        ),
        None => "<user>".to_string(),
    };
    text.replace(home.as_ref(), &masked)
}

/// Ask Windows whether the scheduled fill's task is
/// registered. None when the query cannot run at all
/// (not Windows, or schtasks missing); Some(false) when
/// it ran and the task is not there.
#[cfg(windows)]
fn query_fill_task_registered() -> Option<bool> {
    std::process::Command::new("schtasks")
        .args(["/query", "/tn", "Playground Update Fill"])
        .output()
        .ok()
        .map(|out| out.status.success())
}

/// Non-Windows: there is no Task Scheduler to ask.
#[cfg(not(windows))]
fn query_fill_task_registered() -> Option<bool> {
    None
}

/// Open a folder in the OS file explorer (creating it
/// first — the fill-log folder may not exist yet).
fn open_folder(path: &std::path::Path) -> std::io::Result<()> {
    let _ = std::fs::create_dir_all(path);
    spawn_opener(&path.to_string_lossy(), false)
}

/// Open a URL in the default browser.
fn open_url(url: &str) -> std::io::Result<()> {
    spawn_opener(url, true)
}

/// The platform opener: explorer / open / xdg-open.
/// Spawning is fire-and-forget; only a spawn failure is
/// an error worth reporting.
#[cfg(windows)]
fn spawn_opener(target: &str, is_url: bool) -> std::io::Result<()> {
    if is_url {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .spawn()?;
    } else {
        // explorer returns 1 even on success; the spawn
        // landing is all that can be known here.
        std::process::Command::new("explorer").arg(target).spawn()?;
    }
    Ok(())
}

/// The platform opener (macOS).
#[cfg(target_os = "macos")]
fn spawn_opener(target: &str, _is_url: bool) -> std::io::Result<()> {
    std::process::Command::new("open").arg(target).spawn()?;
    Ok(())
}

/// The platform opener (other Unix).
#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_opener(target: &str, _is_url: bool) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(target).spawn()?;
    Ok(())
}

/// What an armed key capture will do with the next key
/// press on the Keybindings page: rewrite one existing
/// binding's input, or append a new binding to an action
/// that has none on the keyboard device.
#[derive(Debug, Clone)]
enum KbCapture {
    Replace { action: String, pos: usize },
    Add { action: String },
}

pub struct App {
    mod_manager: ModManager,
    current_tab: Tab,
    view_filter: BrowseView,
    search_query: String,
    favorites_only: bool,
    selected_mod: Option<String>,
    /// The live hold-to-delete, if any: the mod id and
    /// when the press began. Progress is derived from the
    /// instant at render; the delete fires from the tick
    /// once REMOVE_HOLD_TIME elapses (feel pass,
    /// 2026-10-23 — replaces the arm-to-confirm
    /// pending_remove for routine single-mod deletes).
    remove_hold: Option<(String, std::time::Instant)>,
    /// The feel pass's animation state (2026-10-23):
    /// `anim_epoch` phases the indeterminate pulse;
    /// `anim_until` keeps the shared AnimTick clock
    /// subscribed while anything is settling;
    /// `switch_anims` holds the card switches mid-glide;
    /// `shown_counts` the rolling numbers (displayed
    /// value, target); `eased_bars` the progress bars'
    /// displayed fractions; `hovered_card` the card the
    /// mouse is over (it lifts).
    anim_epoch: std::time::Instant,
    anim_until: std::time::Instant,
    switch_anims: HashMap<String, SwitchAnim>,
    shown_counts: HashMap<&'static str, (f32, usize)>,
    eased_bars: HashMap<String, f32>,
    hovered_card: Option<String>,
    /// Feel pass part 2 (2026-10-23): the status line's
    /// fade state — the last status the animation clock
    /// has seen, and when the current one arrived.
    status_seen: String,
    status_changed_at: std::time::Instant,
    /// When the details panel / the Show: menu last
    /// opened — their settle offsets decay from these.
    details_opened_at: std::time::Instant,
    menu_opened_at: std::time::Instant,
    /// Smooth scrolling (feel pass part 2): the grid's
    /// eased offset. grid_offset stays the target the
    /// scrollable reports; grid_shown is what renders,
    /// gliding toward the target while the clock issues
    /// the matching scroll_to. grid_self_scroll marks
    /// the echo of our own scroll_to so it is never
    /// mistaken for the user's own scroll.
    grid_shown: f32,
    grid_self_scroll: bool,
    conflicts: Vec<Conflict>,
    icons: HashMap<String, ImageHandle>,
    active_dir_input: String,
    archive_dir_input: String,
    /// Settings: the profile folder display + the console
    /// switch state. Read from game.xml when the folder is
    /// picked; since Round 2 the switch is a staged Settings
    /// value (Launch Configuration) that writes game.xml when
    /// the page saves. None = no switch state known.
    game_dir_input: String,
    console_on: Option<bool>,
    /// Settings: the true game install folder display (the
    /// one holding `shared`, for Game Screens). Its own
    /// setting, picked and validated separately from the
    /// profile folder above — the two are different places.
    game_install_dir_input: String,
    /// Settings page staging (Round 1): the Library and
    /// Catalog & Sources values as edited on the page.
    /// Nothing here touches the config until Save folders
    /// — the page's one explicit save — commits all of it
    /// at once (see ModManager::save_settings_page).
    settings_check_conflicts: bool,
    settings_parallel_input: String,
    settings_cache_ttl_input: String,
    settings_catalog_url_input: String,
    settings_sources: ModSources,
    /// Settings page staging (Round 2): the tabbed page's
    /// state. The open tab and the boxes' collapse state
    /// are view state; every value below stages exactly
    /// like Round 1's — nothing touches the config until
    /// the page's one explicit save commits all of it at
    /// once (see apply_directories / save_settings_page).
    /// The Developer Console stages in `console_on` above:
    /// since Round 2 it is a staged value too, written to
    /// game.xml at save time.
    settings_tab: SettingsTab,
    settings_collapsed: HashSet<String>,
    settings_startup_check: bool,
    settings_self_update: bool,
    settings_browse_default: BrowseView,
    settings_start_maximized: bool,
    settings_launch: crate::config::LaunchConfig,
    settings_launch_slot_input: String,
    settings_auto_file: bool,
    settings_keep_history: bool,
    settings_fill_log_keep_input: String,
    settings_log_errors_only_default: bool,
    settings_log_lines_cap_input: String,
    /// The scheduled-fill task query's answer, once it has
    /// run (Settings, General > Updates / Diagnostics):
    /// None = not asked yet or the query cannot run here.
    fill_task_registered: Option<bool>,
    /// A "Check now" press is outstanding: when its check
    /// answers with nothing, the status line says so (the
    /// launch check stays silent by design; a manual one
    /// was asked out loud).
    app_update_check_manual: bool,
    /// A status note to show once the current load finishes
    /// (e.g. the folder switcher's gameSettings.xml result,
    /// which a rescan would otherwise overwrite).
    pending_status: Option<String>,
    status: String,
    loading: bool,
    /// True from launch until the first load and the icon
    /// pass behind it have both finished: while it holds, the
    /// splash owns the window and the main UI isn't rendered
    /// at all. Later loads (Rescan, folder changes) never
    /// set it again — they report progress through the
    /// status line like before.
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
    /// The same stand-down flag for a card's activate slider.
    just_toggled_state: bool,
    /// Configure Save wizard state. The step is 0-based:
    /// 0 Select, 1 Settings, 2 Loadout, 3 Review, 4 Results.
    /// `wizard_original` is the save as read from disk; the
    /// draft beside it is what the player edits, and Review
    /// diffs the two.
    save_slots_cache: Vec<SaveSlot>,
    saves_selected: Option<String>,
    saves_draft: HashSet<String>,
    saves_filter: String,
    wizard_step: u8,
    wizard_original: Option<CareerSettings>,
    wizard_settings: Option<CareerSettings>,
    wizard_finances: Option<FarmFinances>,
    wizard_other_names: Vec<String>,
    wizard_cash: String,
    wizard_loan: String,
    wizard_recorded: Vec<RecordedMod>,
    wizard_missing: Vec<String>,
    wizard_seed_note: String,
    wizard_autodrive: Option<PathBuf>,
    wizard_autodrive_existing: bool,
    wizard_collapsed: HashSet<String>,
    wizard_arm_apply: bool,
    wizard_done: Vec<String>,
    wizard_failed: Vec<String>,
    /// Log Reader state.
    log_lines: Vec<LogLine>,
    log_suspects: Vec<String>,
    log_error_count: usize,
    log_warning_count: usize,
    log_total_lines: usize,
    log_errors_only: bool,
    log_note: String,
    /// Keybindings page state. The document is a staged
    /// working copy of the profile's inputBinding.xml:
    /// every edit lands on it, and nothing touches the
    /// file on disk until the page's explicit Save —
    /// the Settings one-save pattern. `kb_original` is
    /// the text as loaded, the discard target and the
    /// dirty count's baseline.
    kb_doc: Option<BindingsFile>,
    kb_original: Option<String>,
    kb_profile: Option<PathBuf>,
    kb_device: Option<String>,
    kb_search: String,
    kb_unassigned_only: bool,
    kb_conflicts_only: bool,
    kb_show_archived: bool,
    kb_collapsed: HashSet<String>,
    kb_capture: Option<KbCapture>,
    kb_backups: Vec<(PathBuf, u64)>,
    kb_reset_armed: bool,
    /// The purge's armed report: (action blocks,
    /// distinct archived mods) counted on the first
    /// press; the second press stages the removals and
    /// clears this. Any reload, discard or save clears
    /// it too — the count must never outlive the document
    /// it was counted on.
    kb_purge: Option<(usize, usize)>,
    /// Game Screens page state. The page is a working
    /// surface, not staged state: reads are live (the
    /// current splash, its hash identity, the backups),
    /// and writes happen only on an explicit Apply or
    /// Restore, each preceded by a capture of the current
    /// files (see mod_manager::gamescreens). The pending
    /// previews are the exact composition an Apply would
    /// write, at preview size.
    gs_shared: Option<PathBuf>,
    gs_root: Option<PathBuf>,
    gs_error: Option<String>,
    /// The install folder the page resolved to (when it
    /// did) — shown on the page so the location in use is
    /// never a guess.
    gs_install: Option<PathBuf>,
    /// The resolution failed for want of a usable install
    /// folder: the error state carries the Locate picker.
    gs_needs_locate: bool,
    /// Why the last install-folder pick was refused (what
    /// was picked, what was missing), shown under the
    /// error until a pick succeeds or the page reloads.
    gs_pick_note: Option<String>,
    gs_live_canvas: Option<ImageHandle>,
    gs_live_band: Option<ImageHandle>,
    gs_identity: Option<ActiveIdentity>,
    gs_highlands: bool,
    gs_choice: GsChoice,
    gs_pending_canvas: Option<ImageHandle>,
    gs_pending_band: Option<ImageHandle>,
    gs_pending_note: String,
    gs_custom_name: Option<String>,
    gs_custom_art: Option<RgbImage>,
    gs_backups: Vec<gamescreens::BackupInfo>,
    gs_restore_armed: bool,
    gs_busy: bool,
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
    /// Boot handover (splash -> main window) state; see
    /// RestoreState. Stays Idle for every load after boot.
    window_restore: RestoreState,
    /// Layout page (Roadmap #2, Phase 4): the recorded
    /// migration state as last read from layout-state.json
    /// — None until the page is first opened, Some(None)
    /// when no migration is recorded, Some(Some) with the
    /// record itself.
    layout_standing: Option<Option<LayoutState>>,
    /// The previewed plan (Build plan): the exact work an
    /// Apply click approves. Replaced wholesale by a
    /// rebuild; dropped after a run lands (the world it
    /// described is gone).
    layout_preview: Option<LayoutPlan>,
    /// Arm-to-confirm flags, mirroring Remove's pattern:
    /// the first press arms with the stakes in the label,
    /// the second executes.
    layout_arm_apply: bool,
    layout_arm_rollback: bool,
    /// The Reclaim section's census (Layout page), its
    /// arm-to-confirm flag, and its last run's result.
    reclaim: Option<crate::mod_manager::reclaim::ReclaimCensus>,
    reclaim_arm: bool,
    reclaim_outcome: Option<String>,
    /// The in-flight Layout job and its op counts
    /// (apply only — rollback reports no counts).
    layout_running: Option<LayoutRun>,
    layout_done: usize,
    layout_total: usize,
    /// The last run's result, in the page's own words:
    /// success lines and refusal/failure lines are kept
    /// apart so a failure never reads as a success.
    layout_outcome: Option<String>,
    layout_error: Option<String>,
    /// Downloads section (owner, 2026-10-22): every mod
    /// download this session as a tracked job — the page
    /// renders from the registry, and the details panel's
    /// progress line reads the same job state. Session-only
    /// by design: the registry starts empty every launch.
    downloads: DownloadRegistry,
    /// Which of the Downloads page's tabs is showing
    /// (Jobs / Other Links, owner 2026-10-24).
    downloads_tab: DownloadsTab,
    /// Start payloads for registered jobs that have not
    /// begun (queued, or the one about to start), by job id.
    pending_downloads: HashMap<u64, PendingDownload>,
    /// App self-update (owner, 2026-10-22): the launch
    /// check's finding, when a newer Playground release
    /// exists. The notice over the status bar renders
    /// from these three alone; nothing is persisted, so
    /// a dismissal lasts only this launch.
    app_update: Option<AppRelease>,
    /// The Install click's job (download + verify +
    /// stage) is in flight.
    app_update_installing: bool,
    /// The last install attempt's failure, shown in the
    /// notice until dismissed or retried.
    app_update_failed: Option<String>,
}

impl App {
    pub fn new(mod_manager: ModManager) -> Self {
        // Wear the stored theme from the first frame
        // (themes, 2026-10-23): an unknown id leaves the
        // Gunmetal default standing.
        theme::set_theme(mod_manager.ui_theme());
        let active_dir_input = mod_manager.get_active_dir().display().to_string();
        let archive_dir_input = mod_manager.get_archive_dir().display().to_string();
        let game_dir_input = mod_manager
            .game_directory()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let game_install_dir_input = mod_manager
            .game_install_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let console_on = mod_manager
            .game_directory()
            .and_then(|d| game_files::console_enabled(&d));
        let settings_check_conflicts = mod_manager.check_conflicts();
        let settings_parallel_input = mod_manager.parallel_scans().to_string();
        let settings_cache_ttl_input = mod_manager.cache_ttl_hours().to_string();
        let settings_catalog_url_input = mod_manager.catalog_url();
        let settings_sources = mod_manager.mod_sources();
        // Round 2 staging, read from the same config the
        // page will write back on save.
        let settings_startup_check = mod_manager.startup_update_check();
        let settings_self_update = mod_manager.self_update_enabled();
        let settings_browse_default =
            browse_view_from_key(&mod_manager.browse_default_view());
        let settings_start_maximized = mod_manager.start_maximized();
        let settings_launch = mod_manager.launch_config();
        let settings_launch_slot_input = settings_launch
            .slot
            .map(|s| s.to_string())
            .unwrap_or_default();
        let settings_auto_file = mod_manager.auto_file_new_mods();
        let settings_keep_history = mod_manager.keep_download_history();
        let settings_fill_log_keep_input = mod_manager.fill_log_keep().to_string();
        let settings_log_errors_only_default = mod_manager.log_errors_only_default();
        let settings_log_lines_cap_input = mod_manager.log_lines_cap().to_string();
        // The boot Browse view is the configured default —
        // this constructor is the one place a launch reads
        // it (a save mid-session never moves the view).
        let boot_view = settings_browse_default.clone();
        // Downloads history (Round 2): last session's
        // finished rows come back when the setting keeps
        // them; the stored file is read-only here — turning
        // the setting off later leaves it untouched.
        let mut downloads = DownloadRegistry::new();
        if settings_keep_history {
            if let Some(entries) = load_download_history() {
                downloads.restore_history(entries);
            }
        }
        Self {
            mod_manager,
            current_tab: Tab::Browse,
            view_filter: boot_view,
            search_query: String::new(),
            favorites_only: false,
            selected_mod: None,
            remove_hold: None,
            anim_epoch: std::time::Instant::now(),
            anim_until: std::time::Instant::now(),
            switch_anims: HashMap::new(),
            shown_counts: HashMap::new(),
            eased_bars: HashMap::new(),
            hovered_card: None,
            status_seen: String::new(),
            status_changed_at: std::time::Instant::now(),
            details_opened_at: std::time::Instant::now(),
            menu_opened_at: std::time::Instant::now(),
            grid_shown: 0.0,
            grid_self_scroll: false,
            conflicts: Vec::new(),
            icons: HashMap::new(),
            active_dir_input,
            archive_dir_input,
            game_dir_input,
            game_install_dir_input,
            console_on,
            settings_check_conflicts,
            settings_parallel_input,
            settings_cache_ttl_input,
            settings_catalog_url_input,
            settings_sources,
            settings_tab: SettingsTab::General,
            settings_collapsed: HashSet::new(),
            settings_startup_check,
            settings_self_update,
            settings_browse_default,
            settings_start_maximized,
            settings_launch,
            settings_launch_slot_input,
            settings_auto_file,
            settings_keep_history,
            settings_fill_log_keep_input,
            settings_log_errors_only_default,
            settings_log_lines_cap_input,
            fill_task_registered: None,
            app_update_check_manual: false,
            pending_status: None,
            status: "Loading mods…".to_string(),
            loading: true,
            booting: true,
            load_phase: LoadPhase::Mods,
            load_done: 0,
            load_total: 0,
            just_toggled_favorite: false,
            just_toggled_state: false,
            save_slots_cache: Vec::new(),
            saves_selected: None,
            saves_draft: HashSet::new(),
            saves_filter: String::new(),
            wizard_step: 0,
            wizard_original: None,
            wizard_settings: None,
            wizard_finances: None,
            wizard_other_names: Vec::new(),
            wizard_cash: String::new(),
            wizard_loan: String::new(),
            wizard_recorded: Vec::new(),
            wizard_missing: Vec::new(),
            wizard_seed_note: String::new(),
            wizard_autodrive: None,
            wizard_autodrive_existing: false,
            wizard_collapsed: HashSet::new(),
            wizard_arm_apply: false,
            wizard_done: Vec::new(),
            wizard_failed: Vec::new(),
            log_lines: Vec::new(),
            log_suspects: Vec::new(),
            log_error_count: 0,
            log_warning_count: 0,
            log_total_lines: 0,
            log_errors_only: false,
            log_note: String::new(),
            kb_doc: None,
            kb_original: None,
            kb_profile: None,
            kb_device: None,
            kb_search: String::new(),
            kb_unassigned_only: false,
            kb_conflicts_only: false,
            kb_show_archived: false,
            kb_collapsed: HashSet::new(),
            kb_capture: None,
            kb_backups: Vec::new(),
            kb_reset_armed: false,
            kb_purge: None,
            gs_shared: None,
            gs_root: None,
            gs_error: None,
            gs_install: None,
            gs_needs_locate: false,
            gs_pick_note: None,
            gs_live_canvas: None,
            gs_live_band: None,
            gs_identity: None,
            gs_highlands: false,
            gs_choice: GsChoice::Preset(GsPreset::Dark),
            gs_pending_canvas: None,
            gs_pending_band: None,
            gs_pending_note: String::new(),
            gs_custom_name: None,
            gs_custom_art: None,
            gs_backups: Vec::new(),
            gs_restore_armed: false,
            gs_busy: false,
            update_status: HashMap::new(),
            db_rows: HashMap::new(),
            db_generated_at: None,
            db_summary: update_db::DbSummary::default(),
            db_applied: HashSet::new(),
            link_input: String::new(),
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
            window_restore: RestoreState::Idle,
            layout_standing: None,
            layout_preview: None,
            layout_arm_apply: false,
            layout_arm_rollback: false,
            reclaim: None,
            reclaim_arm: false,
            reclaim_outcome: None,
            layout_running: None,
            layout_done: 0,
            layout_total: 0,
            layout_outcome: None,
            layout_error: None,
            downloads,
            downloads_tab: DownloadsTab::Jobs,
            pending_downloads: HashMap::new(),
            app_update: None,
            app_update_installing: false,
            app_update_failed: None,
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
        let check_conflicts = self.mod_manager.check_conflicts();
        let (tx, rx) = futures::channel::mpsc::channel::<LoadEvent>(128);
        std::thread::spawn(move || load_job(archive, active, parallel, check_conflicts, tx));
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
            if row.source_kind == "kingmods" {
                // KingMods is a manual source (owner,
                // 2026-10-24): its rows never become an
                // install offer, and no download from
                // KingMods is ever attempted. The fill
                // still reads its pages, so the Downloads
                // > Other Links page can list the mods
                // with an update waiting, each with its
                // page link to click through to. (itch.io
                // rows need no such guard: the fill
                // records no download URL for them, so
                // they never form an offer either.)
                self.update_status.remove(&id);
                self.db_applied.insert(id.clone());
                continue;
            }
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
        let cache_ttl_hours = self.mod_manager.cache_ttl_hours();
        Task::perform(
            async move { icon_job(jobs, cache_ttl_hours) },
            Message::IconsLoaded,
        )
    }

    /// Begin one registered download job: flip it to
    /// Downloading and run the SAME download the panel has
    /// always run (GitHub asset, ModHub with the mod page
    /// as referer, or a KingMods offer through its own
    /// lane) — now the reporting variant, so progress
    /// streams into the job registry over a channel while
    /// completion still arrives as UpdateInstalled. Progress
    /// sends may drop when the channel is full: a dropped
    /// sample is replaced by the next one, which is already
    /// coming (the load job's channel discipline).
    fn start_download(&mut self, job: u64) -> Task<Message> {
        let Some(pending) = self.pending_downloads.remove(&job) else {
            return Task::none();
        };
        self.downloads.start(job);
        let PendingDownload {
            mod_id: id,
            info,
            dest,
            token,
        } = pending;
        let url = info.download_url.clone();
        let (tx, rx) = futures::channel::mpsc::channel::<DownloadEvent>(64);
        let progress_task = Task::run(rx, Message::DownloadEvent);
        if let Some(mh_id) = info.repo.strip_prefix("modhub:") {
            // ModHub's CDN wants the mod page as the
            // referer; the GitHub flow doesn't need one.
            let page = crate::sources::mod_hub::page_url(mh_id);
            let dest2 = dest.clone();
            let mut tx2 = tx;
            let download = Task::perform(
                async move {
                    let mut report = move |downloaded: u64, total: Option<u64>| {
                        let _ = tx2.try_send(DownloadEvent::Progress {
                            job,
                            downloaded,
                            total,
                        });
                    };
                    crate::sources::mod_hub::ModHubSource::new()
                        .download_to_reporting(&url, &page, &dest2, &mut report)
                        .await
                        .map(|_| dest2)
                        .map_err(|e| e.to_string())
                },
                move |result| Message::UpdateInstalled(id.clone(), result),
            );
            return Task::batch([download, progress_task]);
        }
        // KingMods offers carry no repo marker — the
        // database row that offered the update names the
        // source (the same lookup the Downloads label
        // uses), and the offer's repo IS the mod's KingMods
        // page, which the lane sends as the referer the
        // file-host chain starts from. Its download URL is
        // the file host's page, resolved by the KingMods
        // lane itself; falling through to the GitHub lane
        // was v1.5.1's refusal bug.
        let kingmods = self
            .mod_manager
            .get_mod(&id)
            .map(|m| {
                let stem = ModManager::mod_stem(&m.path);
                self.db_rows
                    .get(&stem)
                    .map(|row| row.source_kind == "kingmods")
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if kingmods {
            let page = info.repo.clone();
            let dest2 = dest.clone();
            let mut tx2 = tx;
            let download = Task::perform(
                async move {
                    let mut report = move |downloaded: u64, total: Option<u64>| {
                        let _ = tx2.try_send(DownloadEvent::Progress {
                            job,
                            downloaded,
                            total,
                        });
                    };
                    crate::sources::kingmods::KingModsSource::new()
                        .download_to_reporting(&url, &page, &dest2, &mut report)
                        .await
                        .map(|_| dest2)
                        .map_err(|e| e.to_string())
                },
                move |result| Message::UpdateInstalled(id.clone(), result),
            );
            return Task::batch([download, progress_task]);
        }
        let mut tx2 = tx;
        let download = Task::perform(
            async move {
                let mut report = move |downloaded: u64, total: Option<u64>| {
                    let _ = tx2.try_send(DownloadEvent::Progress {
                        job,
                        downloaded,
                        total,
                    });
                };
                let src = GithubSource::new(token);
                src.download_to_reporting(&url, &dest, &mut report)
                    .await
                    .map(|_| dest)
                    .map_err(|e| e.to_string())
            },
            move |result| Message::UpdateInstalled(id.clone(), result),
        );
        Task::batch([download, progress_task])
    }

    /// Start the queue's head when nothing is running —
    /// called as each download ends (and on registration
    /// when the wire is already free, via InstallUpdate's
    /// own start), so a queued job begins the moment its
    /// turn comes.
    fn start_next_download(&mut self) -> Task<Message> {
        if self.downloads.has_running() {
            return Task::none();
        }
        let Some((job, name, version)) = self
            .downloads
            .next_queued()
            .map(|j| (j.id, j.mod_name.clone(), j.version.clone()))
        else {
            return Task::none();
        };
        self.status = format!("Downloading {} v{}…", name, version);
        self.start_download(job)
    }

    /// The Downloads section's source label for one install:
    /// the update database's recorded source when the mod
    /// has a row (that is where KingMods and itch offers
    /// come from), else the offer's own shape — a "modhub:"
    /// repo marker means ModHub, anything else a GitHub repo.
    fn download_source_label(&self, m: &Mod, info: &UpdateInfo) -> String {
        let stem = ModManager::mod_stem(&m.path);
        if let Some(row) = self.db_rows.get(&stem) {
            match row.source_kind.as_str() {
                "github" => return "GitHub".to_string(),
                "modhub" => return "ModHub".to_string(),
                "kingmods" => return "KingMods".to_string(),
                "itch" => return "itch.io".to_string(),
                _ => {}
            }
        }
        if info.repo.starts_with("modhub:") {
            "ModHub".to_string()
        } else {
            "GitHub".to_string()
        }
    }

    /// Save the folder inputs and rescan — the Settings
    /// page's Save button and the folder pickers share this
    /// path. The page's one explicit save: the folders AND
    /// every staged value across all its tabs commit
    /// together through the one config write, then the
    /// rescan the folder save has always started applies
    /// them. A numeric field that doesn't parse refuses
    /// the whole save — nothing is written half-way. The
    /// Developer Console is a staged value like the rest:
    /// its game.xml write happens here, at save time.
    fn apply_directories(&mut self) -> Task<Message> {
        if self.loading {
            self.status =
                "Still loading — wait for it to finish before changing folders."
                    .to_string();
            return Task::none();
        }
        let active = std::path::PathBuf::from(self.active_dir_input.trim());
        let archive = std::path::PathBuf::from(self.archive_dir_input.trim());
        let Ok(parallel) = self.settings_parallel_input.trim().parse::<usize>() else {
            self.status =
                "Parallel scans must be a whole number (1–16) — nothing was saved."
                    .to_string();
            return Task::none();
        };
        let Ok(cache_ttl) = self.settings_cache_ttl_input.trim().parse::<u64>() else {
            self.status =
                "Cache lifetime must be a whole number of hours (minimum 1) — nothing was saved."
                    .to_string();
            return Task::none();
        };
        // The Launch Configuration's slot: empty is unset;
        // anything present must be a whole slot number in
        // range, or the save refuses like any bad number.
        let slot_text = self.settings_launch_slot_input.trim();
        let slot = if slot_text.is_empty() {
            None
        } else {
            match slot_text.parse::<u8>() {
                Ok(n) if (1..=20).contains(&n) => Some(n),
                _ => {
                    self.status =
                        "Savegame slot must be a whole number from 1 to 20 (or empty) — nothing was saved."
                            .to_string();
                    return Task::none();
                }
            }
        };
        let Ok(fill_log_keep) =
            self.settings_fill_log_keep_input.trim().parse::<usize>()
        else {
            self.status =
                "Fill logs to keep must be a whole number (minimum 1) — nothing was saved."
                    .to_string();
            return Task::none();
        };
        let Ok(log_lines_cap) =
            self.settings_log_lines_cap_input.trim().parse::<usize>()
        else {
            self.status =
                "Log lines to keep must be a whole number (minimum 1) — nothing was saved."
                    .to_string();
            return Task::none();
        };
        let mut launch = self.settings_launch.clone();
        launch.slot = slot;
        let page = PageSettings {
            active_dir: active,
            archive_dir: archive,
            check_conflicts: self.settings_check_conflicts,
            parallel_scans: parallel,
            cache_ttl_hours: cache_ttl,
            catalog_url: self.settings_catalog_url_input.clone(),
            mod_sources: self.settings_sources.clone(),
            startup_update_check: self.settings_startup_check,
            self_update_enabled: self.settings_self_update,
            browse_default_view: browse_view_key(&self.settings_browse_default),
            start_maximized: self.settings_start_maximized,
            auto_file_new_mods: self.settings_auto_file,
            keep_download_history: self.settings_keep_history,
            fill_log_keep,
            log_errors_only_default: self.settings_log_errors_only_default,
            log_lines_cap,
            launch,
        };
        match self.mod_manager.save_settings_page(page) {
            Ok(()) => {
                // Show the values as saved (folders tilde-
                // expanded, numbers clamped to their ranges).
                self.active_dir_input =
                    self.mod_manager.get_active_dir().display().to_string();
                self.archive_dir_input =
                    self.mod_manager.get_archive_dir().display().to_string();
                self.settings_parallel_input =
                    self.mod_manager.parallel_scans().to_string();
                self.settings_cache_ttl_input =
                    self.mod_manager.cache_ttl_hours().to_string();
                self.settings_catalog_url_input = self.mod_manager.catalog_url();
                self.settings_launch = self.mod_manager.launch_config();
                self.settings_launch_slot_input = self
                    .settings_launch
                    .slot
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                self.settings_fill_log_keep_input =
                    self.mod_manager.fill_log_keep().to_string();
                self.settings_log_lines_cap_input =
                    self.mod_manager.log_lines_cap().to_string();
                // The staged console switch writes game.xml
                // now, with the save. A failure rides the
                // pending status (the rescan about to start
                // would otherwise overwrite the note).
                if let (Some(dir), Some(on)) =
                    (self.mod_manager.game_directory(), self.console_on)
                {
                    if let Err(e) = game_files::set_console(&dir, on) {
                        self.pending_status = Some(format!(
                            "Settings saved, but the developer console switch in game.xml was not changed: {}",
                            e
                        ));
                    }
                }
                self.begin_load()
            }
            Err(e) => {
                self.status = format!("Could not save settings: {}", e);
                Task::none()
            }
        }
    }

    /// Persist the Downloads page's finished history when
    /// the setting keeps it (Round 2). When the setting is
    /// off this writes nothing — and deletes nothing: the
    /// stored file is left exactly as it was.
    fn save_download_history(&self) {
        if !self.mod_manager.keep_download_history() {
            return;
        }
        let Some(path) = download_history_path() else {
            return;
        };
        let entries = self.downloads.history_entries();
        if let Ok(text) = serde_json::to_string_pretty(&entries) {
            let _ = std::fs::write(path, text);
        }
    }

    /// The scheduled-fill task query, as a task: it spawns
    /// schtasks, which has no place on the UI thread.
    fn query_fill_task(&self) -> Task<Message> {
        Task::perform(
            async move { query_fill_task_registered() },
            Message::FillTaskStatus,
        )
    }

    /// The Diagnostics summary (Settings, Diagnostics):
    /// the app's vital signs in plain text — and already
    /// scrubbed of the user's home path, because this is
    /// the text Copy / Export hand to the world.
    fn diagnostics_summary(&self) -> String {
        let total = self.mod_manager.get_mods().len();
        let active = self.mod_manager.get_mods_by_state(ModState::Active).len();
        let archived = self
            .mod_manager
            .get_mods_by_state(ModState::Archived)
            .len();
        let data_dir = crate::paths::data_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let catalog = match crate::sources::catalog::cache_path() {
            Some(path) => match std::fs::metadata(&path) {
                Ok(meta) => format!(
                    "{} ({})",
                    format_size(meta.len()),
                    path.display()
                ),
                Err(_) => "not downloaded yet".to_string(),
            },
            None => "unknown".to_string(),
        };
        let fill = match self.fill_task_registered {
            Some(true) => "registered (daily 06:00, 14:00, 22:00)",
            Some(false) => "not registered",
            None => "unknown",
        };
        let text = format!(
            "Playground diagnostics\n\
             Version: {}\n\
             OS: {} ({})\n\
             Data folder: {}\n\
             Library: {} mods — {} active, {} archived\n\
             ModHub catalog cache: {}\n\
             Scheduled fill: {}\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            data_dir,
            total,
            active,
            archived,
            catalog,
            fill,
        );
        match dirs::home_dir() {
            Some(home) => scrub_home_prefix(&text, &home),
            None => text,
        }
    }

    /// Re-read the recorded migration state for the
    /// Layout page: the same layout-state.json the CLI
    /// verbs write, so the page's standing picture can
    /// never disagree with the executor's record.
    fn refresh_layout_standing(&mut self) {
        match layout_exec::standing_state() {
            Ok(state) => self.layout_standing = Some(state),
            Err(e) => {
                self.layout_standing = Some(None);
                self.layout_error = Some(format!(
                    "The migration record (layout-state.json) could not be read: {:#}",
                    e
                ));
            }
        }
    }

    /// Start the migration in the background: the
    /// previewed plan is executed exactly as approved
    /// (the executor refuses if the tree drifted since
    /// the preview), with op counts streaming back over
    /// the channel — the load job's pattern, so the page
    /// ticks on a ~2,900-mod migration instead of
    /// freezing.
    fn begin_layout_apply(&mut self, preview: LayoutPlan) -> Task<Message> {
        self.layout_running = Some(LayoutRun::Apply);
        self.kick_anim();
        self.layout_done = 0;
        self.layout_total = 0;
        self.layout_outcome = None;
        self.layout_error = None;
        self.status = "Applying the layout…".to_string();
        let mods: Vec<Mod> = self.mod_manager.get_mods().into_iter().cloned().collect();
        let lib = self.mod_manager.get_archive_dir().clone();
        let game = self.mod_manager.get_active_dir().clone();
        let (tx, rx) = futures::channel::mpsc::channel::<LayoutEvent>(128);
        std::thread::spawn(move || layout_apply_job(preview, lib, game, mods, tx));
        Task::run(rx, Message::LayoutEvent)
    }

    /// Start the rollback in the background: the same
    /// executor entry the CLI verb runs, audit refusal
    /// included.
    fn begin_layout_rollback(&mut self) -> Task<Message> {
        self.layout_running = Some(LayoutRun::Rollback);
        self.kick_anim();
        self.layout_outcome = None;
        self.layout_error = None;
        self.status = "Rolling back the layout…".to_string();
        let (tx, rx) = futures::channel::mpsc::channel::<LayoutEvent>(128);
        std::thread::spawn(move || layout_rollback_job(tx));
        Task::run(rx, Message::LayoutEvent)
    }

    /// Read the game's log.txt in the background (Log Reader).
    fn begin_log_load(&mut self) -> Task<Message> {
        match self.mod_manager.profile_dir() {
            Some(profile) => {
                self.log_note = "Reading log.txt…".to_string();
                let cap = self.mod_manager.log_lines_cap();
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
                    async move { log_job(profile, stems, cap) },
                    Message::LogLoaded,
                )
            }
            None => {
                self.log_note = "No game profile folder — check that the game mods folder really is a game's mods folder."
                    .to_string();
                Task::none()
            }
        }
    }

    fn recompute_conflicts(&mut self) {
        // The check_conflicts setting gates every recompute:
        // off clears whatever was computed before, so no
        // stale conflict can outlive the switch.
        if !self.mod_manager.check_conflicts() {
            self.conflicts = Vec::new();
            self.retarget_counts();
            return;
        }
        let mods: Vec<Mod> = self.mod_manager.get_mods().into_iter().cloned().collect();
        self.conflicts = ConflictDetector::detect_conflicts(&mods);
        // Nearly every data change flows through here, so
        // this is the rolling counts' main artery too
        // (feel pass, 2026-10-23).
        self.retarget_counts();
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
                            m.categories.first().map_or(false, |x| x == c)
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

    /// Configure Save: load a slot's files into the wizard —
    /// settings, finances, the recorded mod list — and seed
    /// the loadout draft from them.
    fn wizard_load_slot(&mut self, dir_name: &str) {
        self.saves_selected = Some(dir_name.to_string());
        self.wizard_arm_apply = false;
        self.wizard_done.clear();
        self.wizard_failed.clear();
        self.wizard_autodrive = None;
        let slot_path = self
            .save_slots_cache
            .iter()
            .find(|s| s.dir_name == dir_name)
            .map(|s| s.path.clone());
        let Some(slot_path) = slot_path else {
            self.wizard_original = None;
            self.wizard_settings = None;
            self.wizard_finances = None;
            self.wizard_recorded = Vec::new();
            self.wizard_missing = Vec::new();
            self.wizard_seed_note = String::new();
            self.saves_draft.clear();
            return;
        };
        // The other slots' names, for the unique-name check.
        self.wizard_other_names = self
            .save_slots_cache
            .iter()
            .filter(|s| s.dir_name != dir_name)
            .filter_map(|s| {
                let xml =
                    std::fs::read_to_string(s.path.join("careerSavegame.xml")).ok()?;
                savegame::read_settings(&xml).savegame_name
            })
            .collect();
        let loaded = savegame::load_slot(&slot_path);
        self.wizard_cash = loaded
            .finances
            .money
            .map(|m| m.to_string())
            .unwrap_or_default();
        self.wizard_loan = loaded
            .finances
            .loan
            .map(|l| l.to_string())
            .unwrap_or_default();
        self.wizard_finances = Some(loaded.finances);
        self.wizard_autodrive_existing = loaded.autodrive_present;
        self.wizard_recorded = loaded.recorded;
        self.wizard_original = Some(loaded.settings.clone());
        self.wizard_settings = Some(loaded.settings);
        self.wizard_seed_draft();
    }

    /// Seed (or reseed) the loadout draft: the save's own
    /// recorded mod list when it has one — mods it names
    /// that the library doesn't hold stay as missing, by
    /// name — else the stored loadout for the slot, else
    /// the current Active set.
    fn wizard_seed_draft(&mut self) {
        if !self.wizard_recorded.is_empty() {
            let by_stem: HashMap<String, String> = self
                .mod_manager
                .get_mods()
                .iter()
                .map(|m| {
                    (
                        ModManager::mod_stem(&m.path).to_lowercase(),
                        m.favorite_key(),
                    )
                })
                .collect();
            let mut draft = HashSet::new();
            let mut missing = Vec::new();
            for rec in &self.wizard_recorded {
                match by_stem.get(&rec.mod_name.to_lowercase()) {
                    Some(key) => {
                        draft.insert(key.clone());
                    }
                    None => missing.push(
                        rec.title.clone().unwrap_or_else(|| rec.mod_name.clone()),
                    ),
                }
            }
            let found = draft.len();
            let total = self.wizard_recorded.len();
            self.saves_draft = draft;
            self.wizard_missing = missing;
            self.wizard_seed_note = format!(
                "Seeded from the save's own recorded mod list — {} of {} found in your library.",
                found, total
            );
            return;
        }
        let stored = self
            .saves_selected
            .as_ref()
            .map(|s| self.mod_manager.save_loadout(s))
            .unwrap_or_default();
        if !stored.is_empty() {
            self.saves_draft = stored.into_iter().collect();
            self.wizard_seed_note =
                "Seeded from the loadout saved for this slot.".to_string();
        } else {
            self.saves_draft = self
                .mod_manager
                .get_mods()
                .into_iter()
                .filter(|m| m.state == ModState::Active)
                .map(|m| m.favorite_key())
                .collect();
            self.wizard_seed_note =
                "Seeded from your current Active set.".to_string();
        }
        self.wizard_missing = Vec::new();
    }

    /// The loadout step's three groups: the map's required
    /// mods first, then the player's favourites, then the
    /// rest of the library. One definition, shared by the
    /// page and the group select/deselect buttons.
    fn wizard_groups(&self) -> Vec<(String, String, Vec<WizardModEntry>)> {
        let required_stems: HashSet<String> = self
            .wizard_recorded
            .iter()
            .filter(|r| r.required)
            .map(|r| r.mod_name.to_lowercase())
            .collect();
        let mut required = Vec::new();
        let mut favourites = Vec::new();
        let mut library = Vec::new();
        for m in self.mod_manager.get_mods() {
            let entry = WizardModEntry {
                key: m.favorite_key(),
                id: m.id.clone(),
                name: m.name.clone(),
                author: m.author.clone(),
                version: m.version.clone(),
                category: m.categories.first().cloned(),
                required: required_stems
                    .contains(&ModManager::mod_stem(&m.path).to_lowercase()),
            };
            if entry.required {
                required.push(entry);
            } else if self.mod_manager.is_favorite(&entry.key) {
                favourites.push(entry);
            } else {
                library.push(entry);
            }
        }
        let by_name = |a: &WizardModEntry, b: &WizardModEntry| {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        };
        required.sort_by(by_name);
        favourites.sort_by(by_name);
        library.sort_by(by_name);
        vec![
            (
                "required".to_string(),
                "Required by Map".to_string(),
                required,
            ),
            (
                "favourites".to_string(),
                "Favourites".to_string(),
                favourites,
            ),
            ("library".to_string(), "Library".to_string(), library),
        ]
    }

    fn wizard_group_entries(&self, group: &str) -> Vec<WizardModEntry> {
        self.wizard_groups()
            .into_iter()
            .find(|(id, _, _)| id == group)
            .map(|(_, _, entries)| entries)
            .unwrap_or_default()
    }

    /// Settings-page edits, by key. Keys are the wizard's
    /// own short names; the mapping to XML elements lives
    /// in mod_manager::savegame.
    fn wizard_set_bool(&mut self, key: &str, value: bool) {
        if let Some(s) = &mut self.wizard_settings {
            match key {
                "traffic" => s.traffic_enabled = Some(value),
                "snow" => s.snow_enabled = Some(value),
                "fruit_destruction" => s.fruit_destruction = Some(value),
                "plowing" => s.plowing_required_enabled = Some(value),
                "stones" => s.stones_enabled = Some(value),
                "lime" => s.lime_required = Some(value),
                "weeds" => s.weeds_enabled = Some(value),
                "motor_start" => s.automatic_motor_start_enabled = Some(value),
                "stop_and_go" => s.stop_and_go_braking = Some(value),
                "trailer_fill" => s.trailer_fill_limit = Some(value),
                "helper_fuel" => s.helper_buy_fuel = Some(value),
                "helper_seeds" => s.helper_buy_seeds = Some(value),
                "helper_fertilizer" => s.helper_buy_fertilizer = Some(value),
                "fixed_visual" => s.fixed_visual_month = Some(value),
                _ => {}
            }
        }
        self.wizard_arm_apply = false;
    }

    fn wizard_set_value(&mut self, key: &str, value: &str) {
        if let Some(s) = &mut self.wizard_settings {
            match key {
                "difficulty" => {
                    s.economic_difficulty = Some(value.to_string())
                }
                "growth" => s.growth_mode = value.parse().ok(),
                "fuel" => s.fuel_usage = value.parse().ok(),
                "timescale" => s.time_scale = value.parse().ok(),
                "autosave" => s.auto_save_interval = value.parse().ok(),
                "days" => s.planned_days_per_period = value.parse().ok(),
                _ => {}
            }
        }
        self.wizard_arm_apply = false;
    }

    /// What stands between the draft and an apply. Review
    /// shows these, and Apply refuses while any remain —
    /// the design's inline validation.
    fn wizard_validation(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.saves_selected.is_none() {
            problems.push("No save is picked.".to_string());
            return problems;
        }
        if let Some(s) = &self.wizard_settings {
            let name = s.savegame_name.as_deref().unwrap_or("").trim();
            if name.is_empty() {
                problems.push(
                    "Please give your save a unique name before applying."
                        .to_string(),
                );
            } else if self.wizard_other_names.iter().any(|n| n == name) {
                problems.push(format!(
                    "Another save already uses the name \"{}\". Please give your save a unique name before applying.",
                    name
                ));
            }
        }
        for (label, text) in [
            ("cash", &self.wizard_cash),
            ("loan", &self.wizard_loan),
        ] {
            if !text.trim().is_empty() {
                match parse_amount(text) {
                    None => problems.push(format!(
                        "The {} amount isn't a whole number.",
                        label
                    )),
                    Some(v) if v < 0 => problems.push(format!(
                        "The {} amount can't be negative.",
                        label
                    )),
                    _ => {}
                }
            }
        }
        problems
    }

    /// Configure Save's apply: backups first, then the
    /// settings, the money, the route file, the stored
    /// loadout, the mods folder's links, and the
    /// gameSettings pointer — each landing on the results
    /// page in its own words, success and failure apart.
    fn wizard_apply(&mut self) {
        let mut done: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let Some(dir_name) = self.saves_selected.clone() else {
            return;
        };
        let slot = self
            .save_slots_cache
            .iter()
            .find(|s| s.dir_name == dir_name)
            .cloned();
        let Some(slot) = slot else {
            self.wizard_failed =
                vec!["The picked save is no longer in the slot list.".to_string()];
            self.wizard_done = Vec::new();
            self.wizard_step = 4;
            return;
        };
        let slot_dir = slot.path.clone();
        let career_path = slot_dir.join("careerSavegame.xml");
        let farms_path = slot_dir.join("farms.xml");

        // ---- Backups first ------------------------------
        for path in [&career_path, &farms_path] {
            if path.exists() {
                let label = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match savegame::backup_file(path) {
                    Ok(dest) => done.push(format!(
                        "Backed up {} to {}.",
                        label,
                        dest.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    )),
                    Err(e) => {
                        failed.push(format!("Backup of {} failed: {}", label, e))
                    }
                }
            }
        }

        // ---- Settings -> careerSavegame.xml -------------
        if let (Some(original), Some(edited)) =
            (&self.wizard_original, &self.wizard_settings)
        {
            let original = original.clone();
            let edited = edited.clone();
            let changes = savegame::describe_changes(&original, &edited);
            if changes.is_empty() {
                done.push("Settings: nothing changed.".to_string());
            } else {
                match std::fs::read_to_string(&career_path) {
                    Ok(xml) => {
                        let (out, applied, skipped) =
                            savegame::apply_settings_to_xml(&xml, &original, &edited);
                        match std::fs::write(&career_path, out) {
                            Ok(()) => done.push(format!(
                                "Settings written to careerSavegame.xml ({}).",
                                applied.join(", ")
                            )),
                            Err(e) => failed.push(format!(
                                "Writing careerSavegame.xml failed: {}",
                                e
                            )),
                        }
                        for label in skipped {
                            failed.push(format!(
                                "{}: the element isn't in this save to rewrite.",
                                label
                            ));
                        }
                    }
                    Err(e) => failed.push(format!(
                        "Reading careerSavegame.xml failed: {}",
                        e
                    )),
                }
            }
        }

        // ---- Cash & loan -> farms.xml -------------------
        if let Some(fin) = &self.wizard_finances {
            let fin = fin.clone();
            let new_money = parse_amount(&self.wizard_cash);
            let new_loan = parse_amount(&self.wizard_loan);
            let money_changed = new_money.is_some() && new_money != fin.money;
            let loan_changed = new_loan.is_some() && new_loan != fin.loan;
            if money_changed || loan_changed {
                let money = if money_changed {
                    new_money.unwrap()
                } else {
                    fin.money.unwrap_or(0)
                };
                let loan = if loan_changed {
                    new_loan.unwrap()
                } else {
                    fin.loan.unwrap_or(0)
                };
                match std::fs::read_to_string(&farms_path) {
                    Ok(xml) => {
                        match savegame::apply_finances_to_xml(&xml, money, loan) {
                            Some(out) => match std::fs::write(&farms_path, out) {
                                Ok(()) => done.push(format!(
                                    "Cash and loan written to farms.xml (cash {}, loan {}).",
                                    money, loan
                                )),
                                Err(e) => failed.push(format!(
                                    "Writing farms.xml failed: {}",
                                    e
                                )),
                            },
                            None => failed.push(
                                "farms.xml has no farm to hold the cash and loan."
                                    .to_string(),
                            ),
                        }
                    }
                    Err(e) => {
                        failed.push(format!("Reading farms.xml failed: {}", e))
                    }
                }
            } else {
                done.push("Cash and loan: unchanged.".to_string());
            }
        }

        // ---- AutoDrive route ----------------------------
        if let Some(picked) = &self.wizard_autodrive {
            let picked = picked.clone();
            let dest = slot_dir.join("AutoDrive_config.xml");
            match std::fs::copy(&picked, &dest) {
                Ok(_) => done.push(
                    "AutoDrive route installed: AutoDrive_config.xml replaced in the save folder."
                        .to_string(),
                ),
                Err(e) => failed.push(format!(
                    "Installing the AutoDrive route failed: {}",
                    e
                )),
            }
        }

        // ---- Store the loadout --------------------------
        let keys: Vec<String> = self.saves_draft.iter().cloned().collect();
        match self.mod_manager.set_save_loadout(&dir_name, keys) {
            Ok(()) => done.push(format!(
                "Loadout saved for {} ({} mods).",
                dir_name,
                self.saves_draft.len()
            )),
            Err(e) => failed.push(format!("Saving the loadout failed: {}", e)),
        }

        // ---- Reconcile the mods folder (links only) -----
        let game_dir = self.mod_manager.get_active_dir().clone();
        let all_mods = self.mod_manager.get_mods();
        let selected: Vec<Mod> = all_mods
            .iter()
            .filter(|m| self.saves_draft.contains(&m.favorite_key()))
            .map(|m| (*m).clone())
            .collect();
        let library: Vec<Mod> = all_mods.iter().map(|m| (*m).clone()).collect();
        let report = savegame::reconcile_mods_folder(&game_dir, &selected, &library);
        done.push(format!(
            "Mods folder: {} linked in, {} already there, {} unselected links removed.",
            report.linked, report.already_present, report.removed
        ));
        failed.extend(report.notes);

        // ---- gameSettings pointer -----------------------
        if let Some(profile) = self.mod_manager.profile_dir() {
            let want = game_dir.to_string_lossy().replace('\\', "/");
            let current = game_files::read_mods_override(&profile);
            let already = matches!(&current, Some((true, dir))
                if dir.trim_end_matches('/') == want.trim_end_matches('/'));
            if already {
                done.push(
                    "gameSettings.xml already points at the mods folder."
                        .to_string(),
                );
            } else {
                match game_files::set_mods_override(&profile, &game_dir) {
                    Ok(()) => done.push(
                        "gameSettings.xml now points at the mods folder."
                            .to_string(),
                    ),
                    Err(e) => failed.push(format!(
                        "Pointing gameSettings.xml at the mods folder failed: {}",
                        e
                    )),
                }
            }
        }

        self.wizard_done = done;
        self.wizard_failed = failed;
        self.wizard_step = 4;
        self.refresh_save_slots();
        self.status = "Configure Save applied — see the results.".to_string();
    }

    /// The delete itself, fired by the hold clock once a
    /// Remove hold completes (feel pass, 2026-10-23).
    fn execute_remove(&mut self, id: &str) {
        let name = self.mod_manager.get_mod(id).map(|m| m.name.clone());
        match self.mod_manager.remove_mod(id) {
            Ok(()) => {
                if self.selected_mod.as_deref() == Some(id) {
                    self.selected_mod = None;
                }
                self.icons.remove(id);
                self.recompute_conflicts();
                self.status = format!(
                    "Deleted {}.",
                    name.unwrap_or_else(|| "mod".to_string())
                );
            }
            Err(e) => self.status = format!("Delete failed: {}", e),
        }
    }

    /// Whether this mod's Remove control is being held,
    /// and its hold progress 0.0..=1.0 (feel pass,
    /// 2026-10-23).
    fn remove_holding(&self, id: &str) -> bool {
        matches!(&self.remove_hold, Some((held, _)) if held == id)
    }

    fn remove_hold_progress(&self, id: &str) -> f32 {
        match &self.remove_hold {
            Some((held, started)) if held == id => (started.elapsed().as_secs_f32()
                / REMOVE_HOLD_TIME.as_secs_f32())
            .clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Hold-to-delete: any action outside the hold's
        // own messages cancels a live hold unexecuted —
        // the same disarm discipline the old two-click
        // confirm lived under.
        if !matches!(
            message,
            Message::RemoveHoldStarted(_)
                | Message::RemoveHoldReleased
                | Message::RemoveHoldTick
        ) {
            self.remove_hold = None;
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
                    // Both load phases are done — but on a
                    // boot the splash does NOT hand over here
                    // any more: the icons are still loading,
                    // and the handover waits for them in
                    // IconsLoaded, so the main window never
                    // sits at splash size wearing the main UI
                    // while the icon pass runs (John's ruling,
                    // 2026-10-22, after a boot showed exactly
                    // that trap). `booting` stays up here;
                    // only the icon pass starts. (A rescan's
                    // load never held the splash anyway —
                    // nothing changes for it.)
                    let was_booting = self.booting;
                    if was_booting {
                        // A boot load just completed whole:
                        // if this launch is a self-updated
                        // binary's first run, it has proven
                        // itself — its predecessor's .bak
                        // backups stop earning their keep
                        // here (see app_update's docs).
                        app_update::cleanup_stale_backups();
                    }
                    self.status = format!("Loaded {} mods — loading icons…", count);
                    return self.begin_icon_load();
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
                        return Self::restore_window();
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
                if self.booting {
                    // The boot hold releases here: the scan
                    // and the icons are both in, so the
                    // splash hands over to the finished main
                    // window in one shape change (see
                    // restore_window). Only a boot can arrive
                    // here with `booting` still up — a
                    // rescan's icon pass finds it long down —
                    // so the stateful decorations toggle
                    // still fires exactly once per launch.
                    self.booting = false;
                    let restore = Self::restore_window();
                    // The start-up update check (Settings >
                    // General > Updates, Round 2), gated by
                    // its switch: the detection pass runs
                    // once the app is up, exactly as it does
                    // behind a rescan. A rescan asks for the
                    // pass itself (detect_after_scan) and gets
                    // it in either switch state — the switch
                    // gates the launch-time check only.
                    let detect = self.detect_after_scan
                        || self.mod_manager.startup_update_check();
                    self.detect_after_scan = false;
                    if detect {
                        return Task::batch(vec![self.start_detect(), restore]);
                    }
                    return restore;
                }
                if self.detect_after_scan {
                    self.detect_after_scan = false;
                    return self.start_detect();
                }
            }
            Message::SelectSaveSlot(dir_name) => {
                self.wizard_load_slot(&dir_name);
            }
            Message::DraftToggle(key) => {
                if !self.saves_draft.remove(&key) {
                    self.saves_draft.insert(key);
                }
                self.wizard_arm_apply = false;
            }
            Message::DraftUseActive => {
                self.saves_draft = self
                    .mod_manager
                    .get_mods()
                    .into_iter()
                    .filter(|m| m.state == ModState::Active)
                    .map(|m| m.favorite_key())
                    .collect();
                self.wizard_missing = Vec::new();
                self.wizard_seed_note =
                    "Seeded from your current Active set.".to_string();
                self.wizard_arm_apply = false;
            }
            Message::DraftEmpty => {
                self.saves_draft.clear();
                self.wizard_seed_note = "Starting from an empty loadout.".to_string();
                self.wizard_arm_apply = false;
            }
            Message::SavesFilterChanged(filter) => self.saves_filter = filter,
            Message::WizardStep(step) => {
                if step >= 1 && step != 4 && self.saves_selected.is_none() {
                    self.status =
                        "Pick a save first — the rest of the wizard builds on it."
                            .to_string();
                } else {
                    self.wizard_step = step;
                    self.wizard_arm_apply = false;
                }
            }
            Message::WizardSetBool(key, value) => {
                self.wizard_set_bool(&key, value);
            }
            Message::WizardSetValue(key, value) => {
                self.wizard_set_value(&key, &value);
            }
            Message::WizardNameChanged(value) => {
                if let Some(s) = &mut self.wizard_settings {
                    s.savegame_name = Some(value);
                }
                self.wizard_arm_apply = false;
            }
            Message::WizardCashChanged(value) => {
                self.wizard_cash = value;
                self.wizard_arm_apply = false;
            }
            Message::WizardLoanChanged(value) => {
                self.wizard_loan = value;
                self.wizard_arm_apply = false;
            }
            Message::WizardCashChip(delta) => {
                let cur = parse_amount(&self.wizard_cash).unwrap_or(0);
                self.wizard_cash = (cur + delta).max(0).to_string();
                self.wizard_arm_apply = false;
            }
            Message::WizardLoanChip(delta) => {
                let cur = parse_amount(&self.wizard_loan).unwrap_or(0);
                self.wizard_loan = (cur + delta).max(0).to_string();
                self.wizard_arm_apply = false;
            }
            Message::WizardUseSaveList => {
                self.wizard_seed_draft();
                self.wizard_arm_apply = false;
            }
            Message::WizardGroupToggle(group, select) => {
                for entry in self.wizard_group_entries(&group) {
                    if select {
                        self.saves_draft.insert(entry.key);
                    } else {
                        self.saves_draft.remove(&entry.key);
                    }
                }
                self.wizard_arm_apply = false;
            }
            Message::WizardCollapseGroup(group) => {
                if !self.wizard_collapsed.remove(&group) {
                    self.wizard_collapsed.insert(group);
                }
            }
            Message::WizardBrowseAutoDrive => {
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title(
                                "Choose the AutoDrive_config.xml to install",
                            )
                            .add_filter("AutoDrive route", &["xml"])
                            .pick_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::WizardAutoDrivePicked,
                );
            }
            Message::WizardAutoDrivePicked(picked) => {
                if picked.is_some() {
                    self.wizard_autodrive = picked;
                    self.wizard_arm_apply = false;
                }
            }
            Message::WizardClearAutoDrive => {
                self.wizard_autodrive = None;
                self.wizard_arm_apply = false;
            }
            Message::WizardApply => {
                let problems = self.wizard_validation();
                if !problems.is_empty() {
                    self.wizard_arm_apply = false;
                    self.status = format!(
                        "Fix this before applying: {}",
                        problems.join(" ")
                    );
                } else if !self.wizard_arm_apply {
                    self.wizard_arm_apply = true;
                    self.status = "Click Generate Folder & Apply again to confirm — make sure the game is not running.".to_string();
                } else {
                    self.wizard_arm_apply = false;
                    self.wizard_apply();
                    return self.begin_load();
                }
            }
            Message::WizardReset => {
                self.wizard_step = 0;
                self.saves_selected = None;
                self.saves_draft.clear();
                self.wizard_original = None;
                self.wizard_settings = None;
                self.wizard_finances = None;
                self.wizard_recorded = Vec::new();
                self.wizard_missing = Vec::new();
                self.wizard_seed_note = String::new();
                self.wizard_autodrive = None;
                self.wizard_done.clear();
                self.wizard_failed.clear();
                self.wizard_arm_apply = false;
                self.refresh_save_slots();
            }
            Message::ReclaimMeasure => {
                // Read-only, like Build plan: census the
                // two pools and show the counts. A fresh
                // measure always disarms a pending run.
                self.reclaim_arm = false;
                self.reclaim_outcome = None;
                self.reclaim = Some(self.build_reclaim_census());
            }
            Message::ReclaimRun => {
                if self.layout_running.is_some() {
                    return Task::none();
                }
                if !self.reclaim_arm {
                    // First press arms only — the button's
                    // label restates the census counts, so
                    // the approval is for THIS census's work.
                    self.reclaim_arm = true;
                    return Task::none();
                }
                self.reclaim_arm = false;
                self.execute_reclaim();
            }
            Message::LayoutBuildPlan => {
                if self.layout_running.is_some() {
                    self.status =
                        "A layout job is running — let it finish before rebuilding the plan."
                            .to_string();
                } else if self.loading {
                    self.status =
                        "Still loading — the plan needs the finished scan.".to_string();
                } else {
                    // The same build_plan the CLI verb
                    // runs, against the current folders and
                    // records — read-only. The plan JSON is
                    // written too, Phase 2's input contract,
                    // exactly as `cli layout-plan` writes it.
                    let plan = self.mod_manager.layout_plan();
                    match layout::write_plan_json(&plan) {
                        Ok(_) => {
                            self.layout_outcome = Some(format!(
                                "Plan built: {} mods, {} moves, {} links to create.",
                                plan.summary.mods,
                                plan.summary.moves,
                                plan.summary.hardlinks + plan.summary.junctions
                            ));
                            self.layout_error = None;
                        }
                        Err(e) => {
                            self.layout_error = Some(format!(
                                "The plan was built but its JSON could not be saved: {}",
                                e
                            ));
                        }
                    }
                    self.layout_preview = Some(plan);
                    self.layout_arm_apply = false;
                }
            }
            Message::LayoutApply => {
                if self.layout_running.is_some() {
                    return Task::none();
                }
                if !self.layout_arm_apply {
                    // First press arms only — the button's
                    // label restates the counts, so the
                    // approval is for THIS plan's work.
                    self.layout_arm_apply = true;
                    return Task::none();
                }
                self.layout_arm_apply = false;
                let Some(preview) = self.layout_preview.clone() else {
                    self.layout_error =
                        Some("Build a plan first — there is nothing approved to apply.".to_string());
                    return Task::none();
                };
                return self.begin_layout_apply(preview);
            }
            Message::LayoutRollback => {
                if self.layout_running.is_some() {
                    return Task::none();
                }
                if !self.layout_arm_rollback {
                    self.layout_arm_rollback = true;
                    return Task::none();
                }
                self.layout_arm_rollback = false;
                return self.begin_layout_rollback();
            }
            Message::LayoutEvent(event) => match event {
                LayoutEvent::Progress { done, total } => {
                    self.layout_done = done;
                    self.layout_total = total;
                }
                LayoutEvent::ApplyFinished(result) => {
                    self.layout_running = None;
                    self.layout_preview = None;
                    self.refresh_layout_standing();
                    match result {
                        Ok(LayoutRunOutcome::AlreadyLaidOut) => {
                            self.layout_outcome = Some(
                                "Already in layout form — the library is sorted and the game folder is projected. Nothing was done."
                                    .to_string(),
                            );
                            self.layout_error = None;
                            self.status = "Layout: already in layout form.".to_string();
                        }
                        Ok(LayoutRunOutcome::Migrated {
                            moves,
                            links,
                            backup,
                            log,
                        }) => {
                            self.layout_outcome = Some(format!(
                                "Migration complete: {} library move(s), {} projection(s) created.\nThe pre-layout game folder is kept whole at {}\nRollback stays open until you delete that backup yourself.\nRun log: {}",
                                moves, links, backup, log
                            ));
                            self.layout_error = None;
                            self.status = "Layout applied — rescanning the library.".to_string();
                            // The tree moved under the
                            // records: rescan so Browse and
                            // the tiles tell the new truth.
                            return self.begin_load();
                        }
                        Ok(LayoutRunOutcome::FailedReversed(why)) => {
                            self.layout_error = Some(format!(
                                "The migration FAILED and was reversed — nothing was changed. {}\nSee the run log for the step-by-step record.",
                                why
                            ));
                            self.layout_outcome = None;
                            self.status =
                                "Layout migration failed and was reversed.".to_string();
                        }
                        Err(e) => {
                            self.layout_error = Some(e);
                            self.layout_outcome = None;
                            self.status = "Layout apply did not run.".to_string();
                        }
                    }
                }
                LayoutEvent::RollbackFinished(result) => {
                    self.layout_running = None;
                    self.layout_preview = None;
                    self.refresh_layout_standing();
                    match result {
                        Ok(summary) => {
                            self.layout_outcome = Some(summary);
                            self.layout_error = None;
                            self.status =
                                "Layout rollback finished — rescanning the library.".to_string();
                            return self.begin_load();
                        }
                        Err(e) => {
                            self.layout_error = Some(e);
                            self.layout_outcome = None;
                            self.status = "Layout rollback did not complete.".to_string();
                        }
                    }
                }
            },
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
                    let (_base, token) = self.mod_manager.github_endpoint();
                    let stem = ModManager::mod_stem(&m.path);
                    let dest = m
                        .path
                        .parent()
                        .map(|p| p.join(format!(".playground-dl-{}.zip", stem)))
                        .unwrap_or_else(|| PathBuf::from(".playground-dl.zip"));
                    // Every install becomes a tracked job
                    // (Downloads section, 2026-10-22): the
                    // registry holds the job's display state,
                    // the pending map its start payload. One
                    // download runs at a time — a click while
                    // another is on the wire QUEUES and starts
                    // when it ends. (Before the registry, the
                    // click simply launched a second download
                    // in parallel; the same mod was safe only
                    // because its own state had flipped to
                    // Installing.)
                    let source = self.download_source_label(&m, &info);
                    let version = info.version.clone();
                    let job = self.downloads.register(
                        id.clone(),
                        m.name.clone(),
                        m.file_name(),
                        source,
                        info.version.clone(),
                    );
                    self.pending_downloads.insert(
                        job,
                        PendingDownload {
                            mod_id: id.clone(),
                            info,
                            dest,
                            token,
                        },
                    );
                    if self.downloads.has_running() {
                        self.status = format!(
                            "Queued {} v{} — a download is already running; this one starts when it finishes.",
                            m.name, version
                        );
                        return Task::none();
                    }
                    self.status = format!("Downloading {} v{}…", m.name, version);
                    return self.start_download(job);
                }
            }
            Message::UpdateInstalled(id, result) => {
                self.db_applied.remove(&id);
                // The Downloads job that ran this install.
                // The download half is over either way: a
                // success moves the job to Installing while
                // the file swaps below, and both halves land
                // it Done or Failed (reason kept) alongside
                // the panel's own state.
                let job = self.downloads.live_job_for_mod(&id).map(|j| j.id);
                if result.is_ok() {
                    if let Some(job) = job {
                        self.downloads.begin_install(job);
                    }
                }
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
                                let mut status = format!(
                                    "Updated {} ({}).",
                                    name, change
                                );
                                // Auto-Fill (Settings >
                                // Library): the completed mod
                                // is filed into its layout
                                // home when the switch and the
                                // layout allow. Its outcome
                                // rides the status — it can
                                // never fail the install that
                                // already succeeded.
                                match self.mod_manager.auto_file_mod(&id) {
                                    Ok(Some(note)) => {
                                        status.push(' ');
                                        status.push_str(&note);
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        status.push_str(&format!(
                                            " Auto-Fill could not file it: {}",
                                            e
                                        ));
                                    }
                                }
                                self.status = status;
                                if let Some(job) = job {
                                    self.downloads.finish(job, Ok(()));
                                }
                            }
                            Err(e) => {
                                self.status = format!("Installing the update for {} failed: {}", name, e);
                                self.update_status
                                    .insert(id, UpdateState::Failed(e.to_string()));
                                if let Some(job) = job {
                                    self.downloads.finish(job, Err(e.to_string()));
                                }
                            }
                        }
                        let _ = std::fs::remove_file(&downloaded);
                    }
                    Err(e) => {
                        self.status = format!("Download for {} failed: {}", name, e);
                        if let Some(job) = job {
                            self.downloads.finish(job, Err(e.clone()));
                        }
                        self.update_status.insert(id, UpdateState::Failed(e));
                    }
                }
                // A job may have finished above: the
                // Downloads history file follows the
                // registry (a no-op when the setting is off).
                self.save_download_history();
                // The wire is free: a queued download starts
                // now, alongside the icon refresh as before.
                return Task::batch([
                    self.begin_icon_load(),
                    self.start_next_download(),
                ]);
            }
            Message::KeybindingsDeviceSelected(device) => {
                self.kb_device = Some(device);
                self.kb_capture = None;
                self.kb_reset_armed = false;
            }
            Message::KeybindingsSearchChanged(query) => {
                self.kb_search = query;
            }
            Message::KeybindingsUnassignedOnlyToggled(on) => {
                self.kb_unassigned_only = on;
            }
            Message::KeybindingsConflictsOnlyToggled(on) => {
                self.kb_conflicts_only = on;
            }
            Message::KeybindingsShowArchivedToggled(on) => {
                self.kb_show_archived = on;
                // A capture armed on an action the flip
                // is about to hide must not land later.
                self.kb_capture = None;
            }
            Message::KeybindingsExpandAll(expand) => {
                if expand {
                    self.kb_collapsed.clear();
                } else {
                    self.kb_collapsed = self
                        .kb_grouped_actions()
                        .into_iter()
                        .map(|(group, _)| group)
                        .collect();
                }
            }
            Message::KeybindingsToggleGroup(group) => {
                if !self.kb_collapsed.remove(&group) {
                    self.kb_collapsed.insert(group);
                }
            }
            Message::KeybindingsCaptureStart(action, pos) => {
                self.kb_capture = Some(KbCapture::Replace { action, pos });
                self.status =
                    "Press a key to bind it — Esc cancels.".to_string();
            }
            Message::KeybindingsAddBinding(action) => {
                self.kb_capture = Some(KbCapture::Add { action });
                self.status =
                    "Press a key to bind it — Esc cancels.".to_string();
            }
            Message::KeybindingsCaptureKey(key, location, modifiers) => {
                let Some(capture) = self.kb_capture.clone() else {
                    return Task::none();
                };
                if matches!(
                    key,
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
                ) {
                    self.kb_capture = None;
                    self.status = "Key capture cancelled.".to_string();
                    return Task::none();
                }
                match keybindings::fs_input_for_key(&key, location, modifiers) {
                    Some(input) => {
                        let applied = match (&capture, &mut self.kb_doc) {
                            (
                                KbCapture::Replace { action, pos },
                                Some(doc),
                            ) => doc.set_input(action, *pos, &input),
                            (KbCapture::Add { action }, Some(doc)) => doc
                                .add_binding(
                                    action,
                                    keybindings::KEYBOARD_DEVICE,
                                    &input,
                                ),
                            _ => false,
                        };
                        self.kb_capture = None;
                        if applied {
                            self.status = format!(
                                "Bound {} — staged, not saved yet.",
                                keybindings::humanize_input(&input)
                            );
                        } else {
                            self.status =
                                "That binding is no longer there — the edit was dropped."
                                    .to_string();
                        }
                    }
                    None => {
                        // The capture stays armed: the user
                        // can simply press another key.
                        self.status = "That key can't be mapped yet.".to_string();
                    }
                }
            }
            Message::KeybindingsClearBinding(action, pos) => {
                self.kb_capture = None;
                if let Some(doc) = &mut self.kb_doc {
                    if doc.remove_binding(&action, pos) {
                        self.status =
                            "Binding cleared — staged, not saved yet.".to_string();
                    }
                }
            }
            Message::KeybindingsAutoResolve(group) => {
                self.kb_reset_armed = false;
                let Some(device) = self.kb_device.clone() else {
                    return Task::none();
                };
                let names: HashSet<String> = self
                    .kb_grouped_actions()
                    .into_iter()
                    .find(|(name, _)| *name == group)
                    .map(|(_, actions)| {
                        actions.iter().map(|a| a.name.clone()).collect()
                    })
                    .unwrap_or_default();
                let owners: HashMap<String, String> = names
                    .iter()
                    .filter_map(|a| {
                        self.mod_manager
                            .action_owner(a)
                            .map(|o| (a.clone(), o.title.clone()))
                    })
                    .collect();
                if let Some(doc) = &mut self.kb_doc {
                    let cleared = doc.auto_resolve(&device, &names, &owners);
                    self.status = if cleared == 0 {
                        format!(
                            "Auto-Resolve: no hard conflicts to clear in {}.",
                            group
                        )
                    } else {
                        format!(
                            "Auto-Resolve cleared {} binding{} in {} — staged, not saved yet.",
                            cleared,
                            if cleared == 1 { "" } else { "s" },
                            group
                        )
                    };
                }
            }
            Message::KeybindingsPurgePressed => {
                self.kb_reset_armed = false;
                let Some(actions) =
                    self.kb_doc.as_ref().map(|d| d.actions())
                else {
                    return Task::none();
                };
                let owners: HashMap<String, keybindings::ActionOwner> =
                    actions
                        .iter()
                        .filter_map(|a| {
                            self.mod_manager
                                .action_owner(&a.name)
                                .map(|o| (a.name.clone(), o.clone()))
                        })
                        .collect();
                if self.kb_purge.take().is_some() {
                    // Second press: stage the removals into
                    // the document. Nothing is written to
                    // disk — the Save bindings flow (with
                    // its backup first) is still the only
                    // way anything lands, and Discard
                    // abandons the purge like any edit.
                    self.kb_capture = None;
                    if let Some(doc) = &mut self.kb_doc {
                        let staged = doc.stage_purge(&owners);
                        self.status = format!(
                            "Purge staged: removed {} action{} from {} archived mod{} — not saved yet.",
                            staged.actions,
                            if staged.actions == 1 { "" } else { "s" },
                            staged.mods,
                            if staged.mods == 1 { "" } else { "s" },
                        );
                    }
                } else {
                    let summary =
                        keybindings::purge_summary(&actions, &owners);
                    if summary.actions == 0 {
                        self.status =
                            "Purge: no bindings belong to archived mods."
                                .to_string();
                    } else {
                        self.kb_purge =
                            Some((summary.actions, summary.mods));
                    }
                }
            }
            Message::KeybindingsSave => {
                self.kb_reset_armed = false;
                let (Some(profile), Some(doc)) =
                    (self.kb_profile.clone(), self.kb_doc.clone())
                else {
                    return Task::none();
                };
                // The backup comes FIRST and a failed
                // backup aborts the save: the live file is
                // never written unbacked-up.
                match keybindings::backup_current(&profile) {
                    Err(e) => {
                        self.status = format!(
                            "Save stopped — the backup could not be made: {}",
                            e
                        );
                    }
                    Ok(_) => {
                        let path = keybindings::bindings_path(&profile);
                        match std::fs::write(&path, doc.text()) {
                            Ok(()) => {
                                self.kb_original = Some(doc.text().to_string());
                                self.kb_purge = None;
                                self.kb_backups =
                                    keybindings::list_backups(&profile);
                                self.status = "Bindings saved — the previous file is in the backups folder.".to_string();
                            }
                            Err(e) => {
                                self.status = format!(
                                    "Could not write {}: {}",
                                    path.display(),
                                    e
                                );
                            }
                        }
                    }
                }
            }
            Message::KeybindingsDiscard => {
                self.kb_capture = None;
                self.kb_reset_armed = false;
                self.kb_purge = None;
                if let Some(original) = self.kb_original.clone() {
                    self.kb_doc = Some(BindingsFile::parse(original));
                    self.status = "Staged binding changes discarded.".to_string();
                }
            }
            Message::KeybindingsRestoreBackup(path) => {
                self.kb_reset_armed = false;
                let Some(profile) = self.kb_profile.clone() else {
                    return Task::none();
                };
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                match keybindings::restore_backup(&profile, &path) {
                    Ok(()) => {
                        self.kb_reload();
                        self.status = format!(
                            "Restored {} — the file it replaced was backed up first.",
                            name
                        );
                    }
                    Err(e) => {
                        self.status =
                            format!("Could not restore {}: {}", name, e);
                    }
                }
            }
            Message::KeybindingsResetDefaults => {
                let Some(profile) = self.kb_profile.clone() else {
                    return Task::none();
                };
                if !self.kb_reset_armed {
                    self.kb_reset_armed = true;
                    self.status = "Reset moves inputBinding.xml into the backups folder — click Reset again to confirm.".to_string();
                    return Task::none();
                }
                self.kb_reset_armed = false;
                match keybindings::reset_to_defaults(&profile) {
                    Ok(dest) => {
                        self.kb_doc = None;
                        self.kb_original = None;
                        self.kb_capture = None;
                        self.kb_purge = None;
                        self.kb_backups = keybindings::list_backups(&profile);
                        self.status = format!(
                            "inputBinding.xml moved to {} — the game regenerates its factory defaults on its next launch.",
                            dest.display()
                        );
                    }
                    Err(e) => {
                        self.status =
                            format!("Could not reset the bindings: {}", e);
                    }
                }
            }
            Message::GameScreensPresetSelected(preset) => {
                self.gs_choice = GsChoice::Preset(preset);
                self.gs_restore_armed = false;
                self.gs_refresh_pending();
            }
            Message::GameScreensStandardSelected => {
                self.gs_choice = GsChoice::Standard;
                self.gs_restore_armed = false;
                self.gs_refresh_pending();
            }
            Message::GameScreensPickCustom => {
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title("Choose an image for the menu screen")
                            .add_filter(
                                "Images",
                                &["png", "jpg", "jpeg", "webp", "bmp", "dds"],
                            )
                            .pick_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::GameScreensCustomPicked,
                );
            }
            Message::GameScreensCustomPicked(picked) => {
                if let Some(path) = picked {
                    match std::fs::read(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|bytes| {
                            gamescreens::decode_source(&bytes)
                                .map_err(|e| format!("{:#}", e))
                        }) {
                        Ok(art) => {
                            self.gs_custom_name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string());
                            self.gs_custom_art = Some(art);
                            self.gs_choice = GsChoice::Custom;
                            self.gs_restore_armed = false;
                            self.gs_refresh_pending();
                        }
                        Err(e) => {
                            self.status = format!("Game Screens: {}", e);
                        }
                    }
                }
            }
            Message::GameScreensApply => {
                if self.gs_busy || self.gs_error.is_some() {
                    return Task::none();
                }
                let (Some(shared), Some(root)) =
                    (self.gs_shared.clone(), self.gs_root.clone())
                else {
                    return Task::none();
                };
                if self.gs_choice == GsChoice::Standard {
                    // The stock choice is a restore: capture
                    // the current pair, then put the oldest
                    // capture back. Two-step, like every
                    // destructive press on this page's
                    // neighbours.
                    if !self.gs_restore_armed {
                        self.gs_restore_armed = true;
                        self.status = "Restoring the stock screens replaces the current pair (it is captured first) — click Apply again to confirm.".to_string();
                        return Task::none();
                    }
                    self.gs_restore_armed = false;
                    let Some(oldest) = gamescreens::oldest_backup(&root) else {
                        self.status = "No stock capture exists yet — the first Apply of a preset or a custom image captures the current screens as the stock originals.".to_string();
                        return Task::none();
                    };
                    match gamescreens::capture_backup(&shared, &root)
                        .and_then(|_| gamescreens::restore_backup(&shared, &oldest))
                    {
                        Ok(()) => {
                            self.status =
                                "Stock screens restored from the oldest capture."
                                    .to_string();
                        }
                        Err(e) => {
                            self.status =
                                format!("Could not restore the stock screens: {:#}", e);
                        }
                    }
                    self.gs_reload();
                    return Task::none();
                }
                let (preset, art) = match self.gs_choice {
                    GsChoice::Preset(p) => {
                        let bytes = match p {
                            GsPreset::Daylight => DAYLIGHT_MASTER,
                            GsPreset::Dark => DARK_MASTER,
                        };
                        match gamescreens::decode_source(bytes) {
                            Ok(art) => (Some(p), art),
                            Err(e) => {
                                self.status = format!(
                                    "The bundled preset art could not be read: {:#}",
                                    e
                                );
                                return Task::none();
                            }
                        }
                    }
                    GsChoice::Custom => match &self.gs_custom_art {
                        Some(art) => (None, art.clone()),
                        None => {
                            self.status =
                                "Pick a custom image first.".to_string();
                            return Task::none();
                        }
                    },
                    GsChoice::Standard => unreachable!(),
                };
                self.gs_busy = true;
                self.status =
                    "Building the screens at 4096x4096 — this takes a few seconds."
                        .to_string();
                return Task::perform(
                    async move {
                        let (splash, blur) = gamescreens::build_pair(&art);
                        let sha = gamescreens::sha256_hex(&splash);
                        gamescreens::apply_pair(&shared, &root, &splash, &blur)
                            .map(|backup| {
                                (preset, backup.display().to_string(), sha)
                            })
                            .map_err(|e| format!("{:#}", e))
                    },
                    Message::GameScreensApplied,
                );
            }
            Message::GameScreensApplied(result) => {
                self.gs_busy = false;
                match result {
                    Ok((preset, backup, sha)) => {
                        if let (Some(p), Some(data_dir)) =
                            (preset, crate::paths::data_dir())
                        {
                            let mut hashes =
                                gamescreens::GeneratedHashes::load(&data_dir);
                            match p {
                                GsPreset::Daylight => {
                                    hashes.daylight_splash = Some(sha)
                                }
                                GsPreset::Dark => hashes.dark_splash = Some(sha),
                            }
                            let _ = hashes.save(&data_dir);
                        }
                        self.status = format!(
                            "Game screens applied. The previous screens are captured in {}.",
                            backup
                        );
                        self.gs_reload();
                    }
                    Err(e) => {
                        self.status = format!("Game Screens: {}", e);
                    }
                }
            }
            Message::GameScreensRestoreBackup(path) => {
                let (Some(shared), Some(root)) =
                    (self.gs_shared.clone(), self.gs_root.clone())
                else {
                    return Task::none();
                };
                match gamescreens::capture_backup(&shared, &root)
                    .and_then(|_| gamescreens::restore_backup(&shared, &path))
                {
                    Ok(()) => {
                        self.status =
                            "Backup restored; the screens it replaced were captured first."
                                .to_string();
                    }
                    Err(e) => {
                        self.status =
                            format!("Could not restore that backup: {:#}", e);
                    }
                }
                self.gs_reload();
            }
            Message::ImportKingModsFile(id) => {
                return Task::perform(
                    async move {
                        let picked = rfd::AsyncFileDialog::new()
                            .set_title(
                                "Choose the file you downloaded from KingMods",
                            )
                            .add_filter("Mod archive", &["zip"])
                            .pick_file()
                            .await
                            .map(|h| h.path().to_path_buf());
                        (id, picked)
                    },
                    |(id, picked)| Message::KingModsFilePicked(id, picked),
                );
            }
            Message::KingModsFilePicked(id, picked) => {
                return self.stage_manual_import(
                    id,
                    picked,
                    "KingMods (manual import)",
                );
            }
            Message::DownloadEvent(event) => match event {
                DownloadEvent::Progress {
                    job,
                    downloaded,
                    total,
                } => {
                    self.downloads.progress(job, downloaded, total);
                }
            },
            Message::ClearFinishedDownloads => {
                self.downloads.clear_finished();
                self.save_download_history();
            }
            Message::AppUpdateCheckDone(found) => {
                // The launch check's only voice: a newer
                // Playground exists. Every quiet outcome
                // (None) leaves the app exactly as it was.
                if let Some(release) = found {
                    self.app_update = Some(release);
                } else if self.app_update_check_manual {
                    // A "Check now" was asked out loud, so
                    // silence would read as broken: confirm
                    // the up-to-date answer in the status.
                    self.status = format!(
                        "Playground is up to date (v{}).",
                        env!("CARGO_PKG_VERSION")
                    );
                }
                self.app_update_check_manual = false;
            }
            Message::DismissAppUpdate => {
                self.app_update = None;
                self.app_update_failed = None;
                self.app_update_installing = false;
            }
            Message::InstallAppUpdate => {
                if let Some(release) = self.app_update.clone() {
                    if !self.app_update_installing {
                        self.app_update_installing = true;
                        self.app_update_failed = None;
                        self.status = format!(
                            "Downloading Playground {}…",
                            release.tag
                        );
                        return Task::perform(
                            async move {
                                app_update::download_and_stage(release)
                                    .await
                                    .map_err(|e| format!("{:#}", e))
                            },
                            Message::AppUpdateStaged,
                        );
                    }
                }
            }
            Message::AppUpdateStaged(result) => {
                self.app_update_installing = false;
                match result {
                    Ok(files) => {
                        // The new release is staged and
                        // verified. The swap itself cannot
                        // run here — this process is the
                        // binary being replaced — so hand
                        // it to update.exe and exit; the
                        // helper waits for this process to
                        // die, swaps, and relaunches.
                        match app_update::spawn_apply_helper(&files) {
                            Ok(_child) => return iced::exit(),
                            Err(e) => {
                                self.app_update_failed = Some(format!(
                                    "Could not start the update helper: {}",
                                    e
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        self.app_update_failed = Some(e);
                    }
                }
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
                self.grid_shown = 0.0;
                self.grid_self_scroll = false;
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
                self.grid_shown = 0.0;
                self.grid_self_scroll = false;
                return iced::widget::scrollable::scroll_to(
                    iced::widget::scrollable::Id::new("grid"),
                    iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                );
            }
            Message::CloseShowMenu => {
                self.show_menu_open = false;
            }
            Message::TabSelected(tab) => {
                self.show_menu_open = false;
                if tab == Tab::Settings {
                    // Every section starts collapsed on
                    // entry (John, 2026-10-23): sections
                    // arrive closed, instead of open waiting
                    // to be collapsed. Open-state never
                    // carries over from a previous visit.
                    self.settings_collapsed =
                        SETTINGS_SECTION_IDS.iter().map(|id| id.to_string()).collect();
                }
                if tab == Tab::Saves {
                    self.refresh_save_slots();
                }
                if tab == Tab::Layout {
                    // Read the migration record fresh on
                    // every open, and start disarmed: an
                    // arm left over from a previous visit
                    // must never carry into this one.
                    self.refresh_layout_standing();
                    self.layout_arm_apply = false;
                    self.layout_arm_rollback = false;
                    self.reclaim_arm = false;
                }
                self.current_tab = tab;
                if tab == Tab::Keybindings {
                    self.kb_reload();
                }
                if tab == Tab::GameScreens {
                    self.gs_reload();
                }
                if tab == Tab::Log {
                    return self.begin_log_load();
                }
                if tab == Tab::Settings {
                    // The Updates box and the Diagnostics
                    // summary both report the scheduled
                    // fill's registration; ask Windows once
                    // per opening of the page.
                    self.fill_task_registered = None;
                    return self.query_fill_task();
                }
            }
            Message::BrowseViewChanged(view) => {
                self.show_menu_open = false;
                self.view_filter = view;
                self.grid_offset = 0.0;
                self.grid_shown = 0.0;
                self.grid_self_scroll = false;
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
                } else if self.just_toggled_state {
                    self.just_toggled_state = false;
                } else {
                    if let Some(m) = self.mod_manager.get_mod(&id) {
                        let stem = ModManager::mod_stem(&m.path);
                        self.link_input = self
                            .mod_manager
                            .mod_link(&stem)
                            .map(|s| s.to_string())
                            .unwrap_or_default();
                    }
                    if self.selected_mod.as_deref() != Some(id.as_str()) {
                        // A fresh panel settles in (feel
                        // pass part 2).
                        self.details_opened_at = std::time::Instant::now();
                        self.kick_anim();
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
            Message::CardToggleState(id) => {
                // The slider click also reaches the card's select
                // handler right after; flag it to stand down once.
                self.just_toggled_state = true;
                let name = self.mod_manager.get_mod(&id).map(|m| m.name.clone());
                let is_active = self
                    .mod_manager
                    .get_mod(&id)
                    .map(|m| m.state == ModState::Active)
                    .unwrap_or(false);
                // Feel pass: the switch GLIDES — start its
                // animation from wherever the knob sits now
                // toward the state it is flipping to.
                let from = self.switch_t(&id, is_active);
                self.switch_anims.insert(
                    id.clone(),
                    SwitchAnim {
                        from,
                        to: if is_active { 0.0 } else { 1.0 },
                        started: std::time::Instant::now(),
                    },
                );
                self.kick_anim();
                // The slider runs the panel's own move operations:
                // active flips to archive, anything else flips to
                // active — under the layout these are the link
                // operations, so the card and the panel can never
                // disagree about what flipping means.
                let result = if is_active {
                    self.mod_manager.move_to_archive(&id)
                } else {
                    self.mod_manager.move_to_active(&id)
                };
                match result {
                    Ok(()) => {
                        self.status = format!(
                            "{} {}.",
                            if is_active { "Deactivated" } else { "Activated" },
                            name.unwrap_or_default()
                        )
                    }
                    Err(e) => self.status = format!("Toggle failed: {}", e),
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
            Message::RemoveHoldStarted(id) => {
                let name = self
                    .mod_manager
                    .get_mod(&id)
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| "this mod".to_string());
                self.remove_hold = Some((id, std::time::Instant::now()));
                self.status = format!(
                    "Keep holding to permanently delete {} — release to cancel.",
                    name
                );
            }
            Message::RemoveHoldReleased => {
                if let Some((id, _)) = self.remove_hold.take() {
                    let name = self
                        .mod_manager
                        .get_mod(&id)
                        .map(|m| m.name.clone())
                        .unwrap_or_else(|| "this mod".to_string());
                    self.status = format!(
                        "Delete cancelled — press and hold Remove to delete {}.",
                        name
                    );
                }
            }
            Message::AnimTick => {
                // The feel pass's shared clock: settle
                // every live animation a step.
                let now = std::time::Instant::now();
                // Status fade-in (part 2): a status the
                // clock has not seen yet arrived since the
                // last tick — stamp it, and its alpha ramps
                // from the stamp in the status row.
                if self.status != self.status_seen {
                    self.status_seen = self.status.clone();
                    self.status_changed_at = now;
                }
                self.switch_anims
                    .retain(|_, a| now.duration_since(a.started) < SWITCH_ANIM_DUR);
                // Rolling counts travel a third of the
                // remaining distance per tick, plus a
                // floor step so small moves still land.
                let mut traveling = false;
                for (shown, target) in self.shown_counts.values_mut() {
                    let t = *target as f32;
                    let diff = t - *shown;
                    if diff.abs() < 0.5 {
                        *shown = t;
                    } else {
                        traveling = true;
                        *shown += diff * 0.3 + diff.signum() * 0.5;
                        if (t - *shown).abs() < 0.5 {
                            *shown = t;
                        }
                    }
                }
                // Easing bars step toward their live
                // actuals; keys for finished downloads
                // are pruned.
                if self.layout_running.is_some() && self.layout_total > 0 {
                    let frac = self.layout_done as f32 / self.layout_total as f32;
                    self.step_bar("layout", frac);
                }
                let jobs: Vec<(String, f32)> = self
                    .downloads
                    .jobs()
                    .iter()
                    .filter(|j| j.state == DownloadJobState::Downloading)
                    .filter_map(|j| j.percent().map(|p| (format!("dl-{}", j.id), p)))
                    .collect();
                let live_keys: Vec<String> =
                    jobs.iter().map(|(k, _)| k.clone()).collect();
                for (k, p) in &jobs {
                    self.step_bar(k, *p);
                }
                self.eased_bars
                    .retain(|k, _| k == "layout" || live_keys.contains(k));
                // Smooth scrolling (part 2): the rendered
                // offset glides a step toward the target
                // the scrollable reported, and the
                // scrollable is moved to match — its echo
                // comes back flagged, never a new target.
                let mut grid_scroll_task = None;
                let grid_diff = self.grid_offset - self.grid_shown;
                if grid_diff.abs() > 0.5 {
                    self.grid_shown += grid_diff * 0.35;
                    if (self.grid_offset - self.grid_shown).abs() <= 0.5 {
                        self.grid_shown = self.grid_offset;
                    }
                    self.grid_self_scroll = true;
                    grid_scroll_task = Some(iced::widget::scrollable::scroll_to(
                        iced::widget::scrollable::Id::new("grid"),
                        iced::widget::scrollable::AbsoluteOffset {
                            x: 0.0,
                            y: self.grid_shown,
                        },
                    ));
                }
                // The clock lives while anything travels
                // or a job runs (the pulse needs it too).
                if traveling
                    || !self.switch_anims.is_empty()
                    || self.layout_running.is_some()
                    || self.downloads.live_count() > 0
                    || grid_scroll_task.is_some()
                    || self.status_changed_at.elapsed()
                        < std::time::Duration::from_millis(220)
                    || self.details_opened_at.elapsed()
                        < std::time::Duration::from_millis(160)
                    || self.menu_opened_at.elapsed()
                        < std::time::Duration::from_millis(160)
                {
                    self.anim_until = now + ANIM_TAIL;
                }
                if let Some(task) = grid_scroll_task {
                    return task;
                }
            }
            Message::CardHover(id) => {
                self.hovered_card = id;
            }
            Message::RemoveHoldTick => {
                let due = matches!(
                    &self.remove_hold,
                    Some((_, started)) if started.elapsed() >= REMOVE_HOLD_TIME
                );
                if due {
                    let (id, _) = self.remove_hold.take().unwrap();
                    self.execute_remove(&id);
                }
            }
            Message::SaveDirectories => return self.apply_directories(),
            Message::SettingsCheckConflictsToggled(on) => {
                self.settings_check_conflicts = on
            }
            Message::SettingsParallelScansChanged(value) => {
                self.settings_parallel_input = value
            }
            Message::SettingsCacheTtlChanged(value) => {
                self.settings_cache_ttl_input = value
            }
            Message::SettingsCatalogUrlChanged(value) => {
                self.settings_catalog_url_input = value
            }
            Message::SettingsSourceToggled(list, index, enabled) => {
                let entries = match list {
                    SourceList::ModHub => &mut self.settings_sources.mod_hub,
                    SourceList::GitHub => &mut self.settings_sources.github,
                    SourceList::KingMods => &mut self.settings_sources.kingmods,
                };
                if let Some(source) = entries.get_mut(index) {
                    source.enabled = enabled;
                }
            }
            Message::ThemeSelected(id) => {
                // Themes apply live and persist at once
                // (themes, 2026-10-23) — deliberately NOT
                // staged behind the page's Save settings:
                // a theme you cannot see while choosing is
                // no choice at all.
                if theme::set_theme(&id) {
                    let name = theme::current_theme().name;
                    match self.mod_manager.set_ui_theme(&id) {
                        Ok(()) => {
                            self.status = format!("Theme: {} — applied and saved.", name)
                        }
                        Err(e) => {
                            self.status = format!(
                                "Theme: {} — applied, but saving it failed: {}",
                                name, e
                            )
                        }
                    }
                }
            }
            Message::DownloadsTabSelected(tab) => {
                self.downloads_tab = tab;
            }
            Message::ImportItchFile(id) => {
                return Task::perform(
                    async move {
                        let picked = rfd::AsyncFileDialog::new()
                            .set_title(
                                "Choose the file you downloaded from itch.io",
                            )
                            .add_filter("Mod archive", &["zip"])
                            .pick_file()
                            .await
                            .map(|h| h.path().to_path_buf());
                        (id, picked)
                    },
                    |(id, picked)| Message::ItchFilePicked(id, picked),
                );
            }
            Message::ItchFilePicked(id, picked) => {
                return self.stage_manual_import(
                    id,
                    picked,
                    "itch.io (manual import)",
                );
            }
            Message::SettingsTabSelected(tab) => {
                self.settings_tab = tab;
                if tab == SettingsTab::Diagnostics
                    && self.fill_task_registered.is_none()
                {
                    return self.query_fill_task();
                }
            }
            Message::SettingsToggleSection(id) => {
                if !self.settings_collapsed.remove(&id) {
                    self.settings_collapsed.insert(id);
                }
            }
            Message::SettingsStartupCheckToggled(on) => {
                self.settings_startup_check = on
            }
            Message::SettingsSelfUpdateToggled(on) => {
                self.settings_self_update = on
            }
            Message::CheckAppUpdateNow => {
                self.app_update_check_manual = true;
                self.status = "Checking for a newer Playground…".to_string();
                return Task::perform(
                    app_update::check_at_launch(),
                    Message::AppUpdateCheckDone,
                );
            }
            Message::SettingsBrowseDefaultChanged(view) => {
                self.settings_browse_default = view
            }
            Message::SettingsStartMaximizedToggled(on) => {
                self.settings_start_maximized = on
            }
            Message::SettingsLaunchToggled(setting, on) => match setting {
                // The console stages like every other value
                // on the page now; game.xml is written when
                // the page saves (see apply_directories).
                LaunchSetting::Console => self.console_on = Some(on),
                LaunchSetting::SkipModUpdateDialog => {
                    self.settings_launch.skip_mod_update_dialog = on
                }
                LaunchSetting::Cheats => self.settings_launch.cheats = on,
                LaunchSetting::AutoStart => self.settings_launch.autostart = on,
                LaunchSetting::SkipStartVideos => {
                    self.settings_launch.skip_start_videos = on
                }
            },
            Message::SettingsLaunchSlotChanged(value) => {
                self.settings_launch_slot_input = value
            }
            Message::SettingsAutoFileToggled(on) => {
                self.settings_auto_file = on
            }
            Message::SettingsKeepHistoryToggled(on) => {
                self.settings_keep_history = on
            }
            Message::SettingsFillLogKeepChanged(value) => {
                self.settings_fill_log_keep_input = value
            }
            Message::SettingsLogErrorsOnlyDefaultToggled(on) => {
                self.settings_log_errors_only_default = on
            }
            Message::SettingsLogLinesCapChanged(value) => {
                self.settings_log_lines_cap_input = value
            }
            Message::FillTaskStatus(known) => {
                self.fill_task_registered = known;
            }
            Message::OpenFolder(path) => {
                if let Err(e) = open_folder(&path) {
                    self.status =
                        format!("Could not open {}: {}", path.display(), e);
                }
            }
            Message::OpenUrl(url) => {
                if let Err(e) = open_url(&url) {
                    self.status = format!("Could not open {}: {}", url, e);
                }
            }
            Message::CopyDiagnostics => {
                self.status = "Diagnostics copied to the clipboard.".to_string();
                return iced::clipboard::write(self.diagnostics_summary());
            }
            Message::CopyLaunchFlags => {
                // Copy the flags alone — the form Steam's
                // Launch Options field takes. The slot reads
                // from its field, exactly as the preview does.
                let slot = self
                    .settings_launch_slot_input
                    .trim()
                    .parse::<u8>()
                    .ok()
                    .filter(|n| (1..=20).contains(n));
                let flags = crate::config::launch_flags(
                    &crate::config::LaunchConfig {
                        slot,
                        ..self.settings_launch.clone()
                    },
                );
                if flags.is_empty() {
                    self.status =
                        "No launch options are set — nothing to copy.".to_string();
                } else {
                    self.status = "Launch flags copied — paste them into \
                                   Steam's Launch Options for FS25."
                        .to_string();
                    return iced::clipboard::write(flags);
                }
            }
            Message::ExportDiagnostics => {
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title("Export diagnostics")
                            .set_file_name("playground-diagnostics.txt")
                            .add_filter("Text", &["txt"])
                            .save_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::DiagnosticsExported,
                );
            }
            Message::DiagnosticsExported(picked) => {
                if let Some(path) = picked {
                    match std::fs::write(&path, self.diagnostics_summary()) {
                        Ok(()) => {
                            self.status = format!(
                                "Diagnostics exported to {}.",
                                path.display()
                            )
                        }
                        Err(e) => {
                            self.status =
                                format!("Could not export diagnostics: {}", e)
                        }
                    }
                }
            }
            Message::RereadEveryMod => {
                if self.loading {
                    self.status =
                        "Still loading — wait for it to finish before re-reading."
                            .to_string();
                } else {
                    match crate::mod_manager::library_db::clear_default() {
                        Ok(_) => {
                            self.status =
                                "Re-reading every mod from its folder…".to_string();
                            return self.begin_load();
                        }
                        Err(e) => {
                            self.status = format!(
                                "Could not clear the library database: {}",
                                e
                            );
                        }
                    }
                }
            }
            Message::BrowseFolder(target) => {
                let title = match target {
                    FolderTarget::Active => "Choose the game's mods folder",
                    FolderTarget::Archive => "Choose the library folder",
                    FolderTarget::Game => {
                        "Choose the Farming Simulator profile folder (holds game.xml)"
                    }
                    FolderTarget::GameInstall => {
                        "Choose the Farming Simulator install folder (holds the shared folder)"
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
                                        "Profile folder set: {}. Developer console is {}.",
                                        path.display(),
                                        if on { "on" } else { "off" }
                                    ),
                                    None => format!(
                                        "Profile folder set: {}, but no game.xml console switch was found there.",
                                        path.display()
                                    ),
                                };
                            }
                            Err(e) => {
                                self.status =
                                    format!("Could not save the profile folder: {}", e)
                            }
                        }
                    }
                    FolderTarget::GameInstall => {
                        // The Game Screens install folder: the
                        // pick must hold a shared folder (or be
                        // the shared folder itself) before it is
                        // saved — anything else is refused with
                        // the pick named, and the Game Screens
                        // page keeps the refusal on show.
                        match gamescreens::validate_install_pick(&path) {
                            Ok(install) => {
                                match self
                                    .mod_manager
                                    .set_game_install_dir(install.clone())
                                {
                                    Ok(()) => {
                                        self.game_install_dir_input =
                                            install.display().to_string();
                                        self.status = format!(
                                            "Game install folder set: {}.",
                                            install.display()
                                        );
                                        self.gs_reload();
                                    }
                                    Err(e) => {
                                        self.status = format!(
                                            "Could not save the game install folder: {}",
                                            e
                                        )
                                    }
                                }
                            }
                            Err(e) => {
                                let reason = e.reason();
                                self.status = format!(
                                    "Game install folder not set — {}",
                                    reason
                                );
                                self.gs_pick_note = Some(reason);
                            }
                        }
                    }
                }
            }
            Message::RefreshLog => return self.begin_log_load(),
            Message::LogLoaded(outcome) => match outcome {
                Ok(o) => {
                    self.log_lines = o.lines;
                    self.log_suspects = o.suspects;
                    self.log_error_count = o.error_count;
                    self.log_warning_count = o.warning_count;
                    self.log_total_lines = o.total_lines;
                    self.log_note = String::new();
                    // The reader opens as the Diagnostics
                    // default says (Round 2): with the
                    // errors/warnings-only filter already
                    // applied when the setting has it on.
                    self.log_errors_only =
                        self.mod_manager.log_errors_only_default();
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
            Message::RestoreWindowId(id) => {
                // The handover's first fact. Collection
                // starts exactly once per process:
                // restore_window is returned only by the
                // boot transition, and any repeat finds
                // the state past Idle and is dropped —
                // the frame restore must never be set in
                // motion twice (the decorations toggle is
                // stateful; a second issue could flip a
                // restored window's frame back off).
                if !matches!(self.window_restore, RestoreState::Idle) {
                    return Task::none();
                }
                match id {
                    Some(id) => {
                        self.window_restore = RestoreState::Collecting {
                            id,
                            deadline: std::time::Instant::now()
                                + RESTORE_POSITION_WAIT,
                        };
                        return iced::window::get_position(id)
                            .map(Message::RestoreWindowPosition);
                    }
                    // No window exists to hand over (the
                    // shell is gone): nothing to collect,
                    // nothing to verify.
                    None => {
                        self.window_restore = RestoreState::Settled;
                        return Task::none();
                    }
                }
            }
            Message::RestoreWindowPosition(pos) => {
                let RestoreState::Collecting { id, .. } = self.window_restore
                else {
                    // A late answer, after the deadline
                    // fallback already restored the window:
                    // dropped on purpose. Applying the
                    // move now would be a second, separate
                    // shape change — the lurch the batched
                    // handover exists to remove.
                    return Task::none();
                };
                self.window_restore = RestoreState::Verifying {
                    id,
                    check_at: std::time::Instant::now() + RESTORE_VERIFY_DELAY,
                };
                return match pos {
                    // Both facts are in: frame, size, and
                    // centering land as ONE batched shape
                    // change.
                    Some(pos) => Task::batch(vec![
                        iced::window::toggle_decorations(id),
                        iced::window::resize(id, Size::new(1280.0, 800.0)),
                        iced::window::move_to(id, Self::main_top_left(pos)),
                    ]),
                    // The read answered with no position:
                    // shape without the move.
                    None => Self::restore_shape(id),
                };
            }
            Message::RestoreTick => return match self.window_restore {
                RestoreState::Collecting { id, deadline }
                    if std::time::Instant::now() >= deadline =>
                {
                    // The position read never answered
                    // (its oneshot stream can end with no
                    // output at all): the deadline, not
                    // the read, is what guarantees the
                    // restore fires. Shape without the
                    // move — off-center beats misshapen.
                    self.window_restore = RestoreState::Verifying {
                        id,
                        check_at: std::time::Instant::now()
                            + RESTORE_VERIFY_DELAY,
                    };
                    Self::restore_shape(id)
                }
                RestoreState::Verifying { id, check_at }
                    if std::time::Instant::now() >= check_at =>
                {
                    self.window_restore = RestoreState::VerifyRead { id };
                    iced::window::get_size(id).map(Message::RestoreVerifySize)
                }
                _ => Task::none(),
            },
            Message::RestoreVerifySize(size) => {
                let RestoreState::VerifyRead { id } = self.window_restore
                else {
                    return Task::none();
                };
                self.window_restore = RestoreState::Settled;
                // "Start maximized" (Settings > General >
                // Behavior, Round 2): the restore has settled
                // either way at this point in the handover —
                // the one place a launch may grow the window
                // past its restored size.
                let maximize = self.mod_manager.start_maximized();
                if Self::still_splash_sized(size) {
                    // The batched restore demonstrably
                    // never landed, so issue it once more.
                    // The stateful decorations toggle
                    // rides along safely: it is only ever
                    // re-issued when the coupled resize is
                    // provably missing too, so it cannot
                    // flip a restored window's frame back
                    // off. The centering rides its own
                    // chain here, as the net's always has
                    // — at this point correctness beats
                    // choreography. One check, at most
                    // one re-issue, per boot: no retry
                    // loop.
                    let mut tasks = vec![
                        iced::window::toggle_decorations(id),
                        iced::window::resize(id, Size::new(1280.0, 800.0)),
                        Self::center_main_window(id),
                    ];
                    if maximize {
                        tasks.push(iced::window::maximize(id, true));
                    }
                    return Task::batch(tasks);
                }
                if maximize {
                    return iced::window::maximize(id, true);
                }
            }
            Message::ToggleShowMenu => {
                self.show_menu_open = !self.show_menu_open;
                if self.show_menu_open {
                    // A fresh menu settles in (feel pass
                    // part 2).
                    self.menu_opened_at = std::time::Instant::now();
                    self.kick_anim();
                }
            }
            Message::GridScrolled(viewport) => {
                let y = viewport.absolute_offset().y;
                self.grid_view_h = viewport.bounds().height;
                if self.grid_self_scroll {
                    // The echo of the clock's own scroll_to:
                    // it confirms where the eased rendering
                    // already is, and must not move the
                    // target (smooth scrolling, part 2).
                    self.grid_self_scroll = false;
                    self.grid_shown = y;
                } else {
                    self.grid_offset = y;
                    let diff = self.grid_offset - self.grid_shown;
                    // A jump of most of the viewport — a
                    // scrollbar haul, a page leap — snaps;
                    // easing it would fight the hand on the
                    // bar. Ordinary wheel travel glides: the
                    // first eased step is issued right here,
                    // before the next frame can paint the
                    // un-eased position the wheel reported.
                    let snap = (self.grid_view_h * 0.75).max(CARD_H + GRID_SPACING);
                    if diff.abs() > snap {
                        self.grid_shown = self.grid_offset;
                    } else if diff.abs() > 0.5 {
                        self.grid_shown += diff * 0.35;
                        if (self.grid_offset - self.grid_shown).abs() <= 0.5 {
                            self.grid_shown = self.grid_offset;
                        }
                        self.grid_self_scroll = true;
                        self.kick_anim();
                        return iced::widget::scrollable::scroll_to(
                            iced::widget::scrollable::Id::new("grid"),
                            iced::widget::scrollable::AbsoluteOffset {
                                x: 0.0,
                                y: self.grid_shown,
                            },
                        );
                    } else {
                        self.grid_shown = self.grid_offset;
                    }
                }
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
            .filter_map(|m| m.categories.first().cloned())
            .collect();
        cats.sort();
        cats.dedup();
        view_options.extend(cats.into_iter().map(BrowseView::Category));
        // Stat boxes across the top (John, round 4 — Hub's tiles).
        // Six since 2026-10-21 (John's order, his labels): total /
        // active / archived / favorite / maps / updates available.
        // The state tallies come from the records themselves, the
        // same `state` field the Browse filters match on, so a
        // tile can never disagree with the list it names.
        let all_mods = self.mod_manager.get_mods();
        let (total_count, active_count, archived_count) =
            tally_states(all_mods.iter().map(|m| m.state.clone()));
        let maps_count = all_mods
            .iter()
            .filter(|m| m.categories.first().map_or(false, |c| c == "Maps"))
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
            tile(self.shown_count("mods", total_count).to_string(), "TOTAL MODS"),
            tile(self.shown_count("active", active_count).to_string(), "ACTIVE MODS"),
            tile(
                self.shown_count("archived", archived_count).to_string(),
                "ARCHIVED MODS",
            ),
            tile(
                self.shown_count("favs", self.mod_manager.favorites_count())
                    .to_string(),
                "FAVORITE MODS",
            ),
            tile(self.shown_count("maps", maps_count).to_string(), "MAPS"),
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
                "UPDATES AVAILABLE",
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
                    // Drawn, not typed: the UI font has no ▾
                    // glyph (it rendered as a hollow box on
                    // John's build) — same fix as the star,
                    // including its fixed-size container wrap,
                    // mirroring the star badge's usage exactly.
                    container(iced::widget::canvas(DropdownTick).width(12).height(12))
                        .width(12)
                        .height(12)
                        .center_x(12)
                        .center_y(12),
                ]
                .spacing(8),
            )
            .width(200)
            .padding(8)
            .on_press(Message::ToggleShowMenu)
            // The field wears the highlighted buttons' color
            // now (John, 2026-10-21) — the accent from
            // ui::theme, not the old flat grey.
            .style(theme::dropdown_field()),
            text_input("Search Mods / Author", &self.search_query)
                .style(theme::input_field())
                .on_input(Message::SearchChanged)
                .padding(8)
                .width(300)
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
                    "No mods found. Put mod zips or folders into your game mods folder or library folder, then Rescan."
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
            // Width math from the real shell geometry
            // (John's margin rule): window padding 24 +
            // sidebar + the shell's 10 spacing +
            // PAGE_MARGIN + SCROLLBAR_W on the right, so
            // the last card ends PAGE_MARGIN + SCROLLBAR_W
            // from the edge — a true PAGE_MARGIN clear of
            // the scrollbar, which paints inside the
            // edge's own SCROLLBAR_W band. (Deducting
            // GRID_SPACING instead left the cards ~2px
            // from the edge; PAGE_MARGIN alone left them
            // flush against the bar.)
            let content_w = (self.window_size.width
                - 24.0
                - SIDEBAR_W
                - 10.0
                - PAGE_MARGIN
                - SCROLLBAR_W)
                .max(150.0);
            let grid_w = if pane_open {
                // The pane-open grid scrollable is narrower
                // than the page (pane + spacing + the body
                // row's own PAGE_MARGIN pad), so its bar
                // band eats a SCROLLBAR_W the closed grid
                // never sees — deduct it once more, or the
                // bar paints over the last card's right
                // edge. Net either way: cards end
                // PAGE_MARGIN + SCROLLBAR_W from their
                // scrollable's edge.
                (content_w - DETAILS_W - GRID_SPACING - SCROLLBAR_W).max(150.0)
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
            // The window renders from the EASED offset
            // (smooth scrolling, part 2); the scrollable
            // itself is stepped to the same place by the
            // animation clock, so window and scrollbar
            // agree frame by frame.
            let first_row = ((self.grid_shown / pitch).floor() as usize)
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
                        self.hovered_card.as_deref() == Some(m.id.as_str()),
                        self.switch_t(&m.id, m.state == ModState::Active),
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
            // The pane keeps PAGE_MARGIN on its right too
            // (John's rule, no exceptions): the row rides
            // in a right-padded container, and the grid's
            // content_w above already reserved the pane's
            // width plus this margin, so grid and pane
            // never overlap. The pane's own inner padding
            // (14) is untouched.
            container(
                row![content, details]
                    .spacing(GRID_SPACING)
                    .height(Length::Fill),
            )
            .padding(iced::Padding {
                right: PAGE_MARGIN,
                ..Default::default()
            })
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
            let mut options_col = column![].spacing(2).width(200);
            for opt in view_options {
                let selected = opt == self.view_filter;
                options_col = options_col.push(
                    button(text(format!("{}", opt)))
                        .width(200)
                        .padding([6, 10])
                        .on_press(Message::BrowseViewChanged(opt))
                        .style(theme::nav_pill(selected)),
                );
            }
            let menu_inner: Element<'_, Message> =
                if option_count > 12 {
                    scrollable(options_col).height(336).into()
                } else {
                    options_col.into()
                };
            Some(
                stack![
                    button(Space::with_width(0))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(|_, _| button::Style {
                            background: None,
                            ..button::Style::default()
                        })
                        .on_press(Message::CloseShowMenu),
                    column![
                    // The settle (feel pass part 2): the
                    // menu opens a few pixels low and eases
                    // up into place over ~160ms.
                    Space::with_height(self.settle_offset(self.menu_opened_at)),
                    row![
                        Space::with_width(52),
                        container(menu_inner).padding(4).style(|_| container::Style {
                            background: Some(iced::Background::Color(theme::base())),
                            border: iced::Border {
                                color: iced::Color::from_rgb(0.35, 0.36, 0.40),
                                width: 1.0,
                                radius: 4.0.into(),
                            },
                            ..Default::default()
                        }),
                    ],
                    ]
                ]
                .into(),
            )
        } else {
            None
        };

        // The tiles/toolbar and the body are returned separately:
        // view() draws them in different stack layers (see there).
        // The top column rides in a right-padded container so
        // the tiles end PAGE_MARGIN from the edge (John's
        // margin rule); only the visible content insets —
        // view()'s opaque cover slab behind it stays
        // full-width, so the scroll-clip fix keeps its cover.
        (
            container(column![tiles, toolbar].spacing(8))
                .padding(iced::Padding {
                    right: PAGE_MARGIN,
                    ..Default::default()
                })
                .width(Length::Fill)
                .into(),
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

        // The close button leads the header now (John,
        // 2026-10-21): on the LEFT, round, in the accent —
        // a styled button, so the circle is a quad and
        // shows in captures. Its glyph rides a fixed-size
        // centered container, the star badge's pattern.
        let close_btn = button(
            container(text("X").size(13))
                .width(28)
                .height(28)
                .center_x(28)
                .center_y(28),
        )
        .width(28)
        .height(28)
        .padding(0)
        .style(theme::close_button())
        .on_press(Message::CloseDetails);

        let mut col = column![
            // The settle (feel pass part 2): the panel's
            // contents open a few pixels low and ease up
            // into place over ~160ms.
            Space::with_height(self.settle_offset(self.details_opened_at)),
            row![close_btn, text(&m.name).size(20)]
                .spacing(8)
                .align_y(iced::Alignment::Center),
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
                    "update_available" if row.source_kind == "kingmods" => {
                        "Update available — KingMods is manual: Downloads > Other Links lists it with its page link."
                    }
                    "update_available" if row.source_kind == "itch" => {
                        "Update available — itch.io is manual: Downloads > Other Links lists it with its page link."
                    }
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
        // Updates (ModHub): link by page URL or numeric mod_id.
        // The check button in the update box serves both sources.
        // The link fields themselves are built at the panel's
        // bottom now (John, 2026-10-21: fill boxes last).
        let mh_linked = self
            .mod_manager
            .modhub_link(&stem)
            .map(|s| s.to_string());

        // The update box rides above the action cluster
        // (John, 2026-10-21): the status printout over a
        // Check button wearing the cluster's pill, then
        // the install button when an update is on offer.
        if linked.is_some() || mh_linked.is_some() {
            let state_line = match self.update_status.get(&m.id) {
                Some(UpdateState::Checking) => "Checking for updates…".to_string(),
                Some(UpdateState::UpToDate) => "Up to date.".to_string(),
                Some(UpdateState::Available(info)) => {
                    format!("Update available: {} ({})", info.version, info.tag)
                }
                Some(UpdateState::Installing) => {
                    // The live readout is the Downloads job's
                    // own state (the same numbers the
                    // Downloads page shows); the plain line
                    // remains for a state with no job behind
                    // it, which no longer occurs in practice.
                    match self.downloads.live_job_for_mod(&m.id) {
                        Some(job) => match job.state {
                            DownloadJobState::Queued => {
                                "Queued — waiting for the running download to finish."
                                    .to_string()
                            }
                            DownloadJobState::Downloading => match job.percent() {
                                Some(p) => format!(
                                    "Downloading v{}… {:.0}% ({} of {})",
                                    job.version,
                                    p,
                                    format_size(job.downloaded),
                                    format_size(job.total.unwrap_or(0)),
                                ),
                                None => format!(
                                    "Downloading v{}… {} so far",
                                    job.version,
                                    format_size(job.downloaded)
                                ),
                            },
                            DownloadJobState::Installing => {
                                "Downloaded — installing…".to_string()
                            }
                            _ => "Downloading and installing…".to_string(),
                        },
                        None => "Downloading and installing…".to_string(),
                    }
                }
                Some(UpdateState::Failed(e)) => format!("Check failed: {}", e),
                None => "Not checked yet.".to_string(),
            };
            col = col.push(text(state_line).size(12));
            col = col.push(
                button(centered_label(text("Check for update")))
                    .width(Length::Fill)
                    .style(theme::nav_pill(true))
                    .on_press(Message::CheckUpdate(m.id.clone())),
            );
            if let Some(UpdateState::Available(info)) = self.update_status.get(&m.id) {
                let label = format!("Download & install v{}", info.version);
                col = col.push(
                    button(centered_label(text(label)))
                        .width(Length::Fill)
                        .style(theme::nav_pill(true))
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
            let label = format!("Download & install v{}", info.version);
            col = col.push(
                button(centered_label(text(label)))
                    .width(Length::Fill)
                    .style(theme::nav_pill(true))
                    .on_press(Message::InstallUpdate(m.id.clone())),
            );
            if m.mod_type != crate::mod_manager::types::ModType::Zip {
                col = col.push(
                    text("Update installs work on .zip mods; this is a folder mod.").size(12),
                );
            }
        }
        // The primary actions, together (John, 2026-10-21):
        // favourite, move, and remove used to be scattered
        // down the panel with the whole updates section
        // between them. Now they sit in one cluster of pills
        // in the menu's language — the highlighted pill's
        // accent for favourite and move, the Rescan pill's
        // danger red for remove. Same cluster as before —
        // except remove itself, which is hold-to-delete
        // now (feel pass, 2026-10-23).
        let move_label = if m.state == ModState::Active {
            "Move to Archive"
        } else {
            "Move to Active"
        };
        let move_message = if m.state == ModState::Active {
            Message::MoveToArchive(m.id.clone())
        } else {
            Message::MoveToActive(m.id.clone())
        };
        // Hold-to-delete (feel pass, 2026-10-23): the
        // Remove control is held, not clicked twice. One
        // mouse_area throughout the hold — only its
        // contents change — so the release that cancels
        // always lands on the widget that saw the press.
        // The bar under the label is the hold clock made
        // visible; releasing early cancels.
        let remove_label = if self.remove_holding(&m.id) {
            "Keep holding…"
        } else {
            "Remove mod"
        };
        // Hold polish (feel pass part 2): as the hold
        // fills, the control squeezes — its label steps
        // down a point, its sides step in, and the red
        // brightens toward the hover shade — so the
        // press reads as pressure, not just a timer.
        let hold_p = self.remove_hold_progress(&m.id);
        // Resting height matches the Favorite/Move pills
        // (cosmetic round D1, John, 2026-10-23): the hold
        // bar no longer stacks under the label — while a
        // hold is live it overlays the control's bottom
        // edge, so the progress shows without the control
        // standing thicker than its neighbours at rest.
        let remove_base = container(centered_label(
            text(remove_label).size(14.0 - hold_p),
        ))
        .width(Length::Fill)
        .padding(iced::Padding {
            top: 5.0,
            right: 12.0 + 5.0 * hold_p,
            bottom: 5.0,
            left: 12.0 + 5.0 * hold_p,
        })
        .style(theme::danger_box_hold(hold_p));
        let remove_body: Element<'_, Message> = if hold_p > 0.0 {
            stack![
                remove_base,
                container(
                    column![
                        Space::with_height(Length::Fill),
                        progress_bar(0.0..=1.0, hold_p)
                            .height(4)
                            .style(theme::hold_bar),
                    ]
                    .width(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .into()
        } else {
            remove_base.into()
        };
        let remove_control = mouse_area(remove_body)
        .on_press(Message::RemoveHoldStarted(m.id.clone()))
        .on_release(Message::RemoveHoldReleased)
        .on_exit(Message::RemoveHoldReleased)
        .interaction(iced::mouse::Interaction::Pointer);
        col = col.push(
            column![
                button(centered_label(text(fav_label).size(14)))
                    .width(Length::Fill)
                    .style(theme::nav_pill(true))
                    .on_press(Message::ToggleFavorite(m.id.clone())),
                button(centered_label(text(move_label).size(14)))
                    .width(Length::Fill)
                    .style(theme::nav_pill(true))
                    .on_press(move_message),
                remove_control,
            ]
            .spacing(6),
        );

        // Link this mod — the fill boxes close the panel
        // (John, 2026-10-21): the GitHub and ModHub link
        // fields and their buttons sit below all other
        // content, with the linking hint when neither a
        // hand link nor a database offer exists.
        col = col.push(text("Updates (GitHub):").size(13));
        col = col.push(
            row![
                text_input("owner/repo or repo URL", &self.link_input)
                    .style(theme::input_field())
                    .on_input(Message::LinkInputChanged)
                    .size(13),
                button(text(if linked.is_some() { "Save link" } else { "Link" }).size(13))
                    .style(theme::nav_pill(true))
                    .on_press(Message::SaveModLink(m.id.clone())),
            ]
            .spacing(6),
        );
        if linked.is_none()
            && mh_linked.is_none()
            && !matches!(
                self.update_status.get(&m.id),
                Some(UpdateState::Available(_))
            )
        {
            col = col.push(
                text("Link this mod to its GitHub repo to check it for updates.").size(12),
            );
        }

        // The flyout's bottom margin (cosmetic round D3,
        // John, 2026-10-23): the content used to run flush
        // to the panel's bottom edge.
        col = col.push(Space::with_height(10));
        Some(
            container(scrollable(col))
                .width(DETAILS_W)
                .height(Length::Fill)
                .padding(14)
                .style(|_| container::Style {
                    // The sidebar's ground and edge (John,
                    // 2026-10-21: the panel background is the
                    // menu's background), plus the buttons'
                    // pale ice as the panel's text color —
                    // mirrors theme::sidebar_container, with
                    // the text color that style leaves unset.
                    background: Some(iced::Background::Color(theme::base())),
                    text_color: Some(theme::text()),
                    border: iced::Border {
                        color: theme::panel(),
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..container::Style::default()
                })
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
        // view's hold-to-delete flow and recomputes
        // conflicts afterwards. Other conflict types get a simple
        // card each.
        let card_style = |_: &Theme| container::Style {
            background: Some(iced::Background::Color(theme::base())),
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
        // Same shell geometry as the Browse grid (John's
        // margin rule): window padding 24 + sidebar + the
        // shell's 10 spacing + PAGE_MARGIN + SCROLLBAR_W on
        // the right. The bar here belongs to the outer
        // page_scroll below; its pad does not stack with
        // this formula — card positions are computed from
        // the left, and the pad only caps the page box —
        // so the net is cards ending PAGE_MARGIN +
        // SCROLLBAR_W from the page's edge, a true
        // PAGE_MARGIN clear of the bar (at PAGE_MARGIN
        // alone they sat flush against it).
        let content_w = (self.window_size.width
            - 24.0
            - SIDEBAR_W
            - 10.0
            - PAGE_MARGIN
            - SCROLLBAR_W)
            .max(200.0);
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
                // Remove control (John): a red circle with
                // a minus, sitting where Browse cards carry
                // the favorite star. Hold-to-delete (feel
                // pass, 2026-10-23): press and hold it —
                // while held, the control becomes the
                // filling pill; the delete fires only when
                // the hold completes, and releasing early
                // cancels. One mouse_area throughout (its
                // contents swap under it), so the release
                // always lands on the widget that saw the
                // press.
                let btn_content: Element<'_, Message> = if self.remove_holding(&m.id) {
                    // The same squeeze as the panel's
                    // control (hold polish, part 2): the
                    // pill narrows and brightens as the
                    // hold fills.
                    let hold_p = self.remove_hold_progress(&m.id);
                    container(
                        column![
                            text("Keep holding…").size(11.0 - 0.5 * hold_p),
                            progress_bar(0.0..=1.0, hold_p)
                                .height(3)
                                .style(theme::hold_bar),
                        ]
                        .spacing(3),
                    )
                    .width(110.0 - 6.0 * hold_p)
                    .padding(6.0 + 2.0 * hold_p)
                    .style(theme::danger_box_hold(hold_p))
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
                    circle.into()
                };
                let btn: Element<'_, Message> = mouse_area(btn_content)
                    .on_press(Message::RemoveHoldStarted(m.id.clone()))
                    .on_release(Message::RemoveHoldReleased)
                    .on_exit(Message::RemoveHoldReleased)
                    .interaction(iced::mouse::Interaction::Pointer)
                    .into();
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

        page_scroll(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Configure Save (Roadmap #3): the five-step wizard —
    /// pick the save, edit its settings, pick the loadout,
    /// review, then apply and read the results. The header
    /// frame (title, subtitle, breadcrumb) rides on every
    /// step, and the whole wizard wears the app's current
    /// design language — no Round A-era styling survives
    /// inside it (design requirement, 2026-10-21).
    fn saves_view(&self) -> Element<'_, Message> {
        if self.mod_manager.profile_dir().is_none() {
            return container(
                text("Savegames appear here once FS25 has written its profile folder (Documents/My Games/FarmingSimulator2025).")
                    .size(14),
            )
            .padding(16)
            .into();
        }
        let body: Element<'_, Message> = match self.wizard_step {
            0 => self.wizard_select_view(),
            1 => self.wizard_settings_view(),
            2 => self.wizard_loadout_view(),
            3 => self.wizard_review_view(),
            _ => self.wizard_results_view(),
        };
        column![self.wizard_header(), body]
            .spacing(12)
            .height(Length::Fill)
            .into()
    }

    /// The frame on every wizard page: the title, John's
    /// subtitle, and the breadcrumb — finished steps and
    /// the current one lit, each a jump target.
    fn wizard_header(&self) -> Element<'_, Message> {
        const STEPS: [&str; 5] =
            ["Select Save", "Settings", "Pick Mods", "Review", "Results"];
        let mut crumbs = row![].spacing(6);
        for (i, label) in STEPS.iter().enumerate() {
            let n = i as u8;
            crumbs = crumbs.push(
                button(text(format!("{} · {}", i + 1, label)).size(12))
                    .style(theme::nav_pill(n <= self.wizard_step))
                    .on_press(Message::WizardStep(n)),
            );
        }
        column![
            text("Configure Save Game").size(20).color(theme::text()),
            text("Link an existing FS25 save with your mod loadout and settings")
                .size(13)
                .color(theme::text_dim()),
            crumbs,
        ]
        .spacing(6)
        .into()
    }

    fn wizard_step_buttons(
        &self,
        back: Option<u8>,
        next: Option<(u8, &'static str)>,
    ) -> Element<'_, Message> {
        let mut r = row![].spacing(8);
        if let Some(b) = back {
            r = r.push(
                button(text("Back"))
                    .style(theme::nav_pill(false))
                    .on_press(Message::WizardStep(b)),
            );
        }
        r = r.push(Space::with_width(Length::Fill));
        if let Some((n, label)) = next {
            r = r.push(
                button(text(label))
                    .style(theme::nav_pill(true))
                    .on_press(Message::WizardStep(n)),
            );
        }
        r.into()
    }

    /// Step 1 — Select Save. Slots are cards in the current
    /// language; each shows its own recorded mod count
    /// (from its careerSavegame.xml), its map, play time,
    /// money, and any stored loadout.
    fn wizard_select_view(&self) -> Element<'_, Message> {
        let mut col = column![
            text("Pick the save to configure. Configure Save works on saves the game has already written: bounce into the map, save, come back out — then adjust settings, money, and mods here before you go back in.")
                .size(13)
                .color(theme::text_dim())
                .width(Length::Fill),
        ]
        .spacing(10);
        // C1 (cosmetic round, John, 2026-10-23): ALL 20
        // slots as a 4-across grid — occupied slots show
        // their save, empty slots show as empty ("no
        // save"), muted and non-selectable, never omitted.
        // C1-plus: an occupied card carries its save's MAP
        // ICON when the save's map resolves to a library
        // map mod (its extracted icon); a neutral plate
        // covers a map that can't be resolved.
        let map_icon_for = |slot: &crate::mod_manager::saves::SaveSlot| {
            let want = slot
                .map
                .as_deref()?
                .to_lowercase()
                .replace(|c: char| !c.is_alphanumeric(), "");
            if want.len() < 4 {
                return None;
            }
            self.mod_manager
                .get_mods()
                .into_iter()
                .filter(|m| {
                    m.categories.first().map_or(false, |c| c == "Maps")
                })
                .find(|m| {
                    let have = m
                        .name
                        .to_lowercase()
                        .replace(|c: char| !c.is_alphanumeric(), "");
                    have.contains(&want) || want.contains(&have)
                })
                .and_then(|m| self.icons.get(&m.id).cloned())
        };
        let mut grid = column![].spacing(8);
        for row_start in (1..=20usize).step_by(4) {
            let mut r = row![].spacing(8);
            for n in row_start..row_start + 4 {
                let found = self
                    .save_slots_cache
                    .iter()
                    .find(|slot| slot.slot == n);
                let cell: Element<'_, Message> = match found {
                    Some(slot) => {
                        let selected = self.saves_selected.as_deref()
                            == Some(slot.dir_name.as_str());
                        let title = match &slot.name {
                            Some(name) => {
                                format!("Savegame {} — {}", slot.slot, name)
                            }
                            None => format!("Savegame {}", slot.slot),
                        };
                        let icon: Element<'_, Message> =
                            match map_icon_for(slot) {
                                Some(handle) => iced::widget::image(handle)
                                    .width(Length::Fixed(44.0))
                                    .height(Length::Fixed(44.0))
                                    .content_fit(iced::ContentFit::Cover)
                                    .into(),
                                None => container(
                                    text("MAP").size(10).color(theme::text_dim()),
                                )
                                .width(Length::Fixed(44.0))
                                .height(Length::Fixed(44.0))
                                .center_x(44.0)
                                .center_y(44.0)
                                .style(|_| container::Style {
                                    background: Some(iced::Background::Color(
                                        theme::base(),
                                    )),
                                    border: iced::Border {
                                        radius: 6.0.into(),
                                        ..iced::Border::default()
                                    },
                                    ..container::Style::default()
                                })
                                .into(),
                            };
                        let loadout =
                            self.mod_manager.save_loadout(&slot.dir_name);
                        let lines = column![
                            text(title).size(13).color(theme::text()),
                            text(format!(
                                "{} · {} mods · {} · {}",
                                slot.map.as_deref().unwrap_or("unknown map"),
                                slot.mod_count,
                                slot.play_time_display()
                                    .as_deref()
                                    .unwrap_or("?"),
                                slot.money
                                    .map(fmt_money)
                                    .unwrap_or_else(|| "?".to_string()),
                            ))
                            .size(11)
                            .color(theme::text_dim()),
                            text(if loadout.is_empty() {
                                "No loadout saved".to_string()
                            } else {
                                format!("Loadout: {} mods", loadout.len())
                            })
                            .size(11)
                            .color(theme::text_dim()),
                        ]
                        .spacing(2);
                        button(
                            row![icon, lines]
                                .spacing(8)
                                .align_y(iced::Alignment::Center),
                        )
                        .width(Length::FillPortion(1))
                        .padding(9)
                        .style(move |_theme, _status| {
                            iced::widget::button::Style {
                                background: Some(iced::Background::Color(
                                    theme::panel(),
                                )),
                                border: iced::Border {
                                    color: if selected {
                                        theme::accent()
                                    } else {
                                        theme::base()
                                    },
                                    width: if selected { 2.0 } else { 1.0 },
                                    radius: 8.0.into(),
                                },
                                text_color: theme::text(),
                                ..Default::default()
                            }
                        })
                        .on_press(Message::SelectSaveSlot(
                            slot.dir_name.clone(),
                        ))
                        .into()
                    }
                    None => container(
                        column![
                            text(format!("Savegame {}", n))
                                .size(13)
                                .color(theme::text_dim()),
                            text("no save")
                                .size(11)
                                .color(theme::text_dim()),
                        ]
                        .spacing(2),
                    )
                    .width(Length::FillPortion(1))
                    .padding(9)
                    .style(|_| container::Style {
                        background: Some(iced::Background::Color(theme::base())),
                        border: iced::Border {
                            color: theme::panel(),
                            width: 1.0,
                            radius: 8.0.into(),
                        },
                        ..container::Style::default()
                    })
                    .into(),
                };
                r = r.push(cell);
            }
            grid = grid.push(r);
        }
        col = col.push(grid);
        let next: Element<'_, Message> = if self.saves_selected.is_some() {
            self.wizard_step_buttons(None, Some((1, "Next: Settings")))
        } else {
            text("Pick a save to continue.")
                .size(12)
                .color(theme::text_dim())
                .into()
        };
        column![page_scroll(col).height(Length::Fill), next]
            .spacing(10)
            .height(Length::Fill)
            .into()
    }

    /// Step 2 — Settings. Every verified field, grouped as
    /// the design groups them. A field the save doesn't
    /// carry renders as "Not in this save yet" with no
    /// control; a field whose labels aren't verified
    /// (disaster state, dirt interval, helper sources)
    /// shows its stored value and says so.
    fn wizard_settings_view(&self) -> Element<'_, Message> {
        let Some(s) = &self.wizard_settings else {
            return column![
                text("Pick a save first — its settings load here.").size(14),
                self.wizard_step_buttons(Some(0), None),
            ]
            .spacing(10)
            .into();
        };
        let s = s.clone();
        let fin = self.wizard_finances.clone();
        let num_key = |v: f64| -> String {
            if v == v.trunc() {
                format!("{}", v as i64)
            } else {
                format!("{}", v)
            }
        };

        // ---- General ------------------------------------
        let mut general: Vec<Element<'_, Message>> = Vec::new();
        general.push(wizard_opt_row(
            "Save Name",
            s.savegame_name.is_some(),
            text_input("Save name", s.savegame_name.as_deref().unwrap_or(""))
                .style(theme::input_field())
                .on_input(Message::WizardNameChanged)
                .padding(8)
                .into(),
        ));
        general.push(wizard_opt_row(
            "Economic Difficulty",
            s.economic_difficulty.is_some(),
            wizard_segment(
                "difficulty",
                vec![
                    ("EASY".to_string(), "Easy".to_string()),
                    ("NORMAL".to_string(), "Normal".to_string()),
                    ("HARD".to_string(), "Hard".to_string()),
                ],
                s.economic_difficulty.as_deref().unwrap_or(""),
            ),
        ));
        general.push(wizard_opt_row(
            "Timescale",
            s.time_scale.is_some(),
            wizard_segment(
                "timescale",
                [
                    "0", "0.5", "1", "2", "3", "5", "6", "10", "15", "20",
                    "30", "60", "120",
                ]
                .iter()
                .map(|v| {
                    (
                        v.to_string(),
                        if *v == "0" {
                            "Pause".to_string()
                        } else {
                            format!("{}x", v)
                        },
                    )
                })
                .collect(),
                &s.time_scale.map(num_key).unwrap_or_default(),
            ),
        ));
        general.push(wizard_opt_row(
            "Autosave Interval",
            s.auto_save_interval.is_some(),
            wizard_segment(
                "autosave",
                vec![
                    ("0".to_string(), "Off".to_string()),
                    ("5".to_string(), "5 min".to_string()),
                    ("10".to_string(), "10 min".to_string()),
                    ("15".to_string(), "15 min".to_string()),
                    ("30".to_string(), "30 min".to_string()),
                    ("60".to_string(), "60 min".to_string()),
                ],
                &s.auto_save_interval.map(num_key).unwrap_or_default(),
            ),
        ));
        // Cash and loan: exact-value fields with preset
        // chips, and the amount the save holds right now.
        let money_now = |which: &str| -> String {
            let v = match which {
                "cash" => fin.as_ref().and_then(|f| f.money),
                _ => fin.as_ref().and_then(|f| f.loan),
            };
            match v {
                Some(v) => format!("In the save now: {}", fmt_money(v)),
                None => "Not in this save".to_string(),
            }
        };
        let mut cash_chips = row![].spacing(4);
        for (label, delta) in [
            ("+10,000", 10_000i64),
            ("+100,000", 100_000),
            ("+1,000,000", 1_000_000),
            ("-10,000", -10_000),
            ("-100,000", -100_000),
        ] {
            cash_chips = cash_chips.push(
                button(text(label).size(11))
                    .style(theme::nav_pill(false))
                    .on_press(Message::WizardCashChip(delta)),
            );
        }
        general.push(wizard_row(
            "Adjust Cash",
            column![
                row![
                    text_input("Cash", &self.wizard_cash)
                        .style(theme::input_field())
                        .on_input(Message::WizardCashChanged)
                        .padding(8)
                        .width(220),
                    text(money_now("cash")).size(12).color(theme::text_dim()),
                ]
                .spacing(8),
                cash_chips,
            ]
            .spacing(6)
            .into(),
        ));
        let mut loan_chips = row![].spacing(4);
        for (label, delta) in [
            ("+10,000", 10_000i64),
            ("+50,000", 50_000),
            ("+100,000", 100_000),
            ("-10,000", -10_000),
            ("-50,000", -50_000),
        ] {
            loan_chips = loan_chips.push(
                button(text(label).size(11))
                    .style(theme::nav_pill(false))
                    .on_press(Message::WizardLoanChip(delta)),
            );
        }
        general.push(wizard_row(
            "Adjust Loan",
            column![
                row![
                    text_input("Loan", &self.wizard_loan)
                        .style(theme::input_field())
                        .on_input(Message::WizardLoanChanged)
                        .padding(8)
                        .width(220),
                    text(money_now("loan")).size(12).color(theme::text_dim()),
                ]
                .spacing(8),
                loan_chips,
            ]
            .spacing(6)
            .into(),
        ));
        general.push(wizard_opt_row(
            "Traffic",
            s.traffic_enabled.is_some(),
            wizard_bool_pills("traffic", s.traffic_enabled.unwrap_or(false), "Off", "On"),
        ));
        general.push(wizard_opt_row(
            "Snow",
            s.snow_enabled.is_some(),
            wizard_bool_pills("snow", s.snow_enabled.unwrap_or(false), "Off", "On"),
        ));

        // ---- Seasons ------------------------------------
        let mut seasons: Vec<Element<'_, Message>> = Vec::new();
        seasons.push(wizard_opt_row(
            "Seasonal Growth",
            s.growth_mode.is_some(),
            wizard_segment(
                "growth",
                vec![
                    ("1".to_string(), "Seasonal".to_string()),
                    ("2".to_string(), "Paused".to_string()),
                    ("3".to_string(), "Off".to_string()),
                ],
                &s.growth_mode.map(|v| v.to_string()).unwrap_or_default(),
            ),
        ));
        seasons.push(wizard_opt_row(
            "Days per Month",
            s.planned_days_per_period.is_some(),
            row![
                iced::widget::slider(
                    1.0..=28.0,
                    s.planned_days_per_period.unwrap_or(1) as f32,
                    |v| Message::WizardSetValue(
                        "days".to_string(),
                        format!("{}", v.round() as i64),
                    ),
                )
                .step(1.0_f32)
                .width(220),
                text(format!(
                    "{} day{}",
                    s.planned_days_per_period.unwrap_or(1),
                    if s.planned_days_per_period == Some(1) {
                        ""
                    } else {
                        "s"
                    }
                ))
                .size(13)
                .color(theme::text()),
            ]
            .spacing(10)
            .into(),
        ));
        seasons.push(wizard_opt_row(
            "Fixed Visual Month",
            s.fixed_visual_month.is_some(),
            wizard_bool_pills(
                "fixed_visual",
                s.fixed_visual_month.unwrap_or(false),
                "Off",
                "On",
            ),
        ));

        // ---- Crops and Growth ---------------------------
        let mut crops: Vec<Element<'_, Message>> = Vec::new();
        crops.push(wizard_opt_row(
            "Crop Destruction",
            s.fruit_destruction.is_some(),
            wizard_bool_pills(
                "fruit_destruction",
                s.fruit_destruction.unwrap_or(false),
                "Off",
                "On",
            ),
        ));
        crops.push(wizard_opt_row(
            "Periodic Plowing Required",
            s.plowing_required_enabled.is_some(),
            wizard_bool_pills(
                "plowing",
                s.plowing_required_enabled.unwrap_or(false),
                "Off",
                "On",
            ),
        ));
        crops.push(wizard_opt_row(
            "Fieldstone",
            s.stones_enabled.is_some(),
            wizard_bool_pills("stones", s.stones_enabled.unwrap_or(false), "Off", "On"),
        ));
        crops.push(wizard_opt_row(
            "Lime Required",
            s.lime_required.is_some(),
            wizard_bool_pills("lime", s.lime_required.unwrap_or(false), "Off", "On"),
        ));
        crops.push(wizard_opt_row(
            "Weeds",
            s.weeds_enabled.is_some(),
            wizard_bool_pills("weeds", s.weeds_enabled.unwrap_or(false), "Off", "On"),
        ));
        crops.push(wizard_opt_row(
            "Disaster Destruction",
            s.disaster_destruction_state.is_some(),
            row![
                text(s.disaster_destruction_state.clone().unwrap_or_default())
                    .size(13)
                    .color(theme::text()),
                text("Shown as stored — its other values aren't verified yet.")
                    .size(11)
                    .color(theme::text_dim()),
            ]
            .spacing(8)
            .into(),
        ));

        // ---- Vehicle Control ----------------------------
        let mut vehicle: Vec<Element<'_, Message>> = Vec::new();
        vehicle.push(wizard_opt_row(
            "Dirt",
            s.dirt_interval.is_some(),
            row![
                text(format!(
                    "Stored value: {}",
                    s.dirt_interval.unwrap_or(0)
                ))
                .size(13)
                .color(theme::text()),
                text("Its Off/Slow/Normal/Fast labels aren't verified yet.")
                    .size(11)
                    .color(theme::text_dim()),
            ]
            .spacing(8)
            .into(),
        ));
        vehicle.push(wizard_opt_row(
            "Automatic Engine Start",
            s.automatic_motor_start_enabled.is_some(),
            wizard_bool_pills(
                "motor_start",
                s.automatic_motor_start_enabled.unwrap_or(false),
                "Off",
                "On",
            ),
        ));
        vehicle.push(wizard_opt_row(
            "Stop & Go Braking",
            s.stop_and_go_braking.is_some(),
            wizard_bool_pills(
                "stop_and_go",
                s.stop_and_go_braking.unwrap_or(false),
                "Off",
                "On",
            ),
        ));
        vehicle.push(wizard_opt_row(
            "Trailer Fill Limit",
            s.trailer_fill_limit.is_some(),
            wizard_bool_pills(
                "trailer_fill",
                s.trailer_fill_limit.unwrap_or(false),
                "Off",
                "On",
            ),
        ));
        vehicle.push(wizard_opt_row(
            "Fuel Usage",
            s.fuel_usage.is_some(),
            wizard_segment(
                "fuel",
                vec![
                    ("1".to_string(), "Low".to_string()),
                    ("2".to_string(), "Normal".to_string()),
                    ("3".to_string(), "High".to_string()),
                ],
                &s.fuel_usage.map(|v| v.to_string()).unwrap_or_default(),
            ),
        ));

        // ---- AI Workers ---------------------------------
        let mut ai: Vec<Element<'_, Message>> = Vec::new();
        ai.push(wizard_opt_row(
            "Refill — Fuel",
            s.helper_buy_fuel.is_some(),
            wizard_bool_pills(
                "helper_fuel",
                s.helper_buy_fuel.unwrap_or(false),
                "Off",
                "Buy",
            ),
        ));
        ai.push(wizard_opt_row(
            "Refill — Seeds",
            s.helper_buy_seeds.is_some(),
            wizard_bool_pills(
                "helper_seeds",
                s.helper_buy_seeds.unwrap_or(false),
                "Off",
                "Buy",
            ),
        ));
        ai.push(wizard_opt_row(
            "Refill — Fertilizer",
            s.helper_buy_fertilizer.is_some(),
            wizard_bool_pills(
                "helper_fertilizer",
                s.helper_buy_fertilizer.unwrap_or(false),
                "Off",
                "Buy",
            ),
        ));
        for (label, value) in [
            ("Refill — Slurry Source", s.helper_slurry_source),
            ("Refill — Manure Source", s.helper_manure_source),
        ] {
            ai.push(wizard_opt_row(
                label,
                value.is_some(),
                row![
                    text(format!("Stored value: {}", value.unwrap_or(0)))
                        .size(13)
                        .color(theme::text()),
                    text("Its Farm/Buy labels aren't verified yet.")
                        .size(11)
                        .color(theme::text_dim()),
                ]
                .spacing(8)
                .into(),
            ));
        }

        // ---- Auto Drive (owner's section, verbatim) -----
        let mut ad: Vec<Element<'_, Message>> = Vec::new();
        ad.push(
            text("If AutoDrive is installed, please provide AutoDrive_Config to overwrite route configuration.")
                .size(12)
                .color(theme::text_dim())
                .width(Length::Fill)
                .into(),
        );
        let picked_label = match &self.wizard_autodrive {
            Some(p) => p.display().to_string(),
            None => "No file chosen".to_string(),
        };
        let mut box_row = row![
            container(text(picked_label).size(12).color(theme::text()))
                .width(Length::Fill)
                .padding(8)
                .style(wizard_inset),
            button(text("Browse"))
                .style(theme::nav_pill(false))
                .on_press(Message::WizardBrowseAutoDrive),
        ]
        .spacing(8);
        if self.wizard_autodrive.is_some() {
            box_row = box_row.push(
                button(text("Clear"))
                    .style(theme::nav_pill(false))
                    .on_press(Message::WizardClearAutoDrive),
            );
        }
        ad.push(box_row.into());
        ad.push(
            text(if self.wizard_autodrive_existing {
                "This save already has an AutoDrive_config.xml — the picked file will replace it."
            } else {
                "This save has no AutoDrive_config.xml yet — the picked file will be added."
            })
            .size(12)
            .color(theme::text_dim())
            .into(),
        );

        let sections = column![
            wizard_section("General", general),
            wizard_section("Seasons", seasons),
            wizard_section("Crops and Growth", crops),
            wizard_section("Vehicle Control", vehicle),
            wizard_section("AI Workers", ai),
            wizard_section("Auto Drive", ad),
        ]
        .spacing(12);
        column![
            page_scroll(sections).height(Length::Fill),
            self.wizard_step_buttons(Some(0), Some((2, "Next: Pick Mods"))),
        ]
        .spacing(10)
        .height(Length::Fill)
        .into()
    }

    /// How many card columns the loadout step fits across:
    /// the Browse grid's arithmetic, at the wizard's card
    /// width.
    fn wizard_card_columns(&self) -> usize {
        // The deduction past the sidebar is the shell's
        // real non-content width (John's margin rule):
        // window padding 24 + the shell's 10 spacing +
        // PAGE_MARGIN on the right = 44. (It was 64, which
        // left a ragged ~30px piled up on the right.)
        let available = self.window_size.width - SIDEBAR_W - 44.0;
        ((available / 262.0).floor() as usize).clamp(1, 6)
    }

    /// One loadout card: icon, name, author, version, and
    /// the badges (REQ MOD, category, Selected) — the mod
    /// card's facts in the theme palette, clicked to toggle.
    fn wizard_mod_card(&self, e: &WizardModEntry) -> Element<'_, Message> {
        let selected = self.saves_draft.contains(&e.key);
        let icon: Element<'_, Message> = match self.icons.get(&e.id) {
            Some(handle) => iced::widget::image(handle.clone())
                .width(46)
                .height(46)
                .content_fit(iced::ContentFit::Cover)
                .into(),
            None => container(text(""))
                .width(46)
                .height(46)
                .style(wizard_inset)
                .into(),
        };
        let mut info = column![text(e.name.clone()).size(13).color(theme::text())]
            .spacing(2);
        if let Some(author) = &e.author {
            info = info.push(text(author.clone()).size(11).color(theme::text_dim()));
        }
        info = info.push(
            text(format!("v{}", e.version))
                .size(11)
                .color(theme::text_dim()),
        );
        let mut badges = row![].spacing(4);
        if e.required {
            badges = badges.push(wizard_pill(
                "REQ MOD".to_string(),
                theme::accent(),
                theme::text(),
            ));
        }
        if let Some(cat) = &e.category {
            badges = badges.push(wizard_pill(
                cat.clone(),
                theme::base(),
                theme::text_dim(),
            ));
        }
        if selected {
            badges = badges.push(wizard_pill(
                "Selected".to_string(),
                theme::accent(),
                theme::text(),
            ));
        }
        let content = column![row![icon, info].spacing(8), badges].spacing(6);
        let card = container(content)
            .width(252)
            .padding(8)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(theme::base())),
                border: iced::Border {
                    color: if selected {
                        theme::accent()
                    } else {
                        theme::base()
                    },
                    width: if selected { 2.0 } else { 1.0 },
                    radius: 8.0.into(),
                },
                ..container::Style::default()
            });
        iced::widget::mouse_area(card)
            .on_release(Message::DraftToggle(e.key.clone()))
            .into()
    }

    /// Step 3 — Pick Mods. Collapsible card groups:
    /// Required by Map (the save's own required mods,
    /// badged REQ MOD, pre-selected by the seeding),
    /// Favourites, then the rest of the library — each
    /// with Select All / Deselect All. Mods the save
    /// records that the library doesn't hold are named as
    /// missing, never silently dropped.
    fn wizard_loadout_view(&self) -> Element<'_, Message> {
        let mut col = column![].spacing(10);
        if !self.wizard_seed_note.is_empty() {
            col = col.push(
                text(self.wizard_seed_note.clone())
                    .size(12)
                    .color(theme::text_dim())
                    .width(Length::Fill),
            );
        }
        if !self.wizard_missing.is_empty() {
            col = col.push(
                container(
                    column![
                        text(format!(
                            "In the save's list but not in your library ({}):",
                            self.wizard_missing.len()
                        ))
                        .size(13)
                        .color(theme::text()),
                        text(self.wizard_missing.join(", "))
                            .size(12)
                            .color(theme::text_dim()),
                    ]
                    .spacing(3),
                )
                .width(Length::Fill)
                .padding(10)
                .style(wizard_panel),
            );
        }
        col = col.push(
            row![
                button(text("Use the save's list"))
                    .style(theme::nav_pill(false))
                    .on_press(Message::WizardUseSaveList),
                button(text("Use current Active set"))
                    .style(theme::nav_pill(false))
                    .on_press(Message::DraftUseActive),
                button(text("Empty"))
                    .style(theme::nav_pill(false))
                    .on_press(Message::DraftEmpty),
                text(format!(
                    "{} mods in this loadout",
                    self.saves_draft.len()
                ))
                .size(13)
                .color(theme::text()),
            ]
            .spacing(8),
        );
        col = col.push(
            text_input("Filter mods by name…", &self.saves_filter)
                .style(theme::input_field())
                .on_input(Message::SavesFilterChanged)
                .padding(8),
        );
        let filter = self.saves_filter.to_lowercase();
        for (gid, title, entries) in self.wizard_groups() {
            let visible: Vec<&WizardModEntry> = entries
                .iter()
                .filter(|e| {
                    filter.is_empty() || e.name.to_lowercase().contains(&filter)
                })
                .collect();
            let collapsed = self.wizard_collapsed.contains(&gid);
            let sel_count = entries
                .iter()
                .filter(|e| self.saves_draft.contains(&e.key))
                .count();
            col = col.push(
                row![
                    button(text(format!(
                        "{} — {} mods ({} selected)",
                        title,
                        entries.len(),
                        sel_count
                    ))
                    .size(13))
                    .style(theme::nav_pill(false))
                    .on_press(Message::WizardCollapseGroup(gid.clone())),
                    button(text("Select all").size(12))
                        .style(theme::nav_pill(false))
                        .on_press(Message::WizardGroupToggle(gid.clone(), true)),
                    button(text("Deselect all").size(12))
                        .style(theme::nav_pill(false))
                        .on_press(Message::WizardGroupToggle(gid.clone(), false)),
                ]
                .spacing(6),
            );
            if collapsed {
                continue;
            }
            if visible.is_empty() {
                col = col.push(
                    text("Nothing in this group matches the filter.")
                        .size(12)
                        .color(theme::text_dim()),
                );
                continue;
            }
            let cols = self.wizard_card_columns();
            let shown = visible.len().min(150);
            let mut idx = 0;
            while idx < shown {
                let mut r = row![].spacing(10);
                for _ in 0..cols {
                    if idx < shown {
                        r = r.push(self.wizard_mod_card(visible[idx]));
                        idx += 1;
                    }
                }
                col = col.push(r);
            }
            if visible.len() > shown {
                col = col.push(
                    text(format!(
                        "Showing the first {} of {} — filter by name to find the rest.",
                        shown,
                        visible.len()
                    ))
                    .size(12)
                    .color(theme::text_dim()),
                );
            }
        }
        column![
            page_scroll(col).height(Length::Fill),
            self.wizard_step_buttons(Some(1), Some((3, "Next: Review"))),
        ]
        .spacing(10)
        .height(Length::Fill)
        .into()
    }

    /// Step 4 — Review. The summary table, every change
    /// the apply will make, the honesty line naming what
    /// apply touches (careerSavegame.xml AND farms.xml —
    /// owner ruling, 2026-10-21), and the inline
    /// validation that blocks applying.
    fn wizard_review_view(&self) -> Element<'_, Message> {
        let mut col = column![text("Ready to Apply").size(17).color(theme::text())]
            .spacing(10);
        let slot = self
            .save_slots_cache
            .iter()
            .find(|s| Some(&s.dir_name) == self.saves_selected.as_ref());
        let s = self.wizard_settings.clone();
        let o = self.wizard_original.clone();

        // ---- Summary ------------------------------------
        let mut summary: Vec<Element<'_, Message>> = Vec::new();
        let save_line = match (slot, s.as_ref()) {
            (Some(slot), Some(s)) => format!(
                "{} (Savegame {})",
                s.savegame_name.as_deref().unwrap_or(""),
                slot.slot
            ),
            (Some(slot), None) => format!("Savegame {}", slot.slot),
            _ => "—".to_string(),
        };
        summary.push(wizard_row("Save", text(save_line).size(13).into()));
        summary.push(wizard_row(
            "Map",
            text(
                slot.and_then(|s| s.map.clone())
                    .unwrap_or_else(|| "—".to_string()),
            )
            .size(13)
            .into(),
        ));
        summary.push(wizard_row(
            "Mod Folder",
            text(self.mod_manager.get_active_dir().display().to_string())
                .size(13)
                .into(),
        ));
        summary.push(wizard_row(
            "Mods selected",
            text(format!("{}", self.saves_draft.len()))
                .size(13)
                .into(),
        ));
        summary.push(wizard_row(
            "Difficulty",
            text(
                s.as_ref()
                    .and_then(|s| s.economic_difficulty.clone())
                    .unwrap_or_else(|| "—".to_string()),
            )
            .size(13)
            .into(),
        ));
        summary.push(wizard_row(
            "Timescale",
            text(
                s.as_ref()
                    .and_then(|s| s.time_scale)
                    .map(|v| {
                        if v == 0.0 {
                            "Pause".to_string()
                        } else {
                            format!("{}x", v)
                        }
                    })
                    .unwrap_or_else(|| "—".to_string()),
            )
            .size(13)
            .into(),
        ));
        col = col.push(wizard_section("Summary", summary));

        // ---- Changes ------------------------------------
        let mut changes: Vec<Element<'_, Message>> = Vec::new();
        if let (Some(o), Some(s)) = (&o, &s) {
            let lines = savegame::describe_changes(o, s);
            if lines.is_empty() {
                changes.push(
                    text("Settings: no changes.").size(12).color(theme::text_dim()).into(),
                );
            }
            for line in lines {
                changes.push(text(line).size(12).color(theme::text()).into());
            }
        }
        if let Some(fin) = &self.wizard_finances {
            let new_money = parse_amount(&self.wizard_cash);
            let new_loan = parse_amount(&self.wizard_loan);
            if new_money.is_some() && new_money != fin.money {
                changes.push(
                    text(format!(
                        "Cash: {} → {} (farms.xml)",
                        fin.money.map(fmt_money).unwrap_or_else(|| "—".to_string()),
                        fmt_money(new_money.unwrap())
                    ))
                    .size(12)
                    .color(theme::text())
                    .into(),
                );
            }
            if new_loan.is_some() && new_loan != fin.loan {
                changes.push(
                    text(format!(
                        "Loan: {} → {} (farms.xml)",
                        fin.loan.map(fmt_money).unwrap_or_else(|| "—".to_string()),
                        fmt_money(new_loan.unwrap())
                    ))
                    .size(12)
                    .color(theme::text())
                    .into(),
                );
            }
        }
        if let Some(picked) = &self.wizard_autodrive {
            changes.push(
                text(format!(
                    "AutoDrive route: {} will replace AutoDrive_config.xml in the save folder.",
                    picked
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ))
                .size(12)
                .color(theme::text())
                .into(),
            );
        }
        changes.push(
            text(format!(
                "Loadout: {} mods will be in the mods folder; the loadout is saved for this slot.",
                self.saves_draft.len()
            ))
            .size(12)
            .color(theme::text())
            .into(),
        );
        if !self.wizard_missing.is_empty() {
            changes.push(
                text(format!(
                    "{} of the save's recorded mods aren't in your library and can't be added: {}",
                    self.wizard_missing.len(),
                    self.wizard_missing.join(", ")
                ))
                .size(12)
                .color(theme::text_dim())
                .into(),
            );
        }
        col = col.push(wizard_section("Changes", changes));

        // ---- Honesty line + validation + apply ---------
        let slot_name = self
            .saves_selected
            .as_deref()
            .unwrap_or("the save folder");
        col = col.push(
            text(format!(
                "What apply touches: careerSavegame.xml and farms.xml in {} (a .playground-backup copy of each is written first), the links in the mods folder, and gameSettings.xml's mods-directory line if it doesn't already point at the mods folder. Nothing is touched until you apply — and don't apply while the game is running.",
                slot_name
            ))
            .size(12)
            .color(theme::text_dim())
            .width(Length::Fill),
        );
        let problems = self.wizard_validation();
        for p in &problems {
            col = col.push(text(p.clone()).size(13).color(theme::DANGER));
        }
        let apply_label = if self.wizard_arm_apply {
            "Click again to apply"
        } else {
            "Generate Folder & Apply"
        };
        let mut apply_btn = button(text(apply_label))
            .style(theme::nav_pill(true));
        if problems.is_empty() {
            apply_btn = apply_btn.on_press(Message::WizardApply);
        }
        col = col.push(
            row![
                button(text("Back"))
                    .style(theme::nav_pill(false))
                    .on_press(Message::WizardStep(2)),
                Space::with_width(Length::Fill),
                apply_btn,
            ]
            .spacing(8),
        );
        page_scroll(col).height(Length::Fill).into()
    }

    /// Step 5 — Results: two lists, what succeeded and
    /// what may have failed, kept apart so a failure never
    /// reads as a success.
    fn wizard_results_view(&self) -> Element<'_, Message> {
        let mut col = column![text("Results").size(17).color(theme::text())]
            .spacing(10);
        if self.wizard_done.is_empty() && self.wizard_failed.is_empty() {
            col = col.push(
                text("Nothing has been applied yet.")
                    .size(13)
                    .color(theme::text_dim()),
            );
        }
        if !self.wizard_done.is_empty() {
            let mut list =
                column![text("What succeeded").size(14).color(theme::text())]
                    .spacing(3);
            for line in &self.wizard_done {
                list = list.push(text(line.clone()).size(12).color(theme::text()));
            }
            col = col.push(
                container(list)
                    .width(Length::Fill)
                    .padding(10)
                    .style(wizard_panel),
            );
        }
        if !self.wizard_failed.is_empty() {
            let mut list = column![
                text("What may have failed").size(14).color(theme::DANGER)
            ]
            .spacing(3);
            for line in &self.wizard_failed {
                list = list.push(text(line.clone()).size(12).color(theme::text()));
            }
            col = col.push(
                container(list)
                    .width(Length::Fill)
                    .padding(10)
                    .style(wizard_panel),
            );
        } else if !self.wizard_done.is_empty() {
            col = col.push(
                text("Nothing failed.").size(13).color(theme::text()),
            );
        }
        col = col.push(
            row![
                button(text("Configure another save"))
                    .style(theme::nav_pill(false))
                    .on_press(Message::WizardReset),
                button(text("Back to Browse"))
                    .style(theme::nav_pill(true))
                    .on_press(Message::TabSelected(Tab::Browse)),
            ]
            .spacing(8),
        );
        page_scroll(col).height(Length::Fill).into()
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
                button(text("Refresh"))
                    .style(theme::nav_pill(true))
                    .on_press(Message::RefreshLog),
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
                .width(Length::Fill)
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
        let mut lines_col = column![].spacing(1).width(Length::Fill);
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
        // No page_scroll here (John, 2026-10-22: the pad
        // read as "way too big" on this page): log lines
        // have no hard right edge, so the pad only pushed
        // wrapped text further in. The lines ride in a
        // plain scrollable, as they did before the margin
        // pass — the deliberate exception to the rule.
        col = col.push(
            scrollable(lines_col)
                .width(Length::Fill)
                .height(Length::Fill),
        );
        col.into()
    }

    /// Downloads: two tabs (owner, 2026-10-24) — Jobs,
    /// this session's mod downloads as tracked jobs, and
    /// Other Links, the manual sources' page. The tab
    /// strip follows the Settings page's pattern.
    fn downloads_view(&self) -> Element<'_, Message> {
        let tab_button = |tab: DownloadsTab, label: &str| {
            button(text(label.to_string()))
                .style(theme::nav_pill(self.downloads_tab == tab))
                .on_press(Message::DownloadsTabSelected(tab))
        };
        let tabs = row![
            tab_button(DownloadsTab::Jobs, "Downloads"),
            tab_button(DownloadsTab::OtherLinks, "Other Links"),
        ]
        .spacing(6);
        let content: Element<'_, Message> = match self.downloads_tab {
            DownloadsTab::Jobs => self.downloads_jobs(),
            DownloadsTab::OtherLinks => self.other_links_view(),
        };
        column![text("Downloads").size(20), tabs, content]
            .spacing(8)
            .into()
    }

    /// The Jobs tab: every mod download this session as a
    /// tracked job (owner, 2026-10-22 — "shows which mods
    /// are being downloaded"). Job cards in the suspects
    /// box's dress, the list in a scrollable that fills
    /// the page. Live jobs (queued or working) first with
    /// their real progress, then the finished ones with
    /// their outcomes; a failure wears the Log Reader's
    /// error color and keeps its reason.
    fn downloads_jobs(&self) -> Element<'_, Message> {
        let mut header = row![].spacing(8);
        if self.downloads.has_finished() {
            header = header
                .push(Space::with_width(Length::Fill))
                .push(
                    button(text("Clear finished"))
                        .style(theme::nav_pill(true))
                        .on_press(Message::ClearFinishedDownloads),
                );
        }
        let mut col = column![header].spacing(8);
        if self.downloads.is_empty() {
            col = col.push(text("No downloads yet this session.").size(13));
            return col.into();
        }
        let card = |job: &DownloadJob| -> Element<'_, Message> {
            let mut inner = column![
                text(job.mod_name.clone()).size(14),
                text(format!(
                    "{} · {} · update to v{}",
                    job.file_name, job.source, job.version
                ))
                .size(12),
            ]
            .spacing(3);
            match job.state {
                DownloadJobState::Queued => {
                    inner = inner.push(
                        text("Queued — starts when the running download finishes.")
                            .size(12),
                    );
                }
                DownloadJobState::Downloading => {
                    if let (Some(p), Some(total)) = (job.percent(), job.total) {
                        inner = inner.push(
                            progress_bar(
                                0.0..=100.0,
                                self.eased_bar(&format!("dl-{}", job.id), p),
                            )
                            .width(Length::Fill)
                            .height(10)
                            .style(theme::download_bar),
                        );
                        inner = inner.push(
                            text(format!(
                                "{:.0}% — {} of {}",
                                p,
                                format_size(job.downloaded),
                                format_size(total)
                            ))
                            .size(12),
                        );
                    } else {
                        // No stated total: the byte count
                        // alone — never an invented total,
                        // percent, or speed.
                        inner = inner.push(
                            text(format!(
                                "Downloading… {} so far (the server stated no size)",
                                format_size(job.downloaded)
                            ))
                            .size(12),
                        );
                    }
                }
                DownloadJobState::Installing => {
                    inner = inner.push(text("Downloaded — installing…").size(12));
                }
                DownloadJobState::Done => {
                    inner = inner.push(text("Done — installed.").size(12));
                }
                DownloadJobState::Failed => {
                    let reason = job.fail_reason.as_deref().unwrap_or("unknown error");
                    inner = inner.push(
                        text(format!("Failed: {}", reason))
                            .size(12)
                            .color(iced::Color::from_rgb(0.96, 0.46, 0.40)),
                    );
                }
            }
            container(inner)
                .padding(8)
                .width(Length::Fill)
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
                .into()
        };
        let mut list = column![].spacing(8);
        let live: Vec<&DownloadJob> = self
            .downloads
            .jobs()
            .iter()
            .filter(|j| j.is_live())
            .collect();
        let finished: Vec<&DownloadJob> = self
            .downloads
            .jobs()
            .iter()
            .filter(|j| j.is_finished())
            .collect();
        if !live.is_empty() {
            list = list.push(text("Downloading and queued").size(13));
            for job in live {
                list = list.push(card(job));
            }
        }
        if !finished.is_empty() {
            list = list.push(text("Finished this session").size(13));
            for job in finished {
                list = list.push(card(job));
            }
        }
        col = col.push(
            page_scroll(list)
                .width(Length::Fill)
                .height(Length::Fill),
        );
        col.into()
    }

    /// The Other Links rows (owner, 2026-10-24), in two
    /// separated sections. KingMods: the mods with an
    /// update waiting — the fill still reads KingMods
    /// pages, so the judgement is real — judged against
    /// the version actually installed. itch.io: EVERY
    /// library mod whose home is itch — its page reads
    /// are failing (owner, 2026-10-24), so no update
    /// judgement exists to filter by; the section is
    /// the click-through checklist itself. Rows come
    /// from the update database first, then the
    /// catalog-other store for mods a fill hasn't
    /// recorded yet (itch only: a KingMods row with no
    /// judged latest has no update waiting by rule).
    fn other_link_rows(&self) -> (Vec<OtherLinkRow>, Vec<OtherLinkRow>) {
        let mods = self.mod_manager.get_mods();
        let mut by_stem: HashMap<String, &Mod> = HashMap::new();
        for m in mods {
            let stem = ModManager::mod_stem(&m.path);
            let replace = match by_stem.get(&stem) {
                None => true,
                Some(cur) => {
                    cur.state != ModState::Active && m.state == ModState::Active
                }
            };
            if replace {
                by_stem.insert(stem, m);
            }
        }
        let mut kingmods: Vec<OtherLinkRow> = Vec::new();
        let mut itch: Vec<OtherLinkRow> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (stem, row) in &self.db_rows {
            let Some(m) = by_stem.get(stem) else {
                continue;
            };
            let Some(url) = row.source_ref.clone() else {
                continue;
            };
            match row.source_kind.as_str() {
                "kingmods" => {
                    seen.insert(stem.clone());
                    if update_db::update_waiting(row, &m.version) {
                        kingmods.push(OtherLinkRow {
                            mod_id: m.id.clone(),
                            name: m.name.clone(),
                            file_name: row.file_name.clone(),
                            installed_version: m.version.clone(),
                            latest_version: row.latest_version.clone(),
                            url,
                        });
                    }
                }
                "itch" => {
                    seen.insert(stem.clone());
                    itch.push(OtherLinkRow {
                        mod_id: m.id.clone(),
                        name: m.name.clone(),
                        file_name: row.file_name.clone(),
                        installed_version: m.version.clone(),
                        latest_version: row.latest_version.clone(),
                        url,
                    });
                }
                _ => {}
            }
        }
        for (stem, m) in &by_stem {
            if seen.contains(stem) {
                continue;
            }
            if let Some(other) = self.mod_manager.other_row(stem) {
                if other.source_kind == "itch" {
                    itch.push(OtherLinkRow {
                        mod_id: m.id.clone(),
                        name: m.name.clone(),
                        file_name: other.file_name.clone(),
                        installed_version: m.version.clone(),
                        latest_version: None,
                        url: other.source_ref.clone(),
                    });
                }
            }
        }
        let by_name = |a: &OtherLinkRow, b: &OtherLinkRow| {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        };
        kingmods.sort_by(by_name);
        itch.sort_by(by_name);
        (kingmods, itch)
    }

    /// The Other Links tab (owner, 2026-10-24): the
    /// manual sources, KingMods and itch.io in their own
    /// separated sections. Each row prints the mod's
    /// actual page link as text; clicking it opens the
    /// page in the browser. Playground never downloads
    /// from either source — the file the user grabs
    /// there comes back through the row's Import, the
    /// same funnel the details panel's KingMods path
    /// has always used. ModHub and GitHub are untouched:
    /// they still check and offer installs as before.
    fn other_links_view(&self) -> Element<'_, Message> {
        let (kingmods, itch) = self.other_link_rows();
        let card = |r: &OtherLinkRow, import: Message| -> Element<'_, Message> {
            let version_line = match &r.latest_version {
                Some(latest) => format!(
                    "{} · installed v{} → latest v{}",
                    r.file_name, r.installed_version, latest
                ),
                None => format!(
                    "{} · installed v{}",
                    r.file_name, r.installed_version
                ),
            };
            let inner = column![
                text(r.name.clone()).size(14),
                text(version_line).size(12),
                row![
                    button(text(r.url.clone()).size(12))
                        .style(theme::nav_pill(false))
                        .on_press(Message::OpenUrl(r.url.clone())),
                    button(text("Import downloaded file"))
                        .style(theme::nav_pill(true))
                        .on_press(import),
                ]
                .spacing(6),
            ]
            .spacing(3);
            container(inner)
                .padding(8)
                .width(Length::Fill)
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
                .into()
        };
        let mut list = column![].spacing(8);
        list = list.push(
            text("KingMods and itch.io are manual sources: Playground never downloads from them. Click a link to open the mod's page, download the file there, then Import it on its row here.")
                .size(12),
        );
        list = list.push(
            text(format!(
                "KingMods — {} with an update waiting",
                kingmods.len()
            ))
            .size(15),
        );
        if kingmods.is_empty() {
            list = list.push(text("No KingMods updates are waiting.").size(12));
        }
        for r in &kingmods {
            list = list.push(card(
                r,
                Message::ImportKingModsFile(r.mod_id.clone()),
            ));
        }
        list = list.push(
            text(format!("itch.io — {} linked mods", itch.len())).size(15),
        );
        list = list.push(
            text("itch.io page reads are failing, so every itch.io mod is listed — click through to check for a newer version.")
                .size(12),
        );
        if itch.is_empty() {
            list = list.push(text("No itch.io mods are linked.").size(12));
        }
        for r in &itch {
            list = list.push(card(r, Message::ImportItchFile(r.mod_id.clone())));
        }
        page_scroll(list)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Settings (Round 2, owner 2026-10-22): the tabbed
    /// page modeled on Silo's tabs — General / Game Folders
    /// / Library / Accounts / Diagnostics / Maintenance /
    /// About — with collapsible menu-black boxes under the
    /// strip. Everything editable on every tab stages in
    /// the App and commits through the page's one explicit
    /// save (the footer row, visible on every tab); boxes a
    /// later build will fill carry the owner's placeholder
    /// text and no dead controls. The scroll keeps the
    /// Settings standard (page_scroll_pad 20).
    fn settings_view(&self) -> Element<'_, Message> {
        let tab_button = |tab: SettingsTab, label: &str| {
            button(text(label.to_string()))
                .style(theme::nav_pill(self.settings_tab == tab))
                .on_press(Message::SettingsTabSelected(tab))
        };
        let tabs = row![
            tab_button(SettingsTab::General, "General"),
            tab_button(SettingsTab::ModsFolder, "Game Folders"),
            tab_button(SettingsTab::Library, "Library"),
            tab_button(SettingsTab::Accounts, "Accounts"),
            tab_button(SettingsTab::Diagnostics, "Diagnostics"),
            tab_button(SettingsTab::Maintenance, "Maintenance"),
            tab_button(SettingsTab::About, "About"),
        ]
        .spacing(6);

        let content: Element<'_, Message> = match self.settings_tab {
            SettingsTab::General => self.settings_general_tab(),
            SettingsTab::ModsFolder => self.settings_mods_folder_tab(),
            SettingsTab::Library => self.settings_library_tab(),
            SettingsTab::Accounts => self.settings_accounts_tab(),
            SettingsTab::Diagnostics => self.settings_diagnostics_tab(),
            SettingsTab::Maintenance => self.settings_maintenance_tab(),
            SettingsTab::About => self.settings_about_tab(),
        };

        column![
            text("Settings").size(20),
            tabs,
            page_scroll_pad(content, PAGE_MARGIN + SCROLLBAR_W)
                .height(Length::Fill)
                .width(Length::Fill),
            // The footer row lives outside the scrollable
            // and shows on every tab: the one save for the
            // whole page, alone now (cosmetic round, John,
            // 2026-10-23) — the counts were redundant with
            // the sidebar rows and the Browse tiles (ST18),
            // Rescan moved into the Game Folders folders
            // area (ST16), and Save wears the save green on
            // the right (ST17). The same right pad aligns
            // its edge with the boxes'.
            row![
                Space::with_width(Length::Fill),
                button(text("Save settings"))
                    .style(theme::save_pill())
                    .on_press(Message::SaveDirectories),
            ]
            .spacing(8)
            .padding(iced::Padding {
                right: PAGE_MARGIN + SCROLLBAR_W,
                ..Default::default()
            }),
        ]
        .spacing(10)
        .into()
    }

    /// One collapsible box on the Settings page: a header
    /// row (a +/– affordance and the title) over the body,
    /// in the menu-black section style. The collapse state
    /// lives in the App, keyed by `id`.
    fn settings_box<'a>(
        &'a self,
        id: &'static str,
        title: &str,
        body: Vec<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        let collapsed = self.settings_collapsed.contains(id);
        let header = row![
            button(text(if collapsed { "+" } else { "–" }).size(14))
                .style(theme::nav_pill(false))
                .on_press(Message::SettingsToggleSection(id.to_string())),
            text(title.to_string()).size(15),
        ]
        .spacing(8);
        let mut col = column![header].spacing(7);
        if !collapsed {
            for el in body {
                col = col.push(el);
            }
        }
        container(col)
            .width(Length::Fill)
            .padding(11)
            .style(wizard_section_box)
            .into()
    }

    /// A folder row in the display-field pattern: the path
    /// as read-only text, an Open folder button (greyed
    /// when unset), and Change… through the folder picker.
    /// The staged input behind it commits with the page
    /// save, exactly as Round 1's fields did.
    fn settings_path_row<'a>(
        &'a self,
        label: &'a str,
        value: &'a str,
        target: FolderTarget,
    ) -> Element<'a, Message> {
        let shown = if value.is_empty() { "Not set" } else { value };
        let path = if value.is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        };
        column![
            text(label).size(13),
            row![
                text(shown.to_string()).width(Length::Fill),
                button(text("Open folder"))
                    .style(theme::nav_pill(true))
                    .on_press_maybe(path.map(Message::OpenFolder)),
                button(text("Change…"))
                    .style(theme::nav_pill(true))
                    .on_press(Message::BrowseFolder(target)),
            ]
            .spacing(8),
        ]
        .spacing(4)
        .into()
    }

    /// A box the page is not ready to fill yet: the
    /// owner's placeholder rule — the box exists under its
    /// real title with the placeholder text and no controls
    /// that pretend to work.
    fn settings_placeholder<'a>(
        &'a self,
        id: &'static str,
        title: &str,
    ) -> Element<'a, Message> {
        self.settings_box(
            id,
            title,
            vec![text("under construction").size(13).into()],
        )
    }

    /// General: start-up behaviour, the Launch
    /// Configuration, and the placeholder boxes.
    fn settings_general_tab(&self) -> Element<'_, Message> {
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
            .filter_map(|m| m.categories.first().cloned())
            .collect();
        cats.sort();
        cats.dedup();
        view_options.extend(cats.into_iter().map(BrowseView::Category));
        // Theme picker (themes, 2026-10-23; owner: "we
        // will need toggles in Settings for this"): one
        // row per theme — swatches from the CANDIDATE
        // palette, its name, its tag. Picking applies
        // live and saves at once (see ThemeSelected).
        let current_theme_id = theme::current_theme().id;
        // ST1 (cosmetic round, John, 2026-10-23): the five
        // themes as preview CARDS on ONE row — each card a
        // miniature of its own theme (base ground, panel
        // block, accent bar, its own text), the way the
        // palette board showed them when he picked the
        // five. Click applies live; the active card wears
        // its own accent as a heavier border.
        let mut cards = row![].spacing(8);
        for def in theme::THEMES {
            let p = def.palette;
            let selected = current_theme_id == def.id;
            let preview = container(
                column![
                    container(
                        column![
                            container(text(""))
                                .width(Length::Fill)
                                .height(6)
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(p.accent)),
                                    ..container::Style::default()
                                }),
                            text("Aa").size(15).color(p.text),
                            text("Text").size(10).color(p.text_dim),
                        ]
                        .spacing(3)
                        .padding(6),
                    )
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(p.panel)),
                        border: iced::Border {
                            radius: 5.0.into(),
                            ..iced::Border::default()
                        },
                        ..container::Style::default()
                    }),
                ]
                .spacing(4),
            )
            .width(Length::Fill)
            .padding(6)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(p.base)),
                border: iced::Border {
                    radius: 6.0.into(),
                    ..iced::Border::default()
                },
                ..container::Style::default()
            });
            let tag = match def.id {
                "gunmetal" => " (default)",
                "wildflower" | "lavender-dusk" => " (for women)",
                _ => "",
            };
            let card = button(
                column![
                    preview,
                    text(format!("{}{}", def.name, tag)).size(12),
                ]
                .spacing(5),
            )
            .width(Length::FillPortion(1))
            .padding(6)
            .style(move |_theme, _status| button::Style {
                background: Some(iced::Background::Color(theme::panel())),
                text_color: theme::text(),
                border: iced::Border {
                    color: if selected {
                        p.accent
                    } else {
                        theme::base()
                    },
                    width: if selected { 2.0 } else { 1.0 },
                    radius: 8.0.into(),
                },
                ..button::Style::default()
            })
            .on_press(Message::ThemeSelected(def.id.to_string()));
            cards = cards.push(card);
        }
        let theme_box = self.settings_box(
            "general-theme",
            "Theme",
            vec![
                text("Applies the moment you pick — no save, no restart. The Remove/Rescan red never changes: it is a meaning, not a fashion.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
                cards.into(),
            ],
        );

        let behavior = self.settings_box(
            "general-behavior",
            "Behavior",
            vec![
                row![
                    text("Browse opens showing:").size(13).width(220),
                    pick_list(
                        view_options,
                        Some(self.settings_browse_default.clone()),
                        Message::SettingsBrowseDefaultChanged,
                    ),
                ]
                .spacing(8)
                .into(),
                checkbox("Start maximized", self.settings_start_maximized)
                    .on_toggle(Message::SettingsStartMaximizedToggled)
                    .into(),
                text("Both apply from the next launch; saving never moves the window or the view in this one.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
            ],
        );

        column![
            theme_box,
            behavior,
            self.settings_placeholder("general-graphics", "Graphics"),
        ]
        .spacing(10)
        .into()
    }

    /// Game Folders: the game's folders as display
    /// rows. The Developer Console no longer lives here —
    /// it is staged under General > Launch Configuration.
    fn settings_mods_folder_tab(&self) -> Element<'_, Message> {
        let folders = self.settings_box(
            "folders-paths",
            "Folders",
            vec![
                self.settings_path_row(
                    "Game mods folder (Browse also points the game here via gameSettings.xml):",
                    &self.active_dir_input,
                    FolderTarget::Active,
                ),
                self.settings_path_row(
                    "Profile folder (holds game.xml):",
                    &self.game_dir_input,
                    FolderTarget::Game,
                ),
                self.settings_path_row(
                    "Game install folder (holds the shared folder):",
                    &self.game_install_dir_input,
                    FolderTarget::GameInstall,
                ),
                // Rescan lives with the folders it rescans
                // (cosmetic round ST16, John, 2026-10-23):
                // right side, under the Game install row,
                // still wearing the danger red. The spacer
                // matches its distance from the buttons above
                // to the distance between the box's other
                // button rows (John, live, 2026-10-23).
                Space::with_height(17).into(),
                row![
                    Space::with_width(Length::Fill),
                    button(text("Rescan folders"))
                        .style(theme::danger_pill())
                        .on_press(Message::Scan),
                ]
                .into(),
            ],
        );
        column![folders].spacing(10).into()
    }

    /// Library: the library folder row, the Layout
    /// summary, Round 1's catalog & sources surfaces
    /// re-homed unchanged, and Auto-Fill.
    fn settings_library_tab(&self) -> Element<'_, Message> {
        let library_folder = self.settings_box(
            "library-folder",
            "Library folder",
            vec![self.settings_path_row(
                "Library folder (stored mods):",
                &self.archive_dir_input,
                FolderTarget::Archive,
            )],
        );

        let layout = self.settings_box(
            "library-layout",
            "Layout",
            vec![
                text(format!(
                    "{} mods are filed under the library layout.",
                    self.mod_manager.organized_count()
                ))
                .size(13)
                .into(),
                row![button(text("Open Layout page"))
                    .style(theme::nav_pill(true))
                    .on_press(Message::TabSelected(Tab::Layout))]
                .into(),
                text("Organizing and rollback live on the Layout page; this is its standing count.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
            ],
        );

        // Round 1's Library box values ride along inside
        // Catalog & Sources (the owner's re-homing): the
        // conflict switch, the scan parallelism, and the
        // cache lifetime sit above the catalog fields.
        let mut source_rows: Vec<Element<'_, Message>> = vec![
            checkbox(
                "Check for conflicts",
                self.settings_check_conflicts,
            )
            .on_toggle(Message::SettingsCheckConflictsToggled)
            .into(),
            text("When off, conflicts are not computed and the Conflicts page shows none.")
                .size(12)
                .color(theme::text_dim())
                .into(),
            row![
                text("Parallel scans (1–16):").size(13).width(220),
                text_input("4", &self.settings_parallel_input)
                    .style(theme::input_field())
                    .on_input(Message::SettingsParallelScansChanged)
                    .padding(8)
                    .width(90),
            ]
            .spacing(8)
            .into(),
            row![
                text("Cache lifetime (hours, minimum 1):")
                    .size(13)
                    .width(220),
                text_input("24", &self.settings_cache_ttl_input)
                    .style(theme::input_field())
                    .on_input(Message::SettingsCacheTtlChanged)
                    .padding(8)
                    .width(90),
            ]
            .spacing(8)
            .into(),
            text("The cache lifetime covers the downloaded ModHub catalog and the decoded icon cache.")
                .size(12)
                .color(theme::text_dim())
                .into(),
            text("Catalog URL (the published ModHub catalog):")
                .size(13)
                .into(),
            text_input("Catalog URL", &self.settings_catalog_url_input)
                .style(theme::input_field())
                .on_input(Message::SettingsCatalogUrlChanged)
                .padding(8)
                .width(Length::Fill)
                .into(),
            text("Leave the catalog URL empty to turn catalog fetching off — a catalog already downloaded is still used from its cache.")
                .size(12)
                .color(theme::text_dim())
                .into(),
            text("Mod sources:").size(13).into(),
        ];
        for (list, heading, entries) in [
            (
                SourceList::ModHub,
                "ModHub",
                &self.settings_sources.mod_hub,
            ),
            (
                SourceList::GitHub,
                "GitHub",
                &self.settings_sources.github,
            ),
            (
                SourceList::KingMods,
                "KingMods",
                &self.settings_sources.kingmods,
            ),
        ] {
            source_rows.push(text(heading).size(13).color(theme::text_dim()).into());
            if entries.is_empty() {
                source_rows.push(
                    text("No sources configured.").size(12).into(),
                );
            }
            for (index, source) in entries.iter().enumerate() {
                source_rows.push(
                    row![
                        checkbox(source.name.clone(), source.enabled)
                            .on_toggle(move |on| {
                                Message::SettingsSourceToggled(list, index, on)
                            })
                            .width(220),
                        text(source.url.clone())
                            .size(12)
                            .color(theme::text_dim()),
                    ]
                    .spacing(8)
                    .into(),
                );
            }
        }
        source_rows.push(
            text("Source names and URLs are shown as configured; only their on/off switches are editable here.")
                .size(12)
                .color(theme::text_dim())
                .into(),
        );
        let catalog = self.settings_box(
            "library-catalog",
            "Catalog & Sources",
            source_rows,
        );

        let auto_fill = self.settings_box(
            "library-autofill",
            "Auto-Fill",
            vec![
                checkbox(
                    "Auto-Fill: file new mods into the library layout automatically",
                    self.settings_auto_file,
                )
                .on_toggle(Message::SettingsAutoFileToggled)
                .into(),
                text("When a download or import finishes and the library layout is active, the mod is filed into its category folder and linked into the game folder. With no layout in place, finished mods simply stay where they landed.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
            ],
        );

        // The Developer Console stages like every other
        // value on the page now; its three states mirror
        // Round 1's row (switch / no switch in game.xml /
        // no game folder yet).
        let console_row: Element<'_, Message> =
            match (self.mod_manager.game_directory(), self.console_on) {
                (Some(_), Some(on)) => checkbox(
                    "Developer console (written to game.xml when the page saves)",
                    on,
                )
                .on_toggle(|on| {
                    Message::SettingsLaunchToggled(LaunchSetting::Console, on)
                })
                .into(),
                (Some(_), None) => {
                    text("Developer console: no game.xml console switch found in the profile folder.")
                        .size(13)
                        .into()
                }
                (None, _) => {
                    text("Developer console: set the profile folder (Game Folders tab) to manage the console switch in game.xml.")
                        .size(13)
                        .into()
                }
            };
        // The preview reads the staged values, the slot
        // from its field (an unparseable slot previews as
        // unset; the save itself refuses it).
        let slot = self
            .settings_launch_slot_input
            .trim()
            .parse::<u8>()
            .ok()
            .filter(|n| (1..=20).contains(n));
        let preview = crate::config::launch_command_preview(
            &crate::config::LaunchConfig {
                slot,
                ..self.settings_launch.clone()
            },
        );
        let launch = self.settings_box(
            "general-launch",
            "Launch Configuration",
            vec![
                console_row,
                row![
                    text("Savegame slot (1–20, empty = unset):")
                        .size(13)
                        .width(300),
                    text_input("", &self.settings_launch_slot_input)
                        .style(theme::input_field())
                        .on_input(Message::SettingsLaunchSlotChanged)
                        .padding(8)
                        .width(90),
                ]
                .spacing(8)
                .into(),
                checkbox(
                    "Skip mod update dialog",
                    self.settings_launch.skip_mod_update_dialog,
                )
                .on_toggle(|on| Message::SettingsLaunchToggled(
                    LaunchSetting::SkipModUpdateDialog,
                    on,
                ))
                .into(),
                checkbox("Enable cheats", self.settings_launch.cheats)
                    .on_toggle(|on| Message::SettingsLaunchToggled(
                        LaunchSetting::Cheats,
                        on,
                    ))
                    .into(),
                checkbox("Auto-start the savegame", self.settings_launch.autostart)
                    .on_toggle(|on| Message::SettingsLaunchToggled(
                        LaunchSetting::AutoStart,
                        on,
                    ))
                    .into(),
                checkbox(
                    "Skip start videos",
                    self.settings_launch.skip_start_videos,
                )
                .on_toggle(|on| Message::SettingsLaunchToggled(
                    LaunchSetting::SkipStartVideos,
                    on,
                ))
                .into(),
                text("Launch command preview:").size(13).into(),
                row![
                    text(preview).size(13).color(theme::text_dim()),
                    button(text("Copy flags for Steam"))
                        .style(theme::nav_pill(true))
                        .on_press(Message::CopyLaunchFlags),
                ]
                .spacing(12)
                .into(),
                text("Playground never runs this command. The flags are yours to paste into Steam's Launch Options for FS25 (right-click the game, Properties, General, Launch Options). The Developer Console acts through game.xml when the page saves.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
            ],
        );

        column![auto_fill, launch, catalog, layout, library_folder]
            .spacing(10)
            .into()
    }

    /// Accounts: the GitHub connection state, read-only
    /// this round, and the Sharing placeholder.
    fn settings_accounts_tab(&self) -> Element<'_, Message> {
        let github = self.settings_box(
            "accounts-github",
            "GitHub",
            vec![
                text(if self.mod_manager.github_connected() {
                    "Connected — GitHub updates use the token Playground already holds (the ModHub source's API key, or the GITHUB_TOKEN / GH_TOKEN environment variable)."
                } else {
                    "Not connected — no GitHub token is available to Playground."
                })
                .size(13)
                .into(),
                text("Read-only this round: there are no connect or disconnect controls yet.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
            ],
        );
        column![github]
        .spacing(10)
        .into()
    }

    /// Diagnostics: the scrubbed summary with its three
    /// actions, and the Log Reader's opening defaults.
    fn settings_diagnostics_tab(&self) -> Element<'_, Message> {
        let summary = self.settings_box(
            "diagnostics-summary",
            "Summary",
            vec![
                text(self.diagnostics_summary()).size(13).into(),
                row![
                    button(text("Copy diagnostics"))
                        .style(theme::nav_pill(true))
                        .on_press(Message::CopyDiagnostics),
                    button(text("Export report"))
                        .style(theme::nav_pill(true))
                        .on_press(Message::ExportDiagnostics),
                    button(text("Open log folder"))
                        .style(theme::nav_pill(true))
                        .on_press_maybe(
                            crate::update_db::logs_dir()
                                .map(Message::OpenFolder),
                        ),
                ]
                .spacing(8)
                .into(),
                text("Paths in this summary have your user folder name masked — it is text you may paste somewhere public.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
            ],
        );
        let log_reader = self.settings_box(
            "diagnostics-log-reader",
            "Log Reader",
            vec![
                checkbox(
                    "Open with errors and warnings only",
                    self.settings_log_errors_only_default,
                )
                .on_toggle(Message::SettingsLogErrorsOnlyDefaultToggled)
                .into(),
                row![
                    text("Log lines kept (the last N):").size(13).width(220),
                    text_input("3000", &self.settings_log_lines_cap_input)
                        .style(theme::input_field())
                        .on_input(Message::SettingsLogLinesCapChanged)
                        .padding(8)
                        .width(90),
                ]
                .spacing(8)
                .into(),
                text("Applied when the Log Reader next reads the log.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
            ],
        );
        column![summary, log_reader].spacing(10).into()
    }

    /// Maintenance: the from-scratch re-read, the
    /// Downloads history switch, and the fill log.
    fn settings_maintenance_tab(&self) -> Element<'_, Message> {
        let reread = self.settings_box(
            "maintenance-reread",
            "Library database",
            vec![
                text("Re-reading clears the library database's stored fingerprints, so the next scan parses every mod from its file again — the slow, certain way to settle a record that looks wrong.")
                    .size(13)
                    .into(),
                row![button(text("Re-read every mod"))
                    .style(theme::nav_pill(true))
                    .on_press(Message::RereadEveryMod)]
                .into(),
            ],
        );
        let downloads = self.settings_box(
            "maintenance-downloads",
            "Downloads",
            vec![
                checkbox(
                    "Keep download history between sessions",
                    self.settings_keep_history,
                )
                .on_toggle(Message::SettingsKeepHistoryToggled)
                .into(),
                text("When on, the Downloads page's finished and failed rows are stored in the data folder and come back at the next launch. When off, history lasts for the session only; a stored history is left untouched, just not loaded.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
            ],
        );
        let fill_log = self.settings_box(
            "maintenance-fill-log",
            "Fill log",
            vec![
                text(format!(
                    "Fill logs folder: {}",
                    crate::update_db::logs_dir()
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                ))
                .size(13)
                .into(),
                row![
                    text("Fill logs to keep:").size(13).width(220),
                    text_input("10", &self.settings_fill_log_keep_input)
                        .style(theme::input_field())
                        .on_input(Message::SettingsFillLogKeepChanged)
                        .padding(8)
                        .width(90),
                ]
                .spacing(8)
                .into(),
                text("Each scheduled fill writes fill-<date>-<time>.log there: its start time, the per-source results, the updates found, and any error. Older logs are pruned to the keep count.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
            ],
        );
        let fill_line = match self.fill_task_registered {
            Some(true) => "Scheduled fill: the Windows task \"Playground Update Fill\" is registered.",
            Some(false) => {
                "Scheduled fill: the Windows task \"Playground Update Fill\" is not registered."
            }
            None => {
                if cfg!(windows) {
                    "Scheduled fill: registration could not be checked here."
                } else {
                    "Scheduled fill: no built-in schedule on this platform yet — the fill also runs at launch and on Rescan."
                }
            }
        };
        let updates = self.settings_box(
            "general-updates",
            "Updates",
            vec![
                checkbox(
                    "Check for updates at start-up",
                    self.settings_startup_check,
                )
                .on_toggle(Message::SettingsStartupCheckToggled)
                .into(),
                text("Runs the update check once when Playground starts. The Rescan button always runs it too.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
                row![
                    checkbox(
                        "Playground self-update",
                        self.settings_self_update,
                    )
                    .on_toggle(Message::SettingsSelfUpdateToggled),
                    button(text("Check now"))
                        .style(theme::nav_pill(true))
                        .on_press(Message::CheckAppUpdateNow),
                ]
                .spacing(8)
                .into(),
                text("When on, a newer published Playground is offered at start-up. Check now runs the same check right away.")
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
                text(fill_line).size(13).into(),
                text(if cfg!(windows) {
                    "The scheduled catalog fill (update.exe) runs daily at 06:00, 14:00 and 22:00."
                } else {
                    "On Linux the fill helper is named update. There is no built-in schedule yet — run it from cron or a systemd timer if you want it automatic."
                })
                    .size(12)
                    .color(theme::text_dim())
                    .into(),
            ],
        );

        column![updates, reread, downloads, fill_log]
            .spacing(10)
            .into()
    }

    /// About: the version, the two links, and the credit.
    fn settings_about_tab(&self) -> Element<'_, Message> {
        // ST14 (cosmetic round, John, 2026-10-23): the
        // About tab is a SINGLE CARD holding exactly four
        // things, in this order — the logo, the links
        // (website, then GitHub), the version line, and
        // the copyright line. Nothing else rides the card.
        let card = container(
            column![
                iced::widget::image(iced::widget::image::Handle::from_bytes(
                    include_bytes!("../../assets/logo.png").to_vec(),
                ))
                .filter_method(iced::widget::image::FilterMethod::Linear)
                .width(480)
                .height(480),
                row![
                    button(text("farmerjay.dev"))
                        .style(theme::nav_pill(true))
                        .on_press(Message::OpenUrl(
                            "https://farmerjay.dev".to_string(),
                        )),
                    button(text("GitHub — Jmilburn1/Playground"))
                        .style(theme::nav_pill(true))
                        .on_press(Message::OpenUrl(
                            "https://github.com/Jmilburn1/Playground"
                                .to_string(),
                        )),
                ]
                .spacing(8),
                text(format!(
                    "Playground v{}",
                    env!("CARGO_PKG_VERSION")
                ))
                .size(14),
                text("© 2026 Farmer Jay.").size(13),
            ]
            .spacing(10)
            .align_x(iced::alignment::Alignment::Center),
        )
        .width(Length::Fill)
        .align_x(iced::alignment::Alignment::Center)
        .padding(14)
        .style(wizard_section_box);
        column![card].spacing(10).into()
    }

    /// The Library Layout page (Roadmap #2, Phase 4):
    /// the standing state read from the executor's own
    /// record, a read-only plan preview, the destructive
    /// step stated in plain words over an arm-to-confirm
    /// Apply, progress that ticks while a run lands, and
    /// the rollback button while its window is open.
    fn layout_view(&self) -> Element<'_, Message> {
        let running = self.layout_running;
        let busy = running.is_some();
        let lib = self.mod_manager.get_archive_dir().clone();
        let game = self.mod_manager.get_active_dir().clone();
        let mut col = column![
            text("Library Layout").size(20),
            text("One copy of every mod lives in the library, sorted into category folders. The game mods folder holds links to the active mods — never second copies.")
                .size(13)
                .width(Length::Fill),
        ]
        .spacing(10);

        // ---- Standing state, from the record ----------
        let standing = self.layout_standing.clone();
        match &standing {
            None => {
                col = col.push(text("Standing: the migration record has not been read yet.").size(13));
            }
            Some(None) => {
                col = col.push(
                    text("Standing: not migrated. No Playground layout migration is recorded — the library is in its everyday shape.")
                        .size(13)
                        .width(Length::Fill),
                );
            }
            Some(Some(state)) => {
                let line = match state.status {
                    MigrationStatus::InProgress => format!(
                        "Standing: a migration started {} is recorded as still in progress — it was interrupted mid-run. Roll it back below to restore the pre-migration world before applying again.",
                        state.started_at
                    ),
                    MigrationStatus::FailedReversed => format!(
                        "Standing: the migration started {} FAILED and was reversed — the library and the game folder are as they were. Run log: {}",
                        state.started_at,
                        state.log_file.display()
                    ),
                    MigrationStatus::Swapped => {
                        if state.backup_root.exists() {
                            format!(
                                "Standing: migrated ({}). The rollback window is OPEN — the pre-layout game folder is kept whole at {}.",
                                state.started_at,
                                state.backup_root.display()
                            )
                        } else {
                            format!(
                                "Standing: migrated ({}). The backup {} is gone, so the rollback window is closed — the layout is final.",
                                state.started_at,
                                state.backup_root.display()
                            )
                        }
                    }
                };
                col = col.push(text(line).size(13).width(Length::Fill));
            }
        }

        // ---- Plan preview ------------------------------
        match &self.layout_preview {
            None => {
                let mut b = button(text("Build plan")).style(theme::nav_pill(true));
                if !busy && !self.loading {
                    b = b.on_press(Message::LayoutBuildPlan);
                }
                col = col.push(b);
                col = col.push(
                    text("Building a plan is read-only: it censuses the library and the game mods folder and shows the moves and links a migration would make. Nothing is touched.")
                        .size(12)
                        .width(Length::Fill),
                );
            }
            Some(plan) => {
                let s = &plan.summary;
                col = col.push(
                    text(format!(
                        "Plan — library: {} · game mods folder: {}",
                        plan.library_root.display(),
                        plan.game_root.display()
                    ))
                    .size(13)
                    .width(Length::Fill),
                );
                col = col.push(
                    text(format!(
                        "{} mods · {} to move · {} hardlinks + {} junctions to create · {} already linked · {} already in place · {} total",
                        s.mods,
                        s.moves,
                        s.hardlinks,
                        s.junctions,
                        s.links_satisfied,
                        s.already_in_place,
                        format_size(s.total_bytes)
                    ))
                    .size(13)
                    .width(Length::Fill),
                );
                let mut cats = column![].spacing(1);
                for (name, count) in &s.per_category {
                    cats = cats.push(text(format!("  {}: {}", name, count)).size(12));
                }
                col = col.push(cats);
                if !plan.warnings.is_empty() {
                    let mut warns = column![text(format!("Warnings ({}):", plan.warnings.len())).size(13)].spacing(1);
                    for w in &plan.warnings {
                        let kind = match w.kind {
                            layout::WarningKind::NoLibraryCopy => "no library copy",
                            layout::WarningKind::TargetCollision => "target collision",
                            layout::WarningKind::CategorySanitized => "category sanitized",
                        };
                        warns = warns.push(
                            text(format!("  [{}] {}: {}", kind, w.file_name, w.detail))
                                .size(12)
                                .width(Length::Fill),
                        );
                    }
                    col = col.push(warns);
                }
                let mut rebuild = button(text("Rebuild plan")).style(theme::nav_pill(true));
                if !busy && !self.loading {
                    rebuild = rebuild.on_press(Message::LayoutBuildPlan);
                }
                col = col.push(rebuild);

                let stale = plan.library_root != lib || plan.game_root != game;
                let plan_empty = s.moves == 0 && s.hardlinks == 0 && s.junctions == 0;
                if stale {
                    col = col.push(
                        text("This plan was built for different folders than the ones now configured — it no longer applies. Build a fresh plan.")
                            .size(13)
                            .width(Length::Fill),
                    );
                } else if plan_empty {
                    col = col.push(
                        text("This plan is empty: the library is already in layout form and the game mods folder is fully projected. Nothing to apply.")
                            .size(13)
                            .width(Length::Fill),
                    );
                } else if running != Some(LayoutRun::Apply) {
                    // ---- The destructive step, in plain
                    // words, over the arm-to-confirm button.
                    col = col.push(
                        text(format!(
                            "What Apply will do — read it before you approve: rename {} library mod(s) into their category subfolders; build a staging farm holding {} projection(s); verify that every projection reads through to its library copy; then rename the game mods folder to a timestamped backup and put the farm in its place. The game folder's real copies are NOT deleted — they stay whole in the backup, and the rollback window stays open until you delete the backup yourself. Anything else sitting in the game folder rides into the backup with them and will not be in the new game folder.",
                            s.moves,
                            plan.links.len()
                        ))
                        .size(13)
                        .width(Length::Fill),
                    );
                    if let Some(Some(state)) = &standing {
                        let blocked = state.status == MigrationStatus::InProgress
                            || (state.status == MigrationStatus::Swapped
                                && state.backup_root.exists());
                        if blocked {
                            col = col.push(
                                text("Note: a previous migration's rollback window is still open (see Standing above). The executor will refuse a new apply until that migration is rolled back, or its backup is deleted by hand.")
                                    .size(12)
                                    .width(Length::Fill),
                            );
                        }
                    }
                    let apply_btn = if self.layout_arm_apply {
                        button(centered_label(
                            text(format!(
                                "Click again to apply: {} moves, {} links",
                                s.moves,
                                s.hardlinks + s.junctions
                            ))
                            .size(14),
                        ))
                        .width(Length::Fill)
                        .style(theme::danger_pill())
                        .on_press(Message::LayoutApply)
                    } else {
                        button(centered_label(text("Apply this layout").size(14)))
                            .width(Length::Fill)
                            .style(theme::nav_pill(false))
                            .on_press(Message::LayoutApply)
                    };
                    col = col.push(apply_btn);
                }
            }
        }

        // ---- Progress while a run lands -----------------
        if running == Some(LayoutRun::Apply) {
            let frac = if self.layout_total > 0 {
                self.eased_bar(
                    "layout",
                    self.layout_done as f32 / self.layout_total as f32,
                )
            } else {
                // No total yet: the bar breathes instead
                // of sitting frozen at zero (feel pass).
                self.pulse_frac()
            };
            col = col.push(progress_bar(0.0..=1.0, frac).width(Length::Fill));
            col = col.push(
                text(format!(
                    "Applying: {} of {} operations landed…",
                    self.layout_done, self.layout_total
                ))
                .size(13),
            );
        }
        if running == Some(LayoutRun::Rollback) {
            col = col.push(text("Rolling back…").size(13));
        }

        // ---- Rollback, while its window is open ---------
        if let Some(Some(state)) = &standing {
            let window_open = match state.status {
                MigrationStatus::InProgress | MigrationStatus::FailedReversed => true,
                MigrationStatus::Swapped => state.backup_root.exists(),
            };
            if window_open && running != Some(LayoutRun::Rollback) {
                col = col.push(
                    text("Rollback removes the projection farm, puts the backed-up game folder back in place, and renames the sorted library copies back where they were. If the game folder holds anything that is not a projection of the library, rollback refuses and removes nothing.")
                        .size(13)
                        .width(Length::Fill),
                );
                let (label, armed_label) = if state.status == MigrationStatus::FailedReversed {
                    ("Clear the failed migration's record", "Click again to clear the record")
                } else {
                    ("Roll back the migration", "Click again to roll back")
                };
                let rb_btn = if self.layout_arm_rollback {
                    button(centered_label(text(armed_label).size(14)))
                        .width(Length::Fill)
                        .style(theme::danger_pill())
                        .on_press(Message::LayoutRollback)
                } else {
                    button(centered_label(text(label).size(14)))
                        .width(Length::Fill)
                        .style(theme::nav_pill(false))
                        .on_press(Message::LayoutRollback)
                };
                col = col.push(rb_btn);
            }
        }

        // ---- Reclaim disk space -------------------------
        col = col.push(text("Reclaim disk space").size(15));
        col = col.push(
            text("Two pools: backups — every backup family the app keeps is pruned to its newest entry — and exact duplicate mods (same file name, same version), reduced to one copy. Measuring is read-only; nothing is deleted until the armed Reclaim button is pressed.")
                .size(12)
                .width(Length::Fill),
        );
        match &self.reclaim {
            None => {
                let mut b = button(text("Measure reclaimable space")).style(theme::nav_pill(true));
                if !busy && !self.loading {
                    b = b.on_press(Message::ReclaimMeasure);
                }
                col = col.push(b);
            }
            Some(census) => {
                for family in &census.families {
                    let prunable = family.prunable().len();
                    let line = if prunable == 0 {
                        format!(
                            "{}: {} — nothing older to prune.",
                            family.label,
                            if family.entries.len() == 1 {
                                "1 backup kept".to_string()
                            } else {
                                format!("{} backups kept", family.entries.len())
                            }
                        )
                    } else {
                        let mut line = format!(
                            "{}: {} old copie(s) beyond the newest — {} prunable.",
                            family.label,
                            prunable,
                            format_size(family.prunable_bytes())
                        );
                        if family.entries.len() - 1 - prunable > 0 {
                            line.push_str(" The recorded rollback backup is kept too.");
                        }
                        line
                    };
                    col = col.push(text(line).size(13).width(Length::Fill));
                }
                let dup_copies = census.duplicate_copies();
                let dup_line = if dup_copies == 0 {
                    "Duplicate mods: none — every file name and version has exactly one copy."
                        .to_string()
                } else {
                    format!(
                        "Duplicate mods: {} extra copie(s) across {} set(s) — {}.",
                        dup_copies,
                        census.dup_sets.len(),
                        format_size(census.duplicate_bytes())
                    )
                };
                col = col.push(text(dup_line).size(13).width(Length::Fill));
                if census.is_empty() {
                    col = col.push(text("Nothing to reclaim.").size(13));
                } else {
                    col = col.push(
                        text(format!(
                            "What Reclaim will do — read it before you approve: permanently delete {} old backup(s) ({}) and {} duplicate mod copie(s) ({}). The newest backup in every family stays, and one copy of every duplicate stays — the Active copy where there is one. This cannot be undone from inside Playground.",
                            census.prunable_backups(),
                            format_size(census.prunable_backup_bytes()),
                            dup_copies,
                            format_size(census.duplicate_bytes())
                        ))
                        .size(13)
                        .width(Length::Fill),
                    );
                    let reclaim_btn = if self.reclaim_arm {
                        button(centered_label(
                            text(format!(
                                "Click again to reclaim: {} backup(s), {} copie(s) — {}",
                                census.prunable_backups(),
                                dup_copies,
                                format_size(census.total_bytes())
                            ))
                            .size(14),
                        ))
                        .width(Length::Fill)
                        .style(theme::danger_pill())
                        .on_press(Message::ReclaimRun)
                    } else {
                        button(centered_label(text("Reclaim this space").size(14)))
                            .width(Length::Fill)
                            .style(theme::nav_pill(false))
                            .on_press(Message::ReclaimRun)
                    };
                    col = col.push(reclaim_btn);
                }
                let mut again = button(text("Re-measure")).style(theme::nav_pill(true));
                if !busy && !self.loading {
                    again = again.on_press(Message::ReclaimMeasure);
                }
                col = col.push(again);
            }
        }
        if let Some(outcome) = &self.reclaim_outcome {
            col = col.push(text(outcome.clone()).size(13).width(Length::Fill));
        }

        // ---- The last run's result -----------------------
        if let Some(outcome) = &self.layout_outcome {
            col = col.push(text(outcome.clone()).size(13).width(Length::Fill));
        }
        if let Some(error) = &self.layout_error {
            col = col.push(
                text(error.clone())
                    .size(13)
                    .width(Length::Fill)
                    .color(iced::Color::from_rgb(0.96, 0.46, 0.40)),
            );
        }

        page_scroll(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Keep the feel-pass animation clock alive for a
    /// tail (ANIM_TAIL) — long enough for every glide,
    /// roll and ease kicked by the same event to settle.
    fn kick_anim(&mut self) {
        self.anim_until = std::time::Instant::now() + ANIM_TAIL;
    }

    /// The numbers the rolling counts roll toward
    /// (feel pass, 2026-10-23): the sidebar library
    /// rows, the two counted nav labels, and the Browse
    /// stat tiles — all read from live state.
    fn count_targets(&self) -> [(&'static str, usize); 8] {
        let all = self.mod_manager.get_mods();
        let cat = |name: &str| {
            all.iter()
                .filter(|m| m.categories.first().map_or(false, |c| c == name))
                .count()
        };
        [
            ("mods", all.len()),
            ("favs", self.mod_manager.favorites_count()),
            ("maps", cat("Maps")),
            ("scripts", cat("Scripts")),
            (
                "active",
                all.iter().filter(|m| m.state == ModState::Active).count(),
            ),
            (
                "archived",
                all.iter().filter(|m| m.state == ModState::Archived).count(),
            ),
            ("conflicts", self.conflicts.len()),
            ("downloads", self.downloads.live_count()),
        ]
    }

    /// Point every rolling count at its current target
    /// and keep the clock alive while they travel.
    fn retarget_counts(&mut self) {
        for (key, target) in self.count_targets() {
            match self.shown_counts.get_mut(key) {
                Some((_, t)) => *t = target,
                None => {
                    self.shown_counts.insert(key, (target as f32, target));
                }
            }
        }
        self.kick_anim();
    }

    /// The number a rolling count shows right now. If
    /// the stored target disagrees with the live actual
    /// (a change that reached the view without a
    /// retarget), the truth wins and the count snaps —
    /// a displayed number is never wrong, only
    /// sometimes unrolled.
    fn shown_count(&self, key: &'static str, actual: usize) -> usize {
        match self.shown_counts.get(key) {
            Some(&(shown, target)) if target == actual => shown.round() as usize,
            _ => actual,
        }
    }

    /// A progress bar's displayed value: the eased one
    /// when the animation clock has one stored, the
    /// actual otherwise.
    fn eased_bar(&self, key: &str, actual: f32) -> f32 {
        self.eased_bars.get(key).copied().unwrap_or(actual)
    }

    /// Step one eased bar toward its actual (AnimTick).
    fn step_bar(&mut self, key: &str, actual: f32) {
        let entry = self.eased_bars.entry(key.to_string()).or_insert(actual);
        let diff = actual - *entry;
        if diff.abs() < 0.004 {
            *entry = actual;
        } else {
            *entry += diff * 0.35;
        }
    }

    /// A card switch's knob position right now: mid-
    /// glide it eases (cubic out) between the anim's
    /// ends; at rest it sits exactly on the state's end.
    fn switch_t(&self, id: &str, on: bool) -> f32 {
        match self.switch_anims.get(id) {
            Some(a) => {
                let raw = (a.started.elapsed().as_secs_f32()
                    / SWITCH_ANIM_DUR.as_secs_f32())
                .min(1.0);
                let eased = 1.0 - (1.0 - raw).powi(3);
                a.from + (a.to - a.from) * eased
            }
            None => {
                if on {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    /// The indeterminate progress pulse (feel pass): a
    /// slow breath between 15% and 85%, phased off the
    /// animation epoch — what a running job with no
    /// known total shows instead of a frozen zero bar.
    fn pulse_frac(&self) -> f32 {
        let secs = self.anim_epoch.elapsed().as_secs_f32();
        0.5 - 0.35 * (secs * std::f32::consts::TAU / 1.8).cos()
    }

    /// How far a just-opened surface still has to
    /// settle (feel pass part 2): 6px at the open,
    /// easing out to nothing over ~160ms.
    fn settle_offset(&self, opened_at: std::time::Instant) -> f32 {
        let raw = (opened_at.elapsed().as_secs_f32() / 0.16).min(1.0);
        6.0 * (1.0 - raw) * (1.0 - raw)
    }

    /// The status line's fade-in alpha (feel pass
    /// part 2): a new status fades in over ~220ms and
    /// never starts from fully invisible.
    fn status_alpha(&self) -> f32 {
        let raw = (self.status_changed_at.elapsed().as_secs_f32() / 0.22).min(1.0);
        0.15 + 0.85 * raw
    }

    /// The Reclaim census over the app's two pools
    /// (owner ruling, 2026-10-23): backups beyond each
    /// family's newest, and exact duplicate copies. The
    /// recorded migration's backup is protected wherever
    /// it sits in its family.
    fn build_reclaim_census(&self) -> crate::mod_manager::reclaim::ReclaimCensus {
        use crate::mod_manager::reclaim;
        let profile = self.mod_manager.profile_dir();
        let protected = match &self.layout_standing {
            Some(Some(state)) => Some(state.backup_root.clone()),
            _ => None,
        };
        let families = reclaim::census_backups(
            profile.as_deref(),
            crate::paths::data_dir().as_deref(),
            self.mod_manager.get_active_dir(),
            protected.as_deref(),
        );
        let mods = self.mod_manager.get_mods();
        let dup_sets = reclaim::census_duplicates(mods, self.mod_manager.get_archive_dir());
        reclaim::ReclaimCensus {
            families,
            dup_sets,
        }
    }

    /// Execute the armed reclaim: prune the families on
    /// the filesystem, remove the duplicate copies
    /// through the manager (so the library database,
    /// the icons and the conflicts all stay true), and
    /// write the run's record to the logs folder.
    fn execute_reclaim(&mut self) {
        use crate::mod_manager::reclaim;
        let Some(census) = self.reclaim.clone() else {
            return;
        };
        let mut log: Vec<String> = vec![format!(
            "Playground reclaim — {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        )];
        let mut errors: Vec<String> = Vec::new();
        let mut backups_removed = 0usize;
        let mut backup_bytes = 0u64;
        for family in &census.families {
            let outcome = reclaim::prune_family(family);
            backups_removed += outcome.removed;
            backup_bytes += outcome.bytes;
            log.push(format!(
                "[backups] {}: removed {}, freed {}",
                family.label,
                outcome.removed,
                format_size(outcome.bytes)
            ));
            for e in &outcome.errors {
                log.push(format!("[backups] error: {}", e));
            }
            errors.extend(outcome.errors);
        }
        let mut dups_removed = 0usize;
        let mut dup_bytes = 0u64;
        for set in &census.dup_sets {
            for (id, path, size) in &set.remove {
                match self.mod_manager.remove_mod(id) {
                    Ok(()) => {
                        dups_removed += 1;
                        dup_bytes += size;
                        self.icons.remove(id);
                        if self.selected_mod.as_deref() == Some(id.as_str()) {
                            self.selected_mod = None;
                        }
                        log.push(format!(
                            "[duplicate] removed {} (kept {})",
                            path.display(),
                            set.keep_path.display()
                        ));
                    }
                    Err(e) => {
                        let msg = format!("{}: {}", path.display(), e);
                        log.push(format!("[duplicate] error: {}", msg));
                        errors.push(msg);
                    }
                }
            }
        }
        if dups_removed > 0 {
            self.recompute_conflicts();
        }
        let total = backup_bytes + dup_bytes;
        log.push(format!(
            "[total] {} backup(s), {} duplicate copie(s), {} freed, {} error(s)",
            backups_removed,
            dups_removed,
            format_size(total),
            errors.len()
        ));
        if let Some(dir) = crate::paths::data_dir() {
            let logs = dir.join("logs");
            let _ = std::fs::create_dir_all(&logs);
            let name = format!(
                "reclaim-{}.log",
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            );
            let _ = std::fs::write(logs.join(name), log.join("\n"));
        }
        let mut outcome = format!(
            "Reclaimed {}: deleted {} old backup(s) and {} duplicate copie(s).",
            format_size(total),
            backups_removed,
            dups_removed
        );
        if !errors.is_empty() {
            outcome.push_str(&format!(
                " {} could not be deleted — see the newest reclaim log in the logs folder.",
                errors.len()
            ));
        }
        self.status = outcome.clone();
        self.reclaim_outcome = Some(outcome);
        // Re-measure so the section tells the new truth.
        self.reclaim = Some(self.build_reclaim_census());
    }

    /// (Re)load the Keybindings page's staged document
    /// from the profile's inputBinding.xml. A missing file
    /// is the friendly empty state, never an error; a read
    /// failure says so in the status line.
    fn kb_reload(&mut self) {
        self.kb_capture = None;
        self.kb_reset_armed = false;
        self.kb_purge = None;
        let profile = self.mod_manager.profile_dir();
        self.kb_profile = profile.clone();
        self.kb_backups = profile
            .as_ref()
            .map(|p| keybindings::list_backups(p))
            .unwrap_or_default();
        let Some(profile) = profile else {
            self.kb_doc = None;
            self.kb_original = None;
            return;
        };
        let path = keybindings::bindings_path(&profile);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let doc = BindingsFile::parse(text);
                // Stay on the device the user is viewing
                // when it still exists; otherwise the
                // keyboard tab when present, else the
                // file's first device.
                let devices = doc.devices();
                let keep = self
                    .kb_device
                    .clone()
                    .filter(|d| devices.contains(d));
                self.kb_device = keep.or_else(|| {
                    if devices
                        .iter()
                        .any(|d| d == keybindings::KEYBOARD_DEVICE)
                    {
                        Some(keybindings::KEYBOARD_DEVICE.to_string())
                    } else {
                        devices.first().cloned()
                    }
                });
                self.kb_original = Some(doc.text().to_string());
                self.kb_doc = Some(doc);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.kb_doc = None;
                self.kb_original = None;
            }
            Err(e) => {
                self.kb_doc = None;
                self.kb_original = None;
                self.status =
                    format!("Could not read {}: {}", path.display(), e);
            }
        }
    }

    /// The staged document's actions grouped for the
    /// page: Base Game first (actions no loaded mod
    /// claims), then one group per owning mod, A-Z.
    /// Actions keep file order inside a group. Unless
    /// the Show archived mods toggle is on, archived
    /// mods' actions are excluded before grouping (see
    /// `keybindings::visible_actions`) — the document
    /// itself always holds every block.
    fn kb_grouped_actions(&self) -> Vec<(String, Vec<KbAction>)> {
        let Some(doc) = &self.kb_doc else {
            return Vec::new();
        };
        let all = doc.actions();
        let owners = self.kb_owners(&all);
        let visible =
            keybindings::visible_actions(&all, &owners, self.kb_show_archived);
        let mut base: Vec<KbAction> = Vec::new();
        let mut by_mod: std::collections::BTreeMap<String, Vec<KbAction>> =
            std::collections::BTreeMap::new();
        for action in visible {
            match owners.get(&action.name) {
                Some(owner) => by_mod
                    .entry(owner.title.clone())
                    .or_default()
                    .push(action),
                None => base.push(action),
            }
        }
        let mut out = vec![("Base Game".to_string(), base)];
        out.extend(by_mod.into_iter());
        out
    }

    /// The ownership map for a set of actions, from the
    /// manager's action -> owner join. Actions no loaded
    /// mod claims are simply absent (Base Game).
    fn kb_owners(
        &self,
        actions: &[KbAction],
    ) -> HashMap<String, keybindings::ActionOwner> {
        actions
            .iter()
            .filter_map(|a| {
                self.mod_manager
                    .action_owner(&a.name)
                    .map(|o| (a.name.clone(), o.clone()))
            })
            .collect()
    }

    /// The manual-import funnel (owner, 2026-10-22 for
    /// KingMods; shared with the Other Links page's
    /// itch.io rows, 2026-10-24): stage a copy of the
    /// file the user downloaded by hand under the
    /// funnel's own download name, beside the mod — the
    /// completion path deletes its input file when it
    /// lands, and that file must be our copy, never the
    /// user's download. The job is registered and
    /// started like any install's, with the manual
    /// source named in it; the staged copy then rides
    /// the exact message a finished download lands
    /// with — the same funnel, the same bookkeeping,
    /// no fork.
    fn stage_manual_import(
        &mut self,
        id: String,
        picked: Option<PathBuf>,
        source_label: &str,
    ) -> Task<Message> {
            let Some(picked) = picked else {
                return Task::none();
            };
            let Some(m) = self.mod_manager.get_mod(&id).cloned() else {
                return Task::none();
            };
            // Stage a copy under the funnel's own
            // download name, beside the mod — the
            // completion path deletes its input file
            // when it lands, and that file must be our
            // copy, never the user's download. (A
            // download whose name differs from the
            // mod's own is carried in under this staging
            // name, so the funnel's checks meet the file
            // they expect.)
            let stem = ModManager::mod_stem(&m.path);
            let staged = m
                .path
                .parent()
                .map(|p| p.join(format!(".playground-dl-{}.zip", stem)))
                .unwrap_or_else(|| PathBuf::from(".playground-dl.zip"));
            if let Err(e) = std::fs::copy(&picked, &staged) {
                self.status = format!(
                    "Could not stage {} for installing: {}",
                    picked.display(),
                    e
                );
                return Task::none();
            }
            // The job is the history entry: registered
            // and started like any install's, with the
            // manual import named in its source.
            let version = match self.update_status.get(&id) {
                Some(UpdateState::Available(info)) => info.version.clone(),
                _ => self
                    .db_rows
                    .get(&stem)
                    .and_then(|r| r.latest_version.clone())
                    .unwrap_or_default(),
            };
            let job = self.downloads.register(
                id.clone(),
                m.name.clone(),
                m.file_name(),
                source_label.to_string(),
                version,
            );
            self.downloads.start(job);
            self.status = format!(
                "Installing the file you picked for {}…",
                m.name
            );
            // The staged copy rides the exact message a
            // finished download lands with — the same
            // funnel, the same bookkeeping, no fork.
            Task::done(Message::UpdateInstalled(id, Ok(staged)))
    }

    /// The Keybindings page's Purge box: the two-step
    /// removal of archived mods' bindings. The first
    /// press counts the candidates and reports them in
    /// place; the second stages the removals into the
    /// document — Save bindings (backup first) is still
    /// the only way anything reaches disk, and Discard
    /// abandons the purge like any other staged edit.
    fn kb_purge_box(&self) -> Element<'_, Message> {
        let mut col =
            column![text("Purge inactive bindings").size(15)].spacing(7);
        col = col.push(
            text("Remove the bindings of archived mods — mods not loaded in game. Active mods, base game and unclaimed actions are never touched.")
                .size(12)
                .color(theme::text_dim()),
        );
        let (label, armed) = match self.kb_purge {
            Some((actions, mods)) => (
                format!(
                    "{} actions from {} archived mods — click again to stage purge",
                    actions, mods
                ),
                true,
            ),
            None => ("Purge inactive bindings".to_string(), false),
        };
        col = col.push(
            button(text(label))
                .style(theme::nav_pill(armed))
                .on_press(Message::KeybindingsPurgePressed),
        );
        container(col)
            .width(Length::Fill)
            .padding(11)
            .style(wizard_section_box)
            .into()
    }

    /// The Keybindings page's Backups box: the timestamped
    /// backups (newest first) with Restore, the profile
    /// folder opener, and the arm-to-confirm reset.
    fn kb_backups_box(&self) -> Element<'_, Message> {
        let mut col = column![text("Backups").size(15)].spacing(7);
        if self.kb_backups.is_empty() {
            col = col.push(
                text("No backups yet — one is taken automatically before every save.")
                    .size(12)
                    .color(theme::text_dim()),
            );
        }
        for (path, size) in &self.kb_backups {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            col = col.push(
                row![
                    text(format!("{} ({})", name, format_size(*size)))
                        .size(12)
                        .width(Length::Fill),
                    button(text("Restore"))
                        .style(theme::nav_pill(false))
                        .on_press(Message::KeybindingsRestoreBackup(
                            path.clone(),
                        )),
                ]
                .spacing(8),
            );
        }
        let mut buttons = row![].spacing(8);
        if let Some(profile) = &self.kb_profile {
            buttons = buttons.push(
                button(text("Open profile folder"))
                    .style(theme::nav_pill(false))
                    .on_press(Message::OpenFolder(profile.clone())),
            );
        }
        if self.kb_doc.is_some() {
            let label = if self.kb_reset_armed {
                "Click again to confirm reset"
            } else {
                "Reset to game defaults"
            };
            buttons = buttons.push(
                button(text(label))
                    .style(theme::nav_pill(self.kb_reset_armed))
                    .on_press(Message::KeybindingsResetDefaults),
            );
        }
        col = col.push(buttons);
        container(col)
            .width(Length::Fill)
            .padding(11)
            .style(wizard_section_box)
            .into()
    }

    /// (Re)load the Game Screens page's live state: the
    /// shared folder's splash, its hash identity, the
    /// highlands file's presence, the backups — and the
    /// pending preview for the current choice. An absent
    /// or wrong install folder is an honest error state,
    /// never a write.
    fn gs_reload(&mut self) {
        self.gs_restore_armed = false;
        self.gs_error = None;
        self.gs_live_canvas = None;
        self.gs_live_band = None;
        self.gs_identity = None;
        self.gs_highlands = false;
        self.gs_backups = Vec::new();
        self.gs_install = None;
        self.gs_needs_locate = false;
        self.gs_pick_note = None;
        self.gs_root =
            crate::paths::data_dir().map(|d| gamescreens::backups_root(&d));
        // The install folder is Game Screens' own setting —
        // the game's true install folder, never the profile
        // folder (game.xml) the app also keeps.
        let install = match gamescreens::resolve_install_dir(
            self.mod_manager.game_install_dir().as_deref(),
        ) {
            gamescreens::InstallResolution::Usable(dir) => dir,
            gamescreens::InstallResolution::Unset => {
                self.gs_needs_locate = true;
                self.gs_error = Some(
                    "No game install folder is set for Game Screens — it reads the game's screens from the folder the game is installed in, the one holding the shared folder. Locate it below, or set \"Game install folder\" in Settings > Game Folders. Nothing was written."
                        .to_string(),
                );
                return;
            }
            gamescreens::InstallResolution::NoSharedFolder(dir) => {
                self.gs_needs_locate = true;
                self.gs_error = Some(format!(
                    "The game install folder {} has no shared folder under it — Game Screens reads the game's screens from the install folder's shared folder. Locate the right folder below, or fix \"Game install folder\" in Settings > Game Folders. Nothing was written.",
                    dir.display()
                ));
                return;
            }
        };
        self.gs_install = Some(install.clone());
        let shared = gamescreens::shared_dir(&install);
        self.gs_shared = Some(shared.clone());
        let splash_path = shared.join(gamescreens::SPLASH_NAME);
        if !splash_path.is_file() {
            self.gs_error = Some(format!(
                "No {} in {} — the game creates its screens when it runs. Nothing was written.",
                gamescreens::SPLASH_NAME,
                shared.display()
            ));
            return;
        }
        self.gs_highlands = shared.join(gamescreens::HIGHLANDS_NAME).is_file();
        let root = self.gs_root.clone().unwrap_or_default();
        self.gs_backups = gamescreens::list_backups(&root);
        let live_sha = std::fs::read(&splash_path)
            .ok()
            .map(|bytes| gamescreens::sha256_hex(&bytes));
        if let Ok(bytes) = std::fs::read(&splash_path) {
            if let Ok(img) = gamescreens::decode_dds(&bytes) {
                let (w, h, rgba) = gamescreens::preview_rgba(&img, 320, 320);
                self.gs_live_canvas = Some(ImageHandle::from_rgba(w, h, rgba));
                let band = gamescreens::visible_band(&img);
                let (w, h, rgba) = gamescreens::preview_rgba(&band, 512, 288);
                self.gs_live_band = Some(ImageHandle::from_rgba(w, h, rgba));
            }
        }
        let stock_sha = gamescreens::oldest_backup(&root)
            .and_then(|dir| std::fs::read(dir.join(gamescreens::SPLASH_NAME)).ok())
            .map(|bytes| gamescreens::sha256_hex(&bytes));
        let generated = crate::paths::data_dir()
            .map(|d| gamescreens::GeneratedHashes::load(&d))
            .unwrap_or_default();
        self.gs_identity = Some(gamescreens::identify(
            live_sha.as_deref(),
            stock_sha.as_deref(),
            &generated,
        ));
        self.gs_refresh_pending();
    }

    /// Rebuild the pending preview (the exact composition
    /// an Apply would write) for the current choice.
    fn gs_refresh_pending(&mut self) {
        self.gs_pending_canvas = None;
        self.gs_pending_band = None;
        let composed: Option<RgbImage> = match self.gs_choice {
            GsChoice::Standard => {
                let root = self.gs_root.clone().unwrap_or_default();
                match gamescreens::oldest_backup(&root)
                    .and_then(|dir| {
                        std::fs::read(dir.join(gamescreens::SPLASH_NAME)).ok()
                    })
                    .and_then(|bytes| gamescreens::decode_dds(&bytes).ok())
                {
                    Some(img) => {
                        self.gs_pending_note = "The stock screens, held in the oldest capture. Applying restores them (the current pair is captured first).".to_string();
                        Some(img)
                    }
                    None => {
                        self.gs_pending_note = "No stock capture yet — the first Apply of a preset or a custom image captures the current screens as the stock originals.".to_string();
                        None
                    }
                }
            }
            GsChoice::Preset(p) => {
                let bytes = match p {
                    GsPreset::Daylight => DAYLIGHT_MASTER,
                    GsPreset::Dark => DARK_MASTER,
                };
                match gamescreens::decode_source(bytes) {
                    Ok(art) => {
                        self.gs_pending_note = match p {
                            GsPreset::Daylight => {
                                "Farmer Jay Daylight — the wide pair, proven in game."
                                    .to_string()
                            }
                            GsPreset::Dark => {
                                "Farmer Jay Dark — the night pair, with the Playground shield and Albert Einstein."
                                    .to_string()
                            }
                        };
                        Some(gamescreens::compose(&art))
                    }
                    Err(e) => {
                        self.gs_pending_note =
                            format!("The bundled preset art could not be read: {:#}", e);
                        None
                    }
                }
            }
            GsChoice::Custom => match &self.gs_custom_art {
                Some(art) => {
                    self.gs_pending_note = format!(
                        "Your image: {}.",
                        self.gs_custom_name.as_deref().unwrap_or("custom image")
                    );
                    Some(gamescreens::compose(art))
                }
                None => {
                    self.gs_pending_note =
                        "Pick an image — PNG, JPG, WebP, BMP or a DXT1 DDS. It is scaled to the full width and bottom-aligned, by the game's own rule."
                            .to_string();
                    None
                }
            },
        };
        if let Some(img) = composed {
            let (w, h, rgba) = gamescreens::preview_rgba(&img, 288, 288);
            self.gs_pending_canvas = Some(ImageHandle::from_rgba(w, h, rgba));
            let band = gamescreens::visible_band(&img);
            let (w, h, rgba) = gamescreens::preview_rgba(&band, 512, 288);
            self.gs_pending_band = Some(ImageHandle::from_rgba(w, h, rgba));
        }
    }

    /// The Game Screens page's Backups box: the timestamped
    /// captures (newest first) with per-capture Restore.
    fn gs_backups_box(&self) -> Element<'_, Message> {
        let mut col = column![text("Backups").size(15)].spacing(7);
        if self.gs_backups.is_empty() {
            col = col.push(
                text("No backups yet — one is captured automatically before every Apply or restore.")
                    .size(12)
                    .color(theme::text_dim()),
            );
        }
        for backup in &self.gs_backups {
            col = col.push(
                row![
                    text(format!(
                        "{} ({})",
                        backup.stamp,
                        backup.files.join(", ")
                    ))
                    .size(12)
                    .width(Length::Fill),
                    button(text("Restore"))
                        .style(theme::nav_pill(false))
                        .on_press(Message::GameScreensRestoreBackup(
                            backup.dir.clone(),
                        )),
                ]
                .spacing(8),
            );
        }
        container(col)
            .width(Length::Fill)
            .padding(11)
            .style(wizard_section_box)
            .into()
    }

    /// The Game Screens page: the current screen and its
    /// identity, the choice of what to write (stock, a
    /// bundled preset, or a custom image) with the exact
    /// composition previewed, and the backups. Nothing
    /// writes except Apply and Restore, and both capture
    /// the current files first.
    fn gamescreens_view(&self) -> Element<'_, Message> {
        let title = text("Game Screens").size(20);
        if let Some(err) = &self.gs_error {
            let mut col = column![title, text(err.clone()).size(13)].spacing(10);
            if self.gs_needs_locate {
                col = col.push(
                    row![button(text("Locate game install folder…"))
                        .style(theme::nav_pill(true))
                        .on_press(Message::BrowseFolder(
                            FolderTarget::GameInstall,
                        ))],
                );
                if let Some(note) = &self.gs_pick_note {
                    col = col.push(
                        text(format!("Last pick refused: {}", note))
                            .size(12)
                            .color(theme::text_dim()),
                    );
                }
            }
            return page_scroll(col.push(self.gs_backups_box()))
                .height(Length::Fill)
                .into();
        }
        // Current screen.
        let mut current = column![text("Current screen").size(15)].spacing(7);
        if let Some(band) = &self.gs_live_band {
            current = current.push(
                iced::widget::image(band.clone()).width(Length::Fixed(512.0)),
            );
        } else {
            current = current.push(
                text("The current splash could not be previewed.")
                    .size(12)
                    .color(theme::text_dim()),
            );
        }
        let identity = self
            .gs_identity
            .map(|i| i.label())
            .unwrap_or("Unknown");
        current = current.push(
            row![
                text(format!("Active in Game: {}", identity)).size(13),
                text(if self.gs_highlands {
                    "splash_highlandsFishing.dds: present — Playground never writes this file."
                } else {
                    "splash_highlandsFishing.dds: not present."
                })
                .size(12)
                .color(theme::text_dim()),
            ]
            .spacing(12),
        );
        if let Some(install) = &self.gs_install {
            current = current.push(
                text(format!(
                    "Game install folder: {}",
                    install.display()
                ))
                .size(12)
                .color(theme::text_dim()),
            );
        }
        if let Some(shared) = &self.gs_shared {
            current = current.push(
                row![button(text("Open shared folder"))
                    .style(theme::nav_pill(false))
                    .on_press(Message::OpenFolder(shared.clone()))],
            );
        }
        let current_box = container(current)
            .width(Length::Fill)
            .padding(11)
            .style(wizard_section_box);

        // The choice + its preview.
        let mut choose = column![text("Screens").size(15)].spacing(7);
        // GS2 (cosmetic round, John, 2026-10-23): the
        // options are preview CARDS — each shows its
        // screen; click selects. Dark is selected when
        // the page opens (GS1). Standard's preview is
        // the live band when the stock screen is the
        // active one; before any capture exists the card
        // says so rather than faking a picture. Custom
        // keeps its live composition preview below once
        // an image is chosen.
        let gs_card = |label: &'static str,
                       preview: Option<iced::widget::image::Handle>,
                       selected: bool,
                       msg: Message| {
            let face: Element<'_, Message> = match preview {
                Some(handle) => iced::widget::image(handle)
                    .width(Length::Fixed(200.0))
                    .into(),
                None => container(
                    text("No capture yet").size(12).color(theme::text_dim()),
                )
                .width(Length::Fixed(200.0))
                .height(Length::Fixed(63.0))
                .center_x(200.0)
                .center_y(63.0)
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(theme::panel())),
                    border: iced::Border {
                        radius: 6.0.into(),
                        ..iced::Border::default()
                    },
                    ..container::Style::default()
                })
                .into(),
            };
            button(column![face, text(label).size(13)].spacing(5))
                .padding(7)
                .style(move |_theme, _status| button::Style {
                    background: Some(iced::Background::Color(if selected {
                        theme::accent()
                    } else {
                        theme::panel()
                    })),
                    text_color: if selected {
                        theme::text()
                    } else {
                        theme::text_dim()
                    },
                    border: iced::Border {
                        radius: 8.0.into(),
                        ..iced::Border::default()
                    },
                    ..button::Style::default()
                })
                .on_press(msg)
        };
        let standard_preview =
            if self.gs_identity == Some(ActiveIdentity::Stock) {
                self.gs_live_band.clone()
            } else {
                None
            };
        choose = choose.push(
            row![
                gs_card(
                    "FS25 Standard",
                    standard_preview,
                    self.gs_choice == GsChoice::Standard,
                    Message::GameScreensStandardSelected,
                ),
                gs_card(
                    "Farmer Jay Daylight",
                    Some(iced::widget::image::Handle::from_bytes(
                        DAYLIGHT_MASTER.to_vec(),
                    )),
                    self.gs_choice == GsChoice::Preset(GsPreset::Daylight),
                    Message::GameScreensPresetSelected(GsPreset::Daylight),
                ),
                gs_card(
                    "Farmer Jay Dark",
                    Some(iced::widget::image::Handle::from_bytes(
                        DARK_MASTER.to_vec(),
                    )),
                    self.gs_choice == GsChoice::Preset(GsPreset::Dark),
                    Message::GameScreensPresetSelected(GsPreset::Dark),
                ),
                gs_card(
                    "Custom image…",
                    self.gs_pending_band.clone(),
                    self.gs_choice == GsChoice::Custom,
                    Message::GameScreensPickCustom,
                ),
            ]
            .spacing(8),
        );
        choose = choose.push(
            text(self.gs_pending_note.clone())
                .size(12)
                .color(theme::text_dim()),
        );
        if self.gs_pending_canvas.is_some() || self.gs_pending_band.is_some() {
            let mut previews = row![].spacing(12);
            if let Some(canvas) = &self.gs_pending_canvas {
                previews = previews.push(
                    column![
                        iced::widget::image(canvas.clone())
                            .width(Length::Fixed(216.0)),
                        text("The full canvas — the game shows the bottom band.")
                            .size(11)
                            .color(theme::text_dim()),
                    ]
                    .spacing(4),
                );
            }
            if let Some(band) = &self.gs_pending_band {
                previews = previews.push(
                    column![
                        iced::widget::image(band.clone())
                            .width(Length::Fixed(384.0)),
                        text("What the game shows.")
                            .size(11)
                            .color(theme::text_dim()),
                    ]
                    .spacing(4),
                );
            }
            choose = choose.push(previews);
        }
        let apply_label = match self.gs_choice {
            GsChoice::Standard => {
                if self.gs_restore_armed {
                    "Click again to confirm"
                } else {
                    "Apply — restore stock screens"
                }
            }
            _ => "Apply screens",
        };
        choose = choose.push(
            row![
                button(text(apply_label))
                    .style(theme::nav_pill(true))
                    .on_press_maybe(if self.gs_busy {
                        None
                    } else {
                        Some(Message::GameScreensApply)
                    }),
                if self.gs_busy {
                    text("Building…").size(12).color(theme::text_dim())
                } else {
                    text("").size(12)
                },
            ]
            .spacing(10),
        );
        choose = choose.push(
            text("Apply while the game is closed; it reads these at launch.")
                .size(12)
                .color(theme::text_dim()),
        );
        let choose_box = container(choose)
            .width(Length::Fill)
            .padding(11)
            .style(wizard_section_box);

        // Wrapped in page_scroll like every other page:
        // this page shipped (v1.6.3) as a bare column, so
        // at shorter window heights its tail — the Backups
        // box — slid under the status slab with no
        // scrollbar to reach it (John, live, 2026-10-23).
        page_scroll(
            column![title, current_box, choose_box, self.gs_backups_box()]
                .spacing(10),
        )
        .height(Length::Fill)
        .into()
    }

    /// The Keybindings page: device tabs over collapsible
    /// per-owner groups of action rows, staged editing
    /// with a save footer, and the Backups box. The model
    /// is the game's own inputBinding.xml (see
    /// mod_manager::keybindings) — nothing here writes it
    /// except the page's explicit Save.
    fn keybindings_view(&self) -> Element<'_, Message> {
        let title = text("Keybindings").size(20);
        if self.kb_profile.is_none() {
            return column![
                title,
                text("No game profile folder was found — check that the game mods folder really is a game's mods folder.")
                    .size(13),
            ]
            .spacing(10)
            .into();
        }
        let Some(doc) = &self.kb_doc else {
            return column![
                title,
                text("No inputBinding.xml in the profile folder yet — the game creates this file the first time it runs. Once it exists, your bindings show up here.")
                    .size(13),
                self.kb_backups_box(),
            ]
            .spacing(10)
            .into();
        };

        // The analysis set: every action in the document,
        // or — with Show archived mods off (the default) —
        // the document minus archived mods' actions. The
        // document itself always holds every block.
        let all_actions = doc.actions();
        let owners_full = self.kb_owners(&all_actions);
        let actions = keybindings::visible_actions(
            &all_actions,
            &owners_full,
            self.kb_show_archived,
        );
        let devices = doc.devices();
        let device = self
            .kb_device
            .clone()
            .filter(|d| devices.contains(d))
            .unwrap_or_else(|| {
                devices
                    .first()
                    .cloned()
                    .unwrap_or_else(|| keybindings::KEYBOARD_DEVICE.to_string())
            });
        let is_keyboard = device == keybindings::KEYBOARD_DEVICE;

        // Device tabs, as nav pills.
        let mut tabs = row![].spacing(6);
        for d in &devices {
            tabs = tabs.push(
                button(text(keybindings::device_label(d)))
                    .style(theme::nav_pill(*d == device))
                    .on_press(Message::KeybindingsDeviceSelected(d.clone())),
            );
        }

        // Findings on the selected device, tiered
        // (v1.6.2): each Conflict finding is Hard (a mod
        // fighting itself for a key), Shared (mod-touching)
        // or Base (no mod involved), classified against
        // the manager's action -> owner map.
        let findings = keybindings::analyze(&actions, &device);
        let owners: HashMap<String, String> =
            keybindings::owner_titles(&owners_full);
        let tiers: Vec<Option<keybindings::FindingTier>> = findings
            .iter()
            .map(|f| keybindings::classify(f, &owners))
            .collect();
        let counts = keybindings::tier_counts(&findings, &owners);
        // Actions involved in a Hard finding on this
        // device — the "Hard conflicts only" filter's set.
        let hard_actions: HashSet<&str> = findings
            .iter()
            .zip(tiers.iter())
            .filter(|(_, t)| **t == Some(keybindings::FindingTier::Hard))
            .flat_map(|(f, _)| f.actions.iter().map(|s| s.as_str()))
            .collect();
        // Per-action row marks: the most severe finding an
        // action is involved in on this device (Hard >
        // Shared > Base > Warning), named in quiet text.
        let mark_rank = |label: &str| match label {
            "Hard conflict" => 3,
            "Shared key" => 2,
            "Base game overlap" => 1,
            _ => 0,
        };
        let mut action_marks: HashMap<&str, &str> = HashMap::new();
        for (f, t) in findings.iter().zip(tiers.iter()) {
            let label = match t {
                Some(keybindings::FindingTier::Hard) => "Hard conflict",
                Some(keybindings::FindingTier::Shared) => "Shared key",
                Some(keybindings::FindingTier::Base) => "Base game overlap",
                None => "Warning",
            };
            for a in &f.actions {
                let entry = action_marks.entry(a.as_str()).or_insert(label);
                if mark_rank(label) > mark_rank(entry) {
                    *entry = label;
                }
            }
        }

        // Filters.
        let query = self.kb_search.trim().to_lowercase();
        let action_visible = |group: &str, a: &KbAction| -> bool {
            if self.kb_unassigned_only
                && a.bindings.iter().any(|b| b.device == device)
            {
                return false;
            }
            if self.kb_conflicts_only && !hard_actions.contains(a.name.as_str()) {
                return false;
            }
            if !query.is_empty() {
                let in_name = a.name.to_lowercase().contains(&query)
                    || keybindings::humanize_action(&a.name)
                        .to_lowercase()
                        .contains(&query);
                if !in_name && !group.to_lowercase().contains(&query) {
                    return false;
                }
            }
            true
        };

        let mut groups_col = column![].spacing(10);
        for (group, group_actions) in self.kb_grouped_actions() {
            let visible: Vec<&KbAction> = group_actions
                .iter()
                .filter(|a| action_visible(&group, a))
                .collect();
            if visible.is_empty() {
                continue;
            }
            let group_hard = findings
                .iter()
                .zip(tiers.iter())
                .filter(|(f, t)| {
                    **t == Some(keybindings::FindingTier::Hard)
                        && f.actions
                            .iter()
                            .any(|a| group_actions.iter().any(|ga| &ga.name == a))
                })
                .count();
            let collapsed = self.kb_collapsed.contains(&group);
            let mut header = row![
                button(text(if collapsed { "+" } else { "–" }).size(14))
                    .style(theme::nav_pill(false))
                    .on_press(Message::KeybindingsToggleGroup(group.clone())),
                text(format!("{} ({})", group, group_actions.len())).size(15),
                if group_hard > 0 {
                    text(format!("{} hard", group_hard))
                        .size(12)
                        .color(theme::text_dim())
                } else {
                    text("").size(12)
                },
                Space::with_width(Length::Fill),
            ]
            .spacing(8);
            // Auto-Resolve lives only where it can act:
            // a group with a Hard finding on this device.
            if group_hard > 0 {
                header = header.push(
                    button(text("Auto-Resolve"))
                        .style(theme::nav_pill(false))
                        .on_press(Message::KeybindingsAutoResolve(group.clone())),
                );
            }
            let mut box_col = column![header].spacing(7);
            if !collapsed {
                for a in visible {
                    let category = owners_full
                        .get(&a.name)
                        .and_then(|o| o.category.clone());
                    let mut name_col =
                        column![text(keybindings::humanize_action(&a.name)).size(13)]
                            .spacing(2);
                    if let Some(category) = category {
                        name_col = name_col.push(
                            text(category).size(11).color(theme::text_dim()),
                        );
                    }
                    if let Some(mark) = action_marks.get(a.name.as_str()) {
                        let color = if *mark == "Hard conflict" {
                            iced::Color::from_rgb(0.62, 0.16, 0.13)
                        } else {
                            theme::text_dim()
                        };
                        name_col = name_col
                            .push(text(*mark).size(11).color(color));
                    }
                    let mut chips = row![].spacing(6);
                    let device_bindings: Vec<_> = a
                        .bindings
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| b.device == device)
                        .collect();
                    if device_bindings.is_empty() {
                        let capturing = matches!(
                            &self.kb_capture,
                            Some(KbCapture::Add { action }) if action == &a.name
                        );
                        if capturing {
                            chips = chips.push(
                                button(text("Press a key... (Esc cancels)").size(12))
                                    .style(theme::nav_pill(true)),
                            );
                        } else if is_keyboard {
                            chips = chips.push(
                                button(text("Unassigned — Add").size(12))
                                    .style(theme::nav_pill(false))
                                    .on_press(Message::KeybindingsAddBinding(
                                        a.name.clone(),
                                    )),
                            );
                        } else {
                            chips = chips.push(
                                text("Unassigned").size(12).color(theme::text_dim()),
                            );
                        }
                    } else {
                        for (pos, binding) in device_bindings {
                            let capturing = matches!(
                                &self.kb_capture,
                                Some(KbCapture::Replace { action, pos: p })
                                    if action == &a.name && *p == pos
                            );
                            let label = if capturing {
                                "Press a key... (Esc cancels)".to_string()
                            } else {
                                keybindings::humanize_input(&binding.input)
                            };
                            // A binding whose every token has a
                            // keycap wears the caps; anything
                            // else (axes, gamepad tokens, names
                            // with no cap drawn) keeps the text
                            // chip.
                            let caps = if capturing {
                                None
                            } else {
                                crate::ui::keycaps::binding_caps(&binding.input)
                            };
                            let mut chip = match caps {
                                Some(caps) => {
                                    let mut caps_row = row![]
                                        .spacing(3)
                                        .align_y(iced::Alignment::Center);
                                    for (i, cap) in caps.iter().enumerate() {
                                        if i > 0 {
                                            caps_row = caps_row.push(
                                                text("+")
                                                    .size(11)
                                                    .color(theme::text_dim()),
                                            );
                                        }
                                        caps_row = caps_row.push(
                                            iced::widget::image(
                                                cap.handle.clone(),
                                            )
                                            .width(Length::Fixed(
                                                30.0 * cap.units.min(3.0),
                                            ))
                                            .height(Length::Fixed(30.0))
                                            .content_fit(iced::ContentFit::Fill),
                                        );
                                    }
                                    button(caps_row)
                                        .style(theme::nav_pill(capturing))
                                }
                                None => button(text(label).size(12))
                                    .style(theme::nav_pill(capturing)),
                            };
                            if is_keyboard {
                                chip = chip.on_press(
                                    Message::KeybindingsCaptureStart(
                                        a.name.clone(),
                                        pos,
                                    ),
                                );
                            }
                            chips = chips.push(
                                row![
                                    chip,
                                    button(text("x").size(12))
                                        .style(theme::nav_pill(false))
                                        .on_press(
                                            Message::KeybindingsClearBinding(
                                                a.name.clone(),
                                                pos,
                                            ),
                                        ),
                                ]
                                .spacing(2),
                            );
                        }
                    }
                    box_col = box_col.push(
                        row![
                            name_col.width(Length::Fill),
                            chips,
                        ]
                        .spacing(8),
                    );
                }
            }
            groups_col = groups_col.push(
                container(box_col)
                    .width(Length::Fill)
                    .padding(11)
                    .style(wizard_section_box),
            );
        }
        groups_col = groups_col.push(self.kb_purge_box());
        groups_col = groups_col.push(self.kb_backups_box());

        let top_bar = row![
            text_input("Search actions or mods", &self.kb_search)
                .style(theme::input_field())
                .on_input(Message::KeybindingsSearchChanged)
                .width(260),
            checkbox("Unassigned only", self.kb_unassigned_only)
                .on_toggle(Message::KeybindingsUnassignedOnlyToggled),
            checkbox("Hard conflicts only", self.kb_conflicts_only)
                .on_toggle(Message::KeybindingsConflictsOnlyToggled),
            checkbox("Show archived mods", self.kb_show_archived)
                .on_toggle(Message::KeybindingsShowArchivedToggled),
            Space::with_width(Length::Fill),
            button(text("Expand all"))
                .style(theme::nav_pill(false))
                .on_press(Message::KeybindingsExpandAll(true)),
            button(text("Collapse all"))
                .style(theme::nav_pill(false))
                .on_press(Message::KeybindingsExpandAll(false)),
        ]
        .spacing(10);

        let mut page = column![title, tabs].spacing(10);
        let (banner1, banner2) = keybindings::banner_lines(&counts);
        page = page.push(text(banner1));
        if let Some(line2) = banner2 {
            page = page.push(text(line2).size(12).color(theme::text_dim()));
        }
        page = page.push(top_bar);
        if !is_keyboard {
            page = page.push(
                text("Key capture is keyboard-only for now — on this device you can view and clear bindings.")
                    .size(12)
                    .color(theme::text_dim()),
            );
        }
        page = page.push(
            page_scroll_pad(groups_col, PAGE_MARGIN + SCROLLBAR_W)
                .height(Length::Fill)
                .width(Length::Fill),
        );
        // The save footer, visible only while staged edits
        // exist: the Settings one-save pattern.
        let dirty = match &self.kb_original {
            Some(original) => {
                doc.changed_blocks(&BindingsFile::parse(original.clone()))
            }
            None => 0,
        };
        if dirty > 0 {
            page = page.push(
                row![
                    button(text("Save bindings"))
                        .style(theme::nav_pill(true))
                        .on_press(Message::KeybindingsSave),
                    button(text("Discard changes"))
                        .style(theme::nav_pill(false))
                        .on_press(Message::KeybindingsDiscard),
                    text(format!(
                        "{} unsaved change{}",
                        dirty,
                        if dirty == 1 { "" } else { "s" }
                    ))
                    .size(12),
                ]
                .spacing(8),
            );
        }
        page.into()
    }

    /// Left sidebar (John, round 4 — the Hub layout piece): logo and
    /// app name up top, section navigation, then the LIBRARY counts
    /// that jump the Browse grid straight to that view.
    fn sidebar_view(&self) -> Element<'_, Message> {
        let all = self.mod_manager.get_mods();
        let count_cat = |name: &str| {
            all.iter()
                .filter(|m| m.categories.first().map_or(false, |c| c == name))
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
            button(centered_label(text(label).size(14)))
                .width(Length::Fill)
                // The current page's pill is the accent
                // (John, 2026-10-21) — supersedes the 2026-
                // 10-18 all-grey ruling: in the palette the
                // accent is quiet enough to mark the page
                // without shouting.
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
            nav(
                format!(
                    "Conflicts ({})",
                    self.shown_count("conflicts", self.conflicts.len())
                ),
                Tab::Conflicts,
            ),
            nav("Configure Save".to_string(), Tab::Saves),
            nav("Layout".to_string(), Tab::Layout),
            nav("Log Reader".to_string(), Tab::Log),
            nav("Keybindings".to_string(), Tab::Keybindings),
            nav("Game Screens".to_string(), Tab::GameScreens),
            // Downloads (2026-10-22): the live count rides
            // the label only while something is in play —
            // queued or running — like Conflicts' count
            // above; an idle section needs no number.
            nav(
                if self.downloads.live_count() > 0 {
                    format!(
                        "Downloads ({})",
                        self.shown_count("downloads", self.downloads.live_count())
                    )
                } else {
                    "Downloads".to_string()
                },
                Tab::Downloads,
            ),
            nav("Settings".to_string(), Tab::Settings),
            text("LIBRARY").size(11),
            lib_row("Mods", self.shown_count("mods", all.len()), BrowseView::All),
            // Favourites lives in LIBRARY, directly under
            // Mods (John, 2026-10-21 — his later call the
            // same day reverses its brief stay in the
            // features block): a library view in the library
            // rows' label+count dress, its count the same
            // favorites_count the FAVORITE MODS tile reads,
            // and it lights when that view is on screen.
            lib_row(
                "Favourites",
                self.shown_count("favs", self.mod_manager.favorites_count()),
                BrowseView::Favorites,
            ),
            lib_row(
                "Maps",
                self.shown_count("maps", count_cat("Maps")),
                BrowseView::Category("Maps".to_string()),
            ),
            lib_row(
                "Scripts",
                self.shown_count("scripts", count_cat("Scripts")),
                BrowseView::Category("Scripts".to_string()),
            ),
            // Disk usage rides with the library block, right
            // under the Scripts row (John, 2026-10-18) — the
            // empty stretch belongs between it and the
            // copyright footer pinned at the bottom.
            text(format!("Disk usage: {}", format_size(total_size))).size(12),
            Space::with_height(Length::Fill),
            text(format!("V{}  © 2026 Farmer Jay.", env!("CARGO_PKG_VERSION")))
                .size(11)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            text("By Farmer Jay / Albert Einstein")
                .size(12)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
        ]
        .spacing(8);

        // The shell row no longer pads the window's bottom
        // (the main stack's status slab covers that band
        // now), so the sidebar carries its own 12px bottom
        // inset here: its bordered box ends exactly where
        // it always did, 12 above the window's bottom edge.
        container(
            container(content)
                .width(SIDEBAR_W)
                .height(Length::Fill)
                .padding(12)
                .style(theme::sidebar_container),
        )
        .height(Length::Fill)
        .padding(iced::Padding {
            top: 0.0,
            right: 0.0,
            bottom: 12.0,
            left: 0.0,
        })
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
        let ttl_hours = self.mod_manager.cache_ttl_hours();
        Task::perform(
            async move {
                let pairs = match crate::sources::catalog::ensure(&url, ttl_hours).await {
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
    /// boot ends, as ONE shape change. The launch window
    /// is borderless (see `run`), so the handover is:
    /// re-decorate it, resize it to the app's standard
    /// 1280x800, and re-center it on the splash's center
    /// point (the splash launched centered — John,
    /// 2026-10-21; inheriting its top-left would push
    /// the bigger window off the right and bottom
    /// edges).
    ///
    /// The three actions fire as a single Task::batch
    /// once BOTH facts are collected — the window id
    /// (get_latest; the shell answers GetLatest
    /// unconditionally) and the splash's position
    /// (get_position) — in the RestoreWindowPosition
    /// handler, with the RestoreState machine keeping
    /// the sequence honest. Before v1.3.4 the frame and
    /// size fired immediately and the move landed
    /// whenever its own chain answered, which the owner
    /// saw as a lurch: frame at splash size, blow out to
    /// full size wherever the window sat, then a hop to
    /// center.
    ///
    /// STRUCTURE IS THE SAFETY (field bug, 2026-10-21:
    /// every launch on the owner's Windows stayed
    /// splash-shaped and borderless with the main UI
    /// inside it): the frame and size must never again
    /// depend on get_position. It is a oneshot task, and
    /// if the shell's answer never arrives the chained
    /// continuation silently never executes (the
    /// runtime's oneshot stream ends with no output),
    /// taking the whole restore with it and logging
    /// nothing. So the position read is collected under
    /// a deadline (RESTORE_POSITION_WAIT, driven by the
    /// RestoreTick subscription): if it has not answered
    /// in time, RestoreTick fires frame + size without
    /// the move (see restore_shape), and a position
    /// answer arriving after that is dropped rather than
    /// applied as a second, separate move. Off-center
    /// beats misshapen; one shape change beats two.
    ///
    /// The decorations action is a stateful toggle (Iced
    /// 0.13's runtime offers no set-decorations): the
    /// winit shell answers ToggleDecorations with
    /// `set_decorations(!is_decorated())` and Resize
    /// with `request_inner_size`. Nothing toggles the
    /// decorations between launch and the handover, so
    /// the toggle lands deterministically on decorated;
    /// and the state machine issues it exactly once per
    /// boot — the one exception is the verify net's
    /// guarded re-issue (see the RestoreVerifySize
    /// handler), which fires only when the window
    /// provably still wears the splash's size, i.e.
    /// when the coupled resize — and so the toggle
    /// ahead of it — demonstrably never landed.
    ///
    /// Transparency needs no undo: it is a creation-time
    /// property (window::Settings::transparent), and the
    /// moment `booting` drops, the theme closure in
    /// `run` returns Theme::Dark again and the shell's
    /// clear color goes back to Dark's opaque
    /// background — no trace of the clear window
    /// remains in the main one.
    fn restore_window() -> Task<Message> {
        iced::window::get_latest().map(Message::RestoreWindowId)
    }

    /// Frame and full size, without the centering move:
    /// the handover's fallback when the splash's
    /// position can't be known (the read answered None,
    /// or never answered at all). See restore_window for
    /// why the shape never rides on the position read.
    fn restore_shape(id: iced::window::Id) -> Task<Message> {
        Task::batch(vec![
            iced::window::toggle_decorations(id),
            iced::window::resize(id, Size::new(1280.0, 800.0)),
        ])
    }

    /// The main window's top-left when it is centered on
    /// the splash's center point: the splash sits at
    /// `splash_pos` at SPLASH_W x SPLASH_H, and the main
    /// window is 1280x800 around the same center.
    fn main_top_left(splash_pos: iced::Point) -> iced::Point {
        iced::Point::new(
            splash_pos.x + SPLASH_W / 2.0 - 640.0,
            splash_pos.y + SPLASH_H / 2.0 - 400.0,
        )
    }

    /// Re-center the main window on the splash's center
    /// point, reading the position on its own chain.
    /// Only the verify net's re-issue uses this separate
    /// form: when the batched handover demonstrably
    /// never landed, correctness beats choreography and
    /// the move rides the position read exactly as it
    /// did before the batching.
    fn center_main_window(id: iced::window::Id) -> Task<Message> {
        iced::window::get_position(id).then(move |pos| match pos {
            Some(pos) => iced::window::move_to(id, Self::main_top_left(pos)),
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
    /// mods, then updates judged — and, since the 1.5.3
    /// hold, until the icon pass behind them lands too, so
    /// the main window never opens onto numbers that are
    /// still settling. While the icons load, the readout's
    /// line speaks the status text instead of the counts.
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
        // The counts speak for the two scan phases. Once
        // both are done and only the icon pass is still
        // running — the boot hold keeps the splash up for
        // it — the counts have nothing left to count, so
        // the line speaks the status text instead ("Loaded
        // N mods — loading icons…"): the phase stays
        // visible while it lasts.
        let phases_done =
            self.load_total > 0 && self.load_done >= self.load_total;
        let counts = if phases_done && self.loading {
            self.status.clone()
        } else if self.load_total > 0 {
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
            progress_bar(0.0..=1.0, value)
                .width(260)
                .height(14)
                .style(theme::splash_bar),
            text(counts).size(16).color(theme::text()),
        ]
        .spacing(10)
        .align_x(iced::Alignment::Center);
        let overlay = column![
            Space::with_height(Length::Fill),
            readout,
            Space::with_height(14),
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

    /// The app self-update notice: a steel-teal slab in
    /// the view's top layer, directly above the status
    /// bar. The top layer already overlays the content's
    /// foot, so no page's geometry moves when the notice
    /// appears or clears — and like the status bar it
    /// sits in the layer the scroll-clip fix protects.
    /// An empty strip when there is nothing to say.
    fn app_update_notice(&self) -> Element<'_, Message> {
        let label: String;
        let mut actions = row![].spacing(8).align_y(iced::Alignment::Center);
        if self.app_update_installing {
            let tag = self
                .app_update
                .as_ref()
                .map(|r| r.tag.as_str())
                .unwrap_or("");
            label = format!(
                "Downloading Playground {} — the app will restart to finish installing.",
                tag
            );
        } else if let Some(failure) = &self.app_update_failed {
            label = format!("The app update did not install: {}", failure);
            actions = actions.push(
                button(text("Dismiss").size(12))
                    .style(theme::nav_pill(false))
                    .on_press(Message::DismissAppUpdate),
            );
        } else if let Some(release) = &self.app_update {
            label = format!(
                "Playground {} is available — you are running v{}.",
                release.tag,
                env!("CARGO_PKG_VERSION")
            );
            actions = actions.push(
                button(text("Install update").size(12))
                    .style(theme::nav_pill(false))
                    .on_press(Message::InstallAppUpdate),
            );
            actions = actions.push(
                button(text("Later").size(12))
                    .style(theme::nav_pill(false))
                    .on_press(Message::DismissAppUpdate),
            );
        } else {
            return Space::with_height(0).into();
        }
        let notice = container(
            row![
                text(label).size(13).color(theme::text()),
                Space::with_width(Length::Fill),
                actions,
            ]
            .align_y(iced::Alignment::Center),
        )
        .padding(iced::Padding {
            top: 8.0,
            right: 10.0,
            bottom: 8.0,
            left: 10.0,
        })
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(theme::accent())),
            ..container::Style::default()
        });
        // The visible slab keeps PAGE_MARGIN on its right
        // (John's margin rule — banners included, no
        // full-bleed exceptions): it rides in a plain
        // wrapper so the slab itself insets inside the main
        // column while everything beneath it stays put.
        container(notice)
            .padding(iced::Padding {
                right: PAGE_MARGIN,
                ..Default::default()
            })
            .width(Length::Fill)
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
        // With EMPTY top controls the slab is 40 + 10 + 0 + the
        // 8 of bottom padding = 58 tall, not 50 — the padding
        // paints BELOW the 50 mark, so a 50 hole left the slab
        // overpainting the top 8px of every such page's first
        // row (John, live, 2026-10-22: the Log Reader's title
        // decapitated under the banner; measured in the rig:
        // first-row ink flat-clipped at slab bottom on the Log
        // and Settings pages alike). The non-Browse hole is 58.
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
                58.0,
            ),
            Tab::Saves => (
                Space::with_height(0).into(),
                None,
                self.saves_view(),
                58.0,
            ),
            Tab::Layout => (
                Space::with_height(0).into(),
                None,
                self.layout_view(),
                58.0,
            ),
            Tab::Log => (Space::with_height(0).into(), None, self.log_view(), 58.0),
            Tab::Keybindings => (
                Space::with_height(0).into(),
                None,
                self.keybindings_view(),
                58.0,
            ),
            Tab::GameScreens => (
                Space::with_height(0).into(),
                None,
                self.gamescreens_view(),
                58.0,
            ),
            Tab::Downloads => (
                Space::with_height(0).into(),
                None,
                self.downloads_view(),
                58.0,
            ),
            Tab::Settings => (
                Space::with_height(0).into(),
                None,
                self.settings_view(),
                58.0,
            ),
        };

        let status = row![
            text(&self.status)
                .size(13)
                .color(iced::Color {
                    a: self.status_alpha(),
                    ..theme::text()
                }),
        ];

        let window_bg = theme::base();
        let base = column![
            Space::with_height(top_hole),
            content,
            // 39, not 27: the status slab's 27 plus the
            // 12 the shell row used to pad below the stack
            // (that band is the slab's to cover now — card
            // icons paint past the grid's clip into it).
            Space::with_height(39),
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
            // The app self-update notice (empty when the
            // launch check found nothing): see
            // app_update_notice.
            self.app_update_notice(),
            container(status)
                .padding(iced::Padding {
                    top: 10.0,
                    right: 0.0,
                    // The extra 12 below the text makes the
                    // slab 39 tall, reaching the window's
                    // bottom edge (the shell row no longer
                    // pads there) so no card paint can leak
                    // past it.
                    bottom: 12.0,
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
            // No bottom padding: the stack runs to the
            // window's bottom edge, where the status slab
            // (now 39 tall) covers the band card icons used
            // to leak into. The sidebar restores its own
            // 12px bottom inset inside sidebar_view.
            .padding(iced::Padding {
                top: 12.0,
                right: 12.0,
                bottom: 0.0,
                left: 12.0,
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let events = iced::event::listen_with(|event, _status, _id| match event {
            iced::Event::Window(iced::window::Event::Resized(size)) => {
                Some(Message::WindowResized(size))
            }
            _ => None,
        });
        // The boot handover's clock: it runs only while a
        // handover is live (the position deadline and the
        // verify net's schedule) and is gone the moment
        // the handover settles, so the app carries no
        // timer for the rest of its life.
        let mut subs = vec![events];
        if self.window_restore.ticking() {
            subs.push(iced::time::every(RESTORE_TICK).map(|_| Message::RestoreTick));
        }
        // Hold-to-delete's clock: runs only while a
        // Remove hold is live (feel pass, 2026-10-23).
        if self.remove_hold.is_some() {
            subs.push(iced::time::every(REMOVE_HOLD_TICK).map(|_| Message::RemoveHoldTick));
        }
        // The feel pass's shared clock (2026-10-23):
        // subscribed only while an animation is live or
        // settling — the app carries no idle timer. A
        // status the clock has not seen yet, or one still
        // fading in, also keeps it subscribed (part 2).
        if std::time::Instant::now() < self.anim_until
            || !self.switch_anims.is_empty()
            || self.status != self.status_seen
            || self.status_changed_at.elapsed() < std::time::Duration::from_millis(220)
            || (self.grid_offset - self.grid_shown).abs() > 0.5
        {
            subs.push(iced::time::every(ANIM_TICK).map(|_| Message::AnimTick));
        }
        // Key capture (Keybindings page): a keyboard
        // listener exists only while a capture is armed,
        // so ordinary key presses never reach the app.
        if self.kb_capture.is_some() {
            subs.push(iced::event::listen_with(|event, _status, _id| match event {
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key,
                    location,
                    modifiers,
                    ..
                }) => Some(Message::KeybindingsCaptureKey(key, location, modifiers)),
                _ => None,
            }));
        }
        Subscription::batch(subs)
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

/// Background job for a Layout page apply, run on a worker
/// thread: executes EXACTLY the plan the owner previewed
/// and approved — `layout_exec::apply_previewed` refuses
/// if the folders or the library drifted since the
/// preview, so unapproved work can never run. Op counts
/// stream back over the channel as operations land (try_send:
/// a dropped tick is replaced by the next one); the finish
/// event uses a blocking send so the outcome cannot be
/// lost. A run that failed partway and reversed itself is
/// reported as its own outcome, never as success.
fn layout_apply_job(
    preview: LayoutPlan,
    lib: PathBuf,
    game: PathBuf,
    mods: Vec<Mod>,
    mut tx: futures::channel::mpsc::Sender<LayoutEvent>,
) {
    let progress_tx = std::sync::Arc::new(std::sync::Mutex::new(tx.clone()));
    let progress = move |done: usize, total: usize| {
        if let Ok(mut sender) = progress_tx.lock() {
            let _ = sender.try_send(LayoutEvent::Progress { done, total });
        }
    };
    let result = match layout_exec::apply_previewed(&lib, &game, &preview, &mods, &progress) {
        Ok(ApplyOutcome::AlreadyLaidOut) => Ok(LayoutRunOutcome::AlreadyLaidOut),
        Ok(ApplyOutcome::Migrated {
            moves,
            links,
            backup,
        }) => {
            // The run's log path rides the state record
            // the executor just wrote (status Swapped).
            let log = layout_exec::standing_state()
                .ok()
                .flatten()
                .map(|s| s.log_file.display().to_string())
                .unwrap_or_else(|| "(run log path unavailable)".to_string());
            Ok(LayoutRunOutcome::Migrated {
                moves,
                links,
                backup: backup.display().to_string(),
                log,
            })
        }
        Err(e) => {
            // A failure the executor already reversed
            // leaves a FailedReversed record behind; that
            // is a plain failure report, not an error of
            // the job itself. Anything else (a refusal
            // before any work, a reversal that itself met
            // problems) is an error in the failure's own
            // words.
            let reversed = layout_exec::standing_state()
                .ok()
                .flatten()
                .map(|s| s.status == MigrationStatus::FailedReversed)
                .unwrap_or(false);
            if reversed {
                Ok(LayoutRunOutcome::FailedReversed(format!("{:#}", e)))
            } else {
                Err(format!("{:#}", e))
            }
        }
    };
    let _ = futures::executor::block_on(tx.send(LayoutEvent::ApplyFinished(result)));
}

/// Background job for a Layout page rollback, run on a
/// worker thread: the same executor entry the CLI verb
/// runs (`layout_exec::rollback_recorded`), audit refusal
/// included. The outcome is translated into the page's
/// own summary lines; a refusal stays an error in the
/// executor's words.
fn layout_rollback_job(mut tx: futures::channel::mpsc::Sender<LayoutEvent>) {
    let result = match layout_exec::rollback_recorded() {
        Ok(RollbackOutcome::NothingRecorded) => {
            Ok("No layout migration is recorded — nothing to roll back.".to_string())
        }
        Ok(RollbackOutcome::AlreadyReversed) => Ok(
            "That migration already failed and reversed itself — nothing to restore. The record was cleared."
                .to_string(),
        ),
        Ok(RollbackOutcome::Restored) => Ok(
            "Rollback complete: the pre-layout game folder is back in place and the library sort is undone."
                .to_string(),
        ),
        Err(e) => Err(format!("{:#}", e)),
    };
    let _ = futures::executor::block_on(tx.send(LayoutEvent::RollbackFinished(result)));
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
    check_conflicts: bool,
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
            let conflicts =
                ConflictDetector::detect_conflicts_if_enabled(&mod_list, check_conflicts);
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
/// over the whole file, keep the last `cap` lines for display
/// (the cap is a Settings value since Round 2, default
/// 3000), and collect the library mods that error/warning
/// lines name (file-name stems matched against a lowercase
/// line).
fn log_job(
    profile: PathBuf,
    stems: Vec<(String, String)>,
    cap: usize,
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
    let start = total_lines.saturating_sub(cap.max(1));
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

fn icon_job(
    jobs: Vec<(String, PathBuf, Option<String>)>,
    cache_ttl_hours: u64,
) -> Vec<(String, LoadedIcon)> {
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
            load_icon_cached(path, icon.as_deref(), cache_dir.as_deref(), cache_ttl_hours)
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
                theme::app_theme()
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
            // The app self-update launch check (owner,
            // 2026-10-22): one small request in the
            // background from the first frame; it never
            // delays the load, and its only message is
            // Some when a newer Playground exists (see
            // app_update::check_at_launch). Settings can
            // switch the launch check off (Round 2,
            // self_update_enabled); the page's "Check now"
            // runs the same check on demand either way.
            let check = if app.mod_manager.self_update_enabled() {
                Task::perform(
                    app_update::check_at_launch(),
                    Message::AppUpdateCheckDone,
                )
            } else {
                Task::none()
            };
            (app, Task::batch([task, check]))
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
    /// The tiles' tallies: every record counts once toward
    /// the total, Active and Archived take only their own
    /// state, and a neither-state record (Conflict, Error)
    /// lands in the total alone — total = active + archived
    /// exactly when no such record exists.
    #[test]
    fn tally_states_counts_each_record_once() {
        let (total, active, archived) = tally_states(
            [
                ModState::Active,
                ModState::Archived,
                ModState::Active,
                ModState::Archived,
                ModState::Archived,
            ]
            .into_iter(),
        );
        assert_eq!((total, active, archived), (5, 2, 3));
        assert_eq!(total, active + archived);

        let (total, active, archived) = tally_states(
            [ModState::Active, ModState::Conflict, ModState::Error].into_iter(),
        );
        assert_eq!((total, active, archived), (3, 1, 0));

        assert_eq!(tally_states(std::iter::empty()), (0, 0, 0));
    }

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

    /// The batched handover centers the main window on
    /// the splash's own center point: on the owner's
    /// 1920x1080 screen the centered splash sits at
    /// (702, 344) and the main window must land at
    /// (320, 140); on the sandbox rig's 2560x1600 screen
    /// it is (1022, 604) -> (640, 400).
    /// The Browse default view's stored key round-trips
    /// every choice, and a damaged or foreign key falls
    /// back to All instead of stranding a launch.
    #[test]
    fn browse_default_key_round_trips() {
        for view in [
            BrowseView::All,
            BrowseView::Active,
            BrowseView::Archived,
            BrowseView::Favorites,
            BrowseView::Updates,
            BrowseView::Category("Tractors".to_string()),
        ] {
            assert_eq!(browse_view_from_key(&browse_view_key(&view)), view);
        }
        assert_eq!(browse_view_from_key("nonsense"), BrowseView::All);
        assert_eq!(browse_view_from_key("category:"), BrowseView::All);
        assert_eq!(browse_view_from_key(""), BrowseView::All);
    }

    /// The Diagnostics scrub masks the home path wherever
    /// it appears in the summary and leaves other text —
    /// and a summary without the home path — untouched.
    /// (Unix paths stand in for the Windows original; the
    /// replacement is a prefix substitution either way.)
    #[test]
    fn diagnostics_scrub_masks_the_home_path() {
        let home = std::path::Path::new("/home/jdoe");
        let text = "Data folder: /home/jdoe/.config/playground\n\
                    Catalog: /home/jdoe/.cache/x (1 KB)\n\
                    Other: /srv/shared";
        let scrubbed = scrub_home_prefix(text, home);
        assert!(scrubbed.contains("/home/<user>/.config/playground"));
        assert!(scrubbed.contains("/home/<user>/.cache/x"));
        assert!(scrubbed.contains("/srv/shared"));
        assert!(!scrubbed.contains("jdoe"));
    }

    #[test]
    fn main_top_left_centers_on_the_splash_center() {
        assert_eq!(
            App::main_top_left(iced::Point::new(702.0, 344.0)),
            iced::Point::new(320.0, 140.0)
        );
        assert_eq!(
            App::main_top_left(iced::Point::new(1022.0, 604.0)),
            iced::Point::new(640.0, 400.0)
        );
    }
}

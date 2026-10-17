use crate::mod_manager::conflict::Conflict;
use crate::mod_manager::icons::LoadedIcon;
use crate::mod_manager::types::{Mod, ModState};
use iced::widget::{button, column, container, row, text};
use iced::{Color, Element, Length};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Browse,
    Conflicts,
    Saves,
    Log,
    Settings,
}

/// Which folder a Browse picker is choosing (Settings tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderTarget {
    /// The game's mods folder — the folder switcher; picking one
    /// also writes the gameSettings.xml override.
    Active,
    /// The archive folder — picker convenience only.
    Archive,
    /// The game's install folder, where game.xml lives.
    Game,
}

/// What the browse grid is showing: everything, one folder's mods,
/// favorites, or (once category parsing lands) one category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseView {
    All,
    Active,
    Archived,
    Favorites,
    Category(String),
}

impl std::fmt::Display for BrowseView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowseView::All => write!(f, "All mods"),
            BrowseView::Active => write!(f, "Active mods"),
            BrowseView::Archived => write!(f, "Archived mods"),
            BrowseView::Favorites => write!(f, "Favorite mods"),
            BrowseView::Category(c) => write!(f, "Category: {}", c),
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
    ScanLoaded(Result<ScanOutcome, String>),
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
    ToggleFavoritesOnly,
    MoveToArchive(String),
    MoveToActive(String),
    RemoveMod(String),
    ActiveDirChanged(String),
    ArchiveDirChanged(String),
    SaveDirectories,
    BrowseFolder(FolderTarget),
    FolderPicked(FolderTarget, Option<PathBuf>),
    ToggleConsole(bool),
    RefreshLog,
    LogLoaded(Result<LogOutcome, String>),
    LogErrorsOnlyToggled(bool),
    /// Update checks (GitHub): the details panel's repo field.
    LinkInputChanged(String),
    SaveModLink(String),
    /// Update checks (ModHub): the details panel's ModHub field.
    ModHubInputChanged(String),
    SaveModHubLink(String),
    /// Auto-detect ModHub links for every unlinked .zip mod.
    DetectModHub,
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
    DetectBatchDone {
        found: Vec<(String, String)>,
        missed: usize,
        processed: usize,
    },
    CheckUpdate(String),
    UpdateChecked(String, Result<Option<update_types::UpdateInfo>, String>),
    InstallUpdate(String),
    UpdateInstalled(String, Result<std::path::PathBuf, String>),
    CheckAllUpdates,
    /// The catalog arrived (or not) for an update check:
    /// stem -> catalog entry, so unlinked mods the catalog
    /// knows can be checked locally with no ModHub traffic.
    CheckUpdatesCatalogReady(
        Vec<(String, crate::sources::mod_hub::CatalogEntry)>,
    ),
    AllUpdatesChecked(Vec<(String, Result<Option<update_types::UpdateInfo>, String>)>),
    SelectSaveSlot(String),
    DraftToggle(String),
    DraftUseActive,
    DraftEmpty,
    SaveLoadout,
    ApplyLoadout,
    SavesFilterChanged(String),
    WindowResized(iced::Size),
    Exit,
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
        background: Some(iced::Background::Color(Color::from_rgba(
            0.05, 0.05, 0.06, 0.85,
        ))),
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
    .spacing(3);
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
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(if is_selected {
                Color::from_rgb(0.20, 0.21, 0.24)
            } else {
                Color::from_rgb(0.13, 0.14, 0.16)
            })),
            border: iced::Border {
                color: if is_selected {
                    Color::from_rgb(0.82, 0.83, 0.87)
                } else if is_favorite {
                    Color::from_rgb(0.58, 0.59, 0.63)
                } else {
                    Color::from_rgb(0.30, 0.31, 0.34)
                },
                width: if is_selected || is_favorite { 2.0 } else { 1.0 },
                radius: 8.0.into(),
            },
            ..container::Style::default()
        });

    iced::widget::mouse_area(card)
        .on_release(Message::SelectMod(mod_data.id.clone()))
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

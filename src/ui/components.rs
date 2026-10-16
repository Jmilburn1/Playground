use crate::mod_manager::types::{Mod, ModState};
use iced::widget::{button, column, container, row, text};
use iced::{Color, Element, Length};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Browse,
    Conflicts,
    Settings,
}

#[derive(Debug, Clone)]
pub enum Message {
    Scan,
    SearchChanged(String),
    TabSelected(Tab),
    SelectMod(String),
    CloseDetails,
    ToggleFavorite(String),
    ToggleFavoritesOnly,
    MoveToArchive(String),
    MoveToActive(String),
    RemoveMod(String),
    ActiveDirChanged(String),
    ArchiveDirChanged(String),
    SaveDirectories,
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

fn state_color(state: &ModState) -> Color {
    match state {
        ModState::Active => Color::from_rgb(0.35, 0.85, 0.45),
        ModState::Archived => Color::from_rgb(0.95, 0.75, 0.35),
        ModState::Conflict => Color::from_rgb(0.95, 0.4, 0.35),
        ModState::Error => Color::from_rgb(0.95, 0.4, 0.35),
    }
}

/// One mod card for the browse grid. The whole card is the click target for
/// opening the details fly-out. Favorites and conflicts get plain-text
/// labels beneath the title.
pub fn mod_card<'a>(
    mod_data: &'a Mod,
    icon: Option<&iced::widget::image::Handle>,
    is_favorite: bool,
    is_selected: bool,
    has_conflict: bool,
    width: f32,
    height: f32,
) -> Element<'a, Message> {
    // Fixed-size icon block (John's spec: about half the old full-width
    // strip, at least double its 58px height). FS icons are square, so a
    // 120x120 block fits them without the old dynamic stretching. The
    // monogram fallback uses the same fixed size so cards stay uniform.
    // (Symbol glyphs like stars are avoided in this UI — they render as
    // boxes in the default font on Windows, so favorites/conflicts use
    // plain text labels instead.)
    const ICON_SIZE: f32 = 120.0;
    let icon_box: Element<'a, Message> = match icon {
        Some(handle) => iced::widget::image(handle.clone())
            .width(ICON_SIZE)
            .height(ICON_SIZE)
            .content_fit(iced::ContentFit::Cover)
            .into(),
        None => container(text(initials(&mod_data.name)).size(26))
            .width(ICON_SIZE)
            .height(ICON_SIZE)
            .center_x(ICON_SIZE)
            .center_y(ICON_SIZE)
            .style(container::bordered_box)
            .into(),
    };
    let icon_area: Element<'a, Message> = container(icon_box)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into();

    let mut lines = column![
        icon_area,
        text(mod_data.name.clone()).size(15),
        row![
            text(format!("v{}", mod_data.version)).size(12),
            text(format!("{:?}", mod_data.state))
                .size(12)
                .color(state_color(&mod_data.state)),
        ]
        .spacing(8),
    ]
    .spacing(6);

    if let Some(author) = &mod_data.author {
        lines = lines.push(text(format!("by {}", author)).size(12));
    }
    if is_favorite {
        lines = lines.push(
            text("Favorite")
                .size(12)
                .color(Color::from_rgb(0.95, 0.8, 0.35)),
        );
    }
    if has_conflict {
        lines = lines.push(
            text("Has conflicts")
                .size(12)
                .color(Color::from_rgb(0.95, 0.45, 0.35)),
        );
    }

    let style = if is_selected {
        button::primary
    } else {
        button::secondary
    };

    button(lines)
        .width(width)
        .height(height)
        .padding(10)
        .style(style)
        .on_press(Message::SelectMod(mod_data.id.clone()))
        .into()
}

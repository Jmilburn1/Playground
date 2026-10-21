//! Playground's palette (John, 2026-10-21) — the five colors
//! he picked for the app, and the styles built from them.
//! Anything visual that needs one of these colors takes it
//! from here, so the later full theming pass has one place
//! to stand on.
//!
//! No color outside this palette may be introduced by the
//! styles below. Hover and pressed shades are blends OF
//! palette colors (see `mix`), never new hues. ONE
//! sanctioned exception (John, 2026-10-21): DANGER, the
//! warning red on the Rescan pill — red is a meaning, not
//! a palette member: the one heavy action warns and
//! stands out at the same time.
//!
//! Text contrast, by the WCAG luminance numbers:
//! - TEXT (#DFE9ED) on ACCENT (#3E5F6E): ~5.5:1 — the
//!   selected pill's label on its fill.
//! - TEXT_DIM (#ACC3D0) on PANEL (#1D2E34): ~7.7:1 — the
//!   idle pill's label on its fill.
//! Both clear the 4.5:1 bar for ordinary text.

use iced::widget::{button, container, progress_bar};
use iced::{Background, Border, Color, Shadow, Theme};

/// #181A1B — near-black: the deep base (sidebar ground,
/// the splash theme's text-side anchor).
pub const BASE: Color = Color {
    r: 0x18 as f32 / 255.0,
    g: 0x1A as f32 / 255.0,
    b: 0x1B as f32 / 255.0,
    a: 1.0,
};

/// #1D2E34 — dark petrol: panels, idle pills, the splash
/// bar's track, the sidebar's edge.
pub const PANEL: Color = Color {
    r: 0x1D as f32 / 255.0,
    g: 0x2E as f32 / 255.0,
    b: 0x34 as f32 / 255.0,
    a: 1.0,
};

/// #3E5F6E — steel teal: the accent — progress fill, the
/// selected pill, where hovers lean toward.
pub const ACCENT: Color = Color {
    r: 0x3E as f32 / 255.0,
    g: 0x5F as f32 / 255.0,
    b: 0x6E as f32 / 255.0,
    a: 1.0,
};

/// #ACC3D0 — light steel: secondary text, idle pill labels,
/// the splash's version line.
pub const TEXT_DIM: Color = Color {
    r: 0xAC as f32 / 255.0,
    g: 0xC3 as f32 / 255.0,
    b: 0xD0 as f32 / 255.0,
    a: 1.0,
};

/// #DFE9ED — pale ice: primary text, the splash's counts.
pub const TEXT: Color = Color {
    r: 0xDF as f32 / 255.0,
    g: 0xE9 as f32 / 255.0,
    b: 0xED as f32 / 255.0,
    a: 1.0,
};

/// Blend two palette colors (`t` of the way from `a` to
/// `b`). The only way new shades enter this module.
fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// A fully rounded border: the radius sits far past half
/// of any of these buttons' heights, so the ends close
/// into semicircles whatever height the button ends up —
/// a pill, by construction, not by measurement.
fn pill_border() -> Border {
    Border {
        color: Color::TRANSPARENT,
        width: 0.0,
        radius: 100.0.into(),
    }
}

/// The sidebar's menu pills (John, 2026-10-21: pill-shaped
/// instead of square). Idle is petrol with a light-steel
/// label; hover leans halfway toward the accent and the
/// label comes up to pale ice; the current page's pill is
/// the accent itself with a pale-ice label.
pub fn nav_pill(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let (fill, label) = match (selected, status) {
            (true, button::Status::Hovered) => (mix(ACCENT, TEXT_DIM, 0.18), TEXT),
            (true, _) => (ACCENT, TEXT),
            (false, button::Status::Hovered) => (mix(PANEL, ACCENT, 0.5), TEXT),
            (false, button::Status::Pressed) => (ACCENT, TEXT),
            (false, _) => (PANEL, TEXT_DIM),
        };
        button::Style {
            background: Some(Background::Color(fill)),
            text_color: label,
            border: pill_border(),
            shadow: Shadow::default(),
        }
    }
}

/// #8E2A23 — deep oxide red: NOT a palette color. The one
/// sanctioned exception (John, 2026-10-21), worn only by
/// the Rescan pill: red warns against the heavy action
/// and stands out from the steel-and-petrol scheme at the
/// same time. TEXT on it runs ~6:1 — clears the 4.5:1 bar.
pub const DANGER: Color = Color {
    r: 0x8E as f32 / 255.0,
    g: 0x2A as f32 / 255.0,
    b: 0x23 as f32 / 255.0,
    a: 1.0,
};

/// The sidebar's one action pill (Rescan): the warning
/// red at rest (John, 2026-10-21 — it replaces the accent
/// here; the accent marks where you ARE, red marks what
/// costs you), lightening on hover, settling toward the
/// base when pressed.
pub fn danger_pill() -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let fill = match status {
            button::Status::Hovered => mix(DANGER, TEXT_DIM, 0.25),
            button::Status::Pressed => mix(DANGER, BASE, 0.25),
            _ => DANGER,
        };
        button::Style {
            background: Some(Background::Color(fill)),
            text_color: TEXT,
            border: pill_border(),
            shadow: Shadow::default(),
        }
    }
}

/// The sidebar's ground: the near-black base, edged with
/// petrol where the old style drew a grey border.
pub fn sidebar_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BASE)),
        border: Border {
            color: PANEL,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// The splash's progress bar: steel-teal fill on a petrol
/// track, pill-rounded like the menu it precedes.
pub fn splash_bar(_theme: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: Background::Color(PANEL),
        bar: Background::Color(ACCENT),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 7.0.into(),
        },
    }
}

/// The theme the shell wears while the boot splash is up.
/// Its background is TRANSPARENT, and that is the whole
/// trick: the shell clears the window with the theme's
/// background color (iced_winit builds its Appearance from
/// the extended palette's background base, which for a
/// custom theme is this palette's background as-is), so
/// while this theme holds, every pixel the splash doesn't
/// paint is truly clear and the logo floats on the desktop.
/// The rest of the palette is filled with palette colors
/// only — the splash styles its few widgets explicitly, so
/// these slots are never actually consulted.
pub fn splash_theme() -> Theme {
    Theme::custom(
        "Playground Splash".to_string(),
        iced::theme::Palette {
            background: Color::TRANSPARENT,
            text: TEXT,
            primary: ACCENT,
            success: ACCENT,
            danger: ACCENT,
        },
    )
}

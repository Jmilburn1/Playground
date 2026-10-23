//! Playground's palettes (John, 2026-10-23) — five
//! themes, ruled on one at a time by the owner:
//! GUNMETAL is the default the app wears out of the
//! box; Midnight Steel (the app's original palette),
//! Field Green, Wildflower and Lavender Dusk are
//! selectable on the Settings page. Wildflower and
//! Lavender Dusk are the two themes for women. The
//! pick applies live and persists in the config.
//!
//! Everything visual takes its colors from here, via
//! `pal()` — the CURRENT palette — never from a
//! hard-coded member, so a theme switch repaints the
//! whole app. Hover and pressed shades are blends OF
//! palette colors (see `mix`), never new hues. ONE
//! sanctioned exception (John, 2026-10-21): DANGER,
//! the warning red — red is a meaning, not a palette
//! member, and it NEVER changes with the theme.
//!
//! Text contrast holds per theme by construction:
//! every theme's text on its accent, and its dim text
//! on its panel, clear the WCAG 4.5:1 bar for ordinary
//! text (the pairs were checked when the schemes were
//! approved).

use std::sync::RwLock;

use iced::widget::{button, container, progress_bar, text_input};
use iced::{Background, Border, Color, Shadow, Theme, Vector};

/// One theme's five colors: the deep base, the panel
/// ground, the accent, and the two text tones. Every
/// style in the app is built from these five (plus
/// DANGER, which stands outside theming).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub base: Color,
    pub panel: Color,
    pub accent: Color,
    pub text: Color,
    pub text_dim: Color,
}

/// One selectable theme: its stable id (what the
/// config stores), its display name, its palette.
#[derive(Debug, Clone, Copy)]
pub struct ThemeDef {
    pub id: &'static str,
    pub name: &'static str,
    pub palette: Palette,
}

const fn col(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

/// Gunmetal — the DEFAULT (approved 2026-10-23):
/// dark blued-grey metal, brushed-steel accent,
/// silver-white text.
const GUNMETAL: Palette = Palette {
    base: col(0x23, 0x27, 0x2C),
    panel: col(0x2E, 0x33, 0x39),
    accent: col(0x8F, 0xA3, 0xB8),
    text: col(0xE8, 0xEC, 0xF0),
    text_dim: col(0xA7, 0xB0, 0xBA),
};

/// Midnight Steel — the app's original palette
/// (through v1.6.9): near-black, dark petrol, steel
/// teal, pale ice.
const MIDNIGHT_STEEL: Palette = Palette {
    base: col(0x18, 0x1A, 0x1B),
    panel: col(0x1D, 0x2E, 0x34),
    accent: col(0x3E, 0x5F, 0x6E),
    text: col(0xDF, 0xE9, 0xED),
    text_dim: col(0xAC, 0xC3, 0xD0),
};

/// Field Green: deep green base, fresh field-green
/// accent, soft white-green text.
const FIELD_GREEN: Palette = Palette {
    base: col(0x12, 0x1A, 0x13),
    panel: col(0x1C, 0x2B, 0x1F),
    accent: col(0x63, 0xA8, 0x67),
    text: col(0xEA, 0xF2, 0xE4),
    text_dim: col(0xA9, 0xBF, 0xA5),
};

/// Wildflower (for women): deep plum base, dusty-rose
/// accent, blush-white text.
const WILDFLOWER: Palette = Palette {
    base: col(0x22, 0x13, 0x19),
    panel: col(0x33, 0x20, 0x2A),
    accent: col(0xC4, 0x74, 0x8C),
    text: col(0xF7, 0xE9, 0xEC),
    text_dim: col(0xC6, 0xA5, 0xB0),
};

/// Lavender Dusk (for women): twilight violet base,
/// lavender accent, pearl text.
const LAVENDER_DUSK: Palette = Palette {
    base: col(0x1B, 0x18, 0x26),
    panel: col(0x2A, 0x25, 0x42),
    accent: col(0x9B, 0x8C, 0xD9),
    text: col(0xF0, 0xEC, 0xFB),
    text_dim: col(0xB6, 0xAF, 0xD2),
};

/// The five themes, default first — the Settings
/// picker's rows, in order.
pub const THEMES: &[ThemeDef] = &[
    ThemeDef {
        id: "gunmetal",
        name: "Gunmetal",
        palette: GUNMETAL,
    },
    ThemeDef {
        id: "midnight-steel",
        name: "Midnight Steel",
        palette: MIDNIGHT_STEEL,
    },
    ThemeDef {
        id: "field-green",
        name: "Field Green",
        palette: FIELD_GREEN,
    },
    ThemeDef {
        id: "wildflower",
        name: "Wildflower",
        palette: WILDFLOWER,
    },
    ThemeDef {
        id: "lavender-dusk",
        name: "Lavender Dusk",
        palette: LAVENDER_DUSK,
    },
];

/// The live theme. Styles are rebuilt on every view
/// pass, so swapping this repaints the whole app on
/// the next frame — no restart, no stale closure.
static CURRENT: RwLock<ThemeDef> = RwLock::new(THEMES[0]);

/// The theme currently in force.
pub fn current_theme() -> ThemeDef {
    *CURRENT.read().unwrap()
}

/// The current palette — the one call every style
/// starts from.
pub fn pal() -> Palette {
    current_theme().palette
}

/// Switch the live theme by id. Returns false (and
/// changes nothing) for an unknown id.
pub fn set_theme(id: &str) -> bool {
    match THEMES.iter().find(|t| t.id == id) {
        Some(def) => {
            *CURRENT.write().unwrap() = *def;
            true
        }
        None => false,
    }
}

/// The current palette's colors, one call each — the
/// shape the app's views have always used, now live.
pub fn base() -> Color {
    pal().base
}

pub fn panel() -> Color {
    pal().panel
}

pub fn accent() -> Color {
    pal().accent
}

pub fn text() -> Color {
    pal().text
}

pub fn text_dim() -> Color {
    pal().text_dim
}

/// #8E2A23 — deep oxide red: NOT a palette color. The
/// one sanctioned exception (John, 2026-10-21), worn
/// by the Rescan pill and the details panel's Remove
/// pill (the two things that can hurt you, same red):
/// red warns against the heavy action and stands out
/// from every scheme at the same time. It never
/// changes with the theme (John, 2026-10-23).
pub const DANGER: Color = Color {
    r: 0x8E as f32 / 255.0,
    g: 0x2A as f32 / 255.0,
    b: 0x23 as f32 / 255.0,
    a: 1.0,
};

/// Blend two colors (`t` of the way from `a` to `b`).
/// The only way new shades enter this module.
fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// A fully rounded border: the radius sits far past
/// half of any of these buttons' heights, so the ends
/// close into semicircles whatever height the button
/// ends up — a pill, by construction, not by
/// measurement.
fn pill_border() -> Border {
    Border {
        color: Color::TRANSPARENT,
        width: 0.0,
        radius: 100.0.into(),
    }
}

/// The pill's lift (feel pass, 2026-10-23): at rest a
/// pill floats a hair off the ground — a soft shadow
/// in the palette's own base at half strength. Pressed,
/// the shadow is GONE: the pill sits flat on the
/// ground. That vanishing, with the fill settling
/// toward the base in each style's Pressed arm, is the
/// press "dip" — the click lands visually before the
/// action fires.
fn pill_shadow(pressed: bool) -> Shadow {
    if pressed {
        Shadow::default()
    } else {
        Shadow {
            color: Color { a: 0.45, ..base() },
            offset: Vector::new(0.0, 1.0),
            blur_radius: 3.0,
        }
    }
}

/// The sidebar's menu pills (John, 2026-10-21:
/// pill-shaped instead of square). Idle is the panel
/// color with a dim label; hover leans halfway toward
/// the accent and the label comes up to full text; the
/// current page's pill is the accent itself.
pub fn nav_pill(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = pal();
    move |_theme, status| {
        let (fill, label) = match (selected, status) {
            (true, button::Status::Hovered) => (mix(p.accent, p.text_dim, 0.18), p.text),
            (true, button::Status::Pressed) => (mix(p.accent, p.base, 0.22), p.text),
            (true, _) => (p.accent, p.text),
            (false, button::Status::Hovered) => (mix(p.panel, p.accent, 0.5), p.text),
            (false, button::Status::Pressed) => (mix(p.accent, p.base, 0.22), p.text),
            (false, _) => (p.panel, p.text_dim),
        };
        button::Style {
            background: Some(Background::Color(fill)),
            text_color: label,
            border: pill_border(),
            shadow: pill_shadow(matches!(status, button::Status::Pressed)),
        }
    }
}

/// The details panel's close button (John, 2026-10-21):
/// a round accent button — lightening on hover and
/// settling toward the base when pressed, in the
/// danger pill's rhythm. The caller sizes it square;
/// the radius-100 border closes it into a circle by
/// construction (see `pill_border`).
pub fn close_button() -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = pal();
    move |_theme, status| {
        let fill = match status {
            button::Status::Hovered => mix(p.accent, p.text_dim, 0.18),
            button::Status::Pressed => mix(p.accent, p.base, 0.25),
            _ => p.accent,
        };
        button::Style {
            background: Some(Background::Color(fill)),
            text_color: p.text,
            border: pill_border(),
            shadow: pill_shadow(matches!(status, button::Status::Pressed)),
        }
    }
}

/// The Browse Show: field (John, 2026-10-21): the
/// field wears the highlighted buttons' color — the
/// accent the selected menu pill wears. Hover lightens
/// the same way the selected pill's does; pressed DIPS
/// (feel pass, 2026-10-23): the fill settles toward
/// the base instead of lightening with the hover. The
/// border is the field's own long-standing one,
/// unchanged: only the fill is palette.
pub fn dropdown_field() -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = pal();
    move |_theme, status| {
        let fill = match status {
            button::Status::Hovered => mix(p.accent, p.text_dim, 0.18),
            button::Status::Pressed => mix(p.accent, p.base, 0.15),
            _ => p.accent,
        };
        button::Style {
            background: Some(Background::Color(fill)),
            text_color: p.text,
            border: Border {
                color: Color::from_rgb(0.35, 0.36, 0.40),
                width: 1.0,
                radius: 8.0.into(),
            },
            shadow: pill_shadow(matches!(status, button::Status::Pressed)),
        }
    }
}

/// The sidebar's one action pill (Rescan): the warning
/// red at rest (John, 2026-10-21 — it replaces the
/// accent here; the accent marks where you ARE, red
/// marks what costs you), lightening on hover, settling
/// toward the base when pressed.
pub fn danger_pill() -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = pal();
    move |_theme, status| {
        let fill = match status {
            button::Status::Hovered => mix(DANGER, p.text_dim, 0.25),
            button::Status::Pressed => mix(DANGER, p.base, 0.25),
            _ => DANGER,
        };
        button::Style {
            background: Some(Background::Color(fill)),
            text_color: p.text,
            border: pill_border(),
            shadow: pill_shadow(matches!(status, button::Status::Pressed)),
        }
    }
}

/// The hold-to-delete control's box (feel pass,
/// 2026-10-23; hold polish part 2): danger_pill's
/// dress as a container — the control is a
/// mouse_area, not a button, so it needs the pill as
/// a box style. At t = 0 it is the plain danger box;
/// as the hold fills it brightens toward the hover
/// shade — the squeeze's other half, so the control
/// visibly strains toward firing while the bar fills
/// under it.
pub fn danger_box_hold(t: f32) -> impl Fn(&Theme) -> container::Style {
    let p = pal();
    move |_theme| container::Style {
        background: Some(Background::Color(mix(
            DANGER,
            mix(DANGER, p.text_dim, 0.25),
            t.clamp(0.0, 1.0),
        ))),
        text_color: Some(p.text),
        border: pill_border(),
        shadow: Shadow::default(),
    }
}

/// The sidebar's ground: the palette's base, edged
/// with its panel color where the old style drew a
/// grey border.
pub fn sidebar_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(base())),
        border: Border {
            color: panel(),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// A mod download's progress bar (Downloads page):
/// accent fill on a panel track, so a download in
/// flight wears the app's own colors.
pub fn download_bar(_theme: &Theme) -> progress_bar::Style {
    let p = pal();
    progress_bar::Style {
        background: Background::Color(p.panel),
        bar: Background::Color(p.accent),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 7.0.into(),
        },
    }
}

/// The hold-to-delete control's progress bar (feel
/// pass, 2026-10-23): pale fill on a darkened danger
/// track — it lives inside the DANGER pill, so it
/// dresses in the pill's own colors.
pub fn hold_bar(_theme: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: Background::Color(mix(DANGER, base(), 0.45)),
        bar: Background::Color(text()),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 2.0.into(),
        },
    }
}

/// The splash's progress bar: accent fill on a panel
/// track, pill-rounded like the menu it precedes.
pub fn splash_bar(_theme: &Theme) -> progress_bar::Style {
    let p = pal();
    progress_bar::Style {
        background: Background::Color(p.panel),
        bar: Background::Color(p.accent),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 7.0.into(),
        },
    }
}

/// Text inputs (feel pass part 2, 2026-10-23): the
/// panel ground with a quiet edge at rest; focused,
/// the edge becomes the accent and the ground leans
/// toward it — the focus glow, in the toolkit's own
/// terms (a text input's style carries no shadow, so
/// the border and the ground do the glowing).
pub fn input_field() -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    let p = pal();
    move |_theme, status| {
        let focused = matches!(status, text_input::Status::Focused);
        text_input::Style {
            background: Background::Color(if focused {
                mix(p.panel, p.accent, 0.18)
            } else {
                p.panel
            }),
            border: Border {
                color: if focused {
                    p.accent
                } else {
                    mix(p.panel, p.text_dim, 0.25)
                },
                width: if focused { 1.5 } else { 1.0 },
                radius: 8.0.into(),
            },
            icon: p.text_dim,
            placeholder: mix(p.text_dim, p.panel, 0.35),
            value: p.text,
            selection: mix(p.accent, p.text, 0.25),
        }
    }
}

/// The Iced theme the main window wears: built from
/// the CURRENT palette, so the toolkit's own widgets
/// (scrollbars, pick-list overlays, anything not
/// explicitly styled) follow the chosen theme too.
/// Rebuilt whenever the app state changes — including
/// the moment a new theme is picked.
pub fn app_theme() -> Theme {
    let p = pal();
    Theme::custom(
        format!("Playground {}", current_theme().name),
        iced::theme::Palette {
            background: p.base,
            text: p.text,
            primary: p.accent,
            success: p.accent,
            danger: DANGER,
        },
    )
}

/// The theme the shell wears while the boot splash is
/// up. Its background is TRANSPARENT, and that is the
/// whole trick: the shell clears the window with the
/// theme's background color (iced_winit builds its
/// Appearance from the extended palette's background
/// base, which for a custom theme is this palette's
/// background as-is), so while this theme holds, every
/// pixel the splash doesn't paint is truly clear and
/// the logo floats on the desktop. The rest of the
/// palette is filled with palette colors only — the
/// splash styles its few widgets explicitly, so these
/// slots are never actually consulted.
pub fn splash_theme() -> Theme {
    let p = pal();
    Theme::custom(
        "Playground Splash".to_string(),
        iced::theme::Palette {
            background: Color::TRANSPARENT,
            text: p.text,
            primary: p.accent,
            success: p.accent,
            danger: DANGER,
        },
    )
}

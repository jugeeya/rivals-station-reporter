//! The app's design tokens — a 1:1 port of the Vue app's CSS custom
//! properties (src/styles/global.scss), so the native UI reads as the same
//! product. Names match the CSS variables they replace.

use iced::border::Radius;
use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Shadow, Theme};

pub const COLOR_BG: Color = Color::from_rgb(
    0x0e as f32 / 255.0,
    0x0c as f32 / 255.0,
    0x24 as f32 / 255.0,
);

const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

pub const SURFACE: Color = rgba(179, 179, 179, 0.10);
pub const SURFACE_HOVER: Color = rgba(255, 255, 255, 0.15);
pub const SURFACE_INSET: Color = rgba(0, 0, 0, 0.28);
pub const SURFACE_SOLID: Color = rgba(0x1e, 0x1d, 0x32, 1.0);
pub const ACCENT: Color = rgba(99, 102, 241, 0.80);
pub const ACCENT_HOVER: Color = rgba(132, 134, 252, 0.8);
pub const ACCENT_COMPLETED: Color = rgba(163, 164, 231, 0.25);
pub const TEXT_PRIMARY: Color = Color::WHITE;
pub const TEXT_MUTED: Color = rgba(255, 255, 255, 0.6);
pub const TEXT_SUCCESS: Color = rgba(0x00, 0xff, 0xaa, 1.0);
pub const TEXT_WARNING: Color = rgba(0xfa, 0xcc, 0x15, 1.0);
pub const TEXT_FAILURE: Color = rgba(0xf8, 0x71, 0x71, 1.0);
pub const LINE: Color = rgba(255, 255, 255, 0.08);
pub const LINE_SUBTLE: Color = rgba(255, 255, 255, 0.04);
pub const LINE_DIVIDER: Color = rgba(255, 255, 255, 0.05);
/// The bracket's feeder lines. Brighter than the hairlines above — they carry
/// meaning (which set feeds which) rather than just separating things.
pub const LINE_CONNECTOR: Color = rgba(255, 255, 255, 0.18);

// The CSS radii are em-based (1em/0.5em at 16px root).
pub const RADIUS_CARD: f32 = 16.0;
pub const RADIUS_PANEL: f32 = 8.0;
pub const RADIUS_BUTTON: f32 = 8.0;

// ---- fonts -----------------------------------------------------------------
// Space Grotesk is the display face; Inter the body face; Ubuntu Sans Mono
// for code-ish text (log, hub URL) — same stack as the web app. STATIC
// weight instances from each font's official upstream, not the variable
// files: iced's text stack renders variable fonts at their default instance
// with visibly wrong spacing (the "bad kerning" on early screenshots), while
// statics shape exactly like the browser did.

use iced::font::Weight;

pub const FONT_DISPLAY: iced::Font = iced::Font {
    weight: Weight::Bold,
    ..iced::Font::with_name("Space Grotesk")
};
pub const FONT_DISPLAY_MEDIUM: iced::Font = iced::Font {
    weight: Weight::Medium,
    ..iced::Font::with_name("Space Grotesk")
};
pub const FONT_BODY: iced::Font = iced::Font::with_name("Inter");
pub const FONT_BODY_MEDIUM: iced::Font = iced::Font {
    weight: Weight::Medium,
    ..iced::Font::with_name("Inter")
};
pub const FONT_BODY_SEMIBOLD: iced::Font = iced::Font {
    weight: Weight::Semibold,
    ..iced::Font::with_name("Inter")
};
pub const FONT_BODY_BOLD: iced::Font = iced::Font {
    weight: Weight::Bold,
    ..iced::Font::with_name("Inter")
};
pub const FONT_MONO: iced::Font = iced::Font::with_name("Ubuntu Sans Mono");
/// Material Symbols Outlined, subset to just the glyphs below (2.6 KB).
/// Regenerate with fonttools if a new icon is needed: instance the variable
/// font at FILL=0 GRAD=0 opsz=24 wght=400, then pyftsubset to the codepoints.
pub const FONT_ICONS: iced::Font = iced::Font::with_name("Material Symbols Outlined");

pub const ICON_BRACKET: &str = "\u{e97a}"; // account_tree
pub const ICON_MATCHES: &str = "\u{e8ef}"; // view_list
pub const ICON_TAGS: &str = "\u{f05b}"; // sell
pub const ICON_SPLIT: &str = "\u{e14e}"; // content_cut
pub const ICON_LOG: &str = "\u{eb8e}"; // terminal
pub const ICON_SETTINGS: &str = "\u{e8b8}"; // settings
pub const ICON_REFRESH: &str = "\u{e5d5}"; // refresh

pub const FONT_BYTES: [&[u8]; 8] = [
    include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf"),
    include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf"),
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../assets/fonts/UbuntuSansMono-Regular.ttf"),
    include_bytes!("../assets/fonts/MaterialSymbolsSubset.ttf"),
];

// ---- widget styles ----------------------------------------------------------

/// The big glassy card every view sits on (.card in the web app).
pub fn card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: Radius::new(RADIUS_CARD),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
            offset: iced::Vector::new(0.0, 18.0),
            blur_radius: 48.0,
        },
        text_color: Some(TEXT_PRIMARY),
        ..container::Style::default()
    }
}

/// An inset panel within a card (.panel / --surface-inset).
pub fn panel(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_INSET)),
        border: Border {
            color: LINE_SUBTLE,
            width: 1.0,
            radius: Radius::new(RADIUS_PANEL),
        },
        text_color: Some(TEXT_PRIMARY),
        ..container::Style::default()
    }
}

/// Primary action button (.btn-primary).
pub fn button_primary(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => ACCENT_HOVER,
        button::Status::Disabled => Color { a: 0.35, ..ACCENT },
        _ => ACCENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: if matches!(status, button::Status::Disabled) {
            TEXT_MUTED
        } else {
            TEXT_PRIMARY
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::new(RADIUS_BUTTON),
        },
        ..button::Style::default()
    }
}

/// Secondary button (.btn): surface chip that lightens on hover.
pub fn button_surface(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => SURFACE_HOVER,
        _ => SURFACE,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: if matches!(status, button::Status::Disabled) {
            TEXT_MUTED
        } else {
            TEXT_PRIMARY
        },
        border: Border {
            color: LINE,
            width: 1.0,
            radius: Radius::new(RADIUS_BUTTON),
        },
        ..button::Style::default()
    }
}

/// The active half of the Bracket/Matches view toggle: a surface chip whose
/// accent border says "you are here" without shouting like a primary button.
pub fn button_tab_active(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(SURFACE_HOVER)),
        text_color: TEXT_PRIMARY,
        border: Border {
            color: Color { a: 0.7, ..ACCENT },
            width: 1.0,
            radius: Radius::new(RADIUS_BUTTON),
        },
        ..button::Style::default()
    }
}

/// Text-like button (.linkish): no chrome, accent on hover.
pub fn button_linkish(_theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => TEXT_PRIMARY,
            button::Status::Disabled => Color {
                a: 0.35,
                ..TEXT_MUTED
            },
            _ => TEXT_MUTED,
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::new(RADIUS_BUTTON),
        },
        ..button::Style::default()
    }
}

/// Text inputs (.ob-input / settings fields).
pub fn input(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border_color = match status {
        text_input::Status::Focused { .. } => Color { a: 0.6, ..ACCENT },
        _ => LINE,
    };
    text_input::Style {
        background: Background::Color(SURFACE_INSET),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: Radius::new(RADIUS_BUTTON),
        },
        icon: TEXT_MUTED,
        placeholder: Color {
            a: 0.4,
            ..Color::WHITE
        },
        value: TEXT_PRIMARY,
        selection: Color { a: 0.35, ..ACCENT },
    }
}

/// Tooltip bubble — solid surface so it reads over anything.
pub fn tooltip_bubble(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_SOLID)),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: Radius::new(RADIUS_PANEL),
        },
        text_color: Some(TEXT_PRIMARY),
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.4),
            offset: iced::Vector::new(0.0, 6.0),
            blur_radius: 18.0,
        },
        ..container::Style::default()
    }
}

/// The themed destination picker — closed state, matching the web app's
/// DestinationDropdown trigger (surface-inset chip with a subtle border).
pub fn pick_list_style(
    _theme: &Theme,
    status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
    use iced::widget::pick_list::Status;
    iced::widget::pick_list::Style {
        text_color: TEXT_PRIMARY,
        placeholder_color: TEXT_MUTED,
        handle_color: TEXT_MUTED,
        background: Background::Color(SURFACE_INSET),
        border: Border {
            color: if matches!(status, Status::Hovered | Status::Opened { .. }) {
                Color { a: 0.5, ..ACCENT }
            } else {
                LINE_SUBTLE
            },
            width: 1.0,
            radius: Radius::new(RADIUS_BUTTON),
        },
    }
}

/// The picker's OPEN option list — the whole reason the web app grew its own
/// dropdown widget (the native one couldn't be themed). Here it's just a
/// style: solid dark surface, accent highlight on the selected row.
pub fn pick_list_menu(_theme: &Theme) -> iced::widget::overlay::menu::Style {
    iced::widget::overlay::menu::Style {
        background: Background::Color(SURFACE_SOLID),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: Radius::new(RADIUS_BUTTON),
        },
        text_color: TEXT_PRIMARY,
        selected_text_color: TEXT_PRIMARY,
        selected_background: Background::Color(Color { a: 0.25, ..ACCENT }),
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.4),
            offset: iced::Vector::new(0.0, 8.0),
            blur_radius: 24.0,
        },
    }
}

/// Slide-over drawer panel (settings/log): solid, elevated, left border.
pub fn drawer(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_SOLID)),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: Radius::new(0.0),
        },
        text_color: Some(TEXT_PRIMARY),
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: iced::Vector::new(-12.0, 0.0),
            blur_radius: 40.0,
        },
        ..container::Style::default()
    }
}

// ---- VOD Splitter pieces --------------------------------------------------------

/// A clip whose length says start.gg never closed the set out properly.
pub fn panel_warning(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(vertical_gradient(
            Color {
                a: 0.10,
                ..TEXT_FAILURE
            },
            Color {
                a: 0.03,
                ..TEXT_FAILURE
            },
        )),
        border: Border {
            color: Color {
                a: 0.35,
                ..TEXT_FAILURE
            },
            width: 1.0,
            radius: Radius::new(RADIUS_PANEL),
        },
        text_color: Some(TEXT_PRIMARY),
        ..container::Style::default()
    }
}

/// The numbered step circle in the splitter's setup column.
pub fn step_badge(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: 0.22, ..ACCENT })),
        border: Border {
            color: Color { a: 0.45, ..ACCENT },
            width: 1.0,
            radius: Radius::new(999.0),
        },
        text_color: Some(TEXT_PRIMARY),
        ..container::Style::default()
    }
}

/// Placeholder box where a preview frame hasn't arrived yet.
pub fn thumb_placeholder(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_INSET)),
        border: Border {
            color: LINE_SUBTLE,
            width: 1.0,
            radius: Radius::new(6.0),
        },
        text_color: Some(TEXT_MUTED),
        ..container::Style::default()
    }
}

// ---- typography helpers -------------------------------------------------------

/// Fakes the web app's `letter-spacing: 0.06em` on uppercase section titles:
/// iced's text has no tracking, but a U+2009 THIN SPACE between characters
/// reads the same at these sizes.
pub fn tracked(s: &str) -> String {
    let upper = s.to_uppercase();
    let mut out = String::with_capacity(upper.len() * 2);
    for (i, ch) in upper.chars().enumerate() {
        if i > 0 {
            out.push('\u{2009}');
        }
        out.push(ch);
    }
    out
}

// ---- richer surfaces -----------------------------------------------------------

fn vertical_gradient(top: Color, bottom: Color) -> Background {
    Background::Gradient(
        iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
            .add_stop(0.0, top)
            .add_stop(1.0, bottom)
            .into(),
    )
}

/// The glassy main card, take two: without backdrop-blur (not a thing
/// outside a browser), a soft vertical gradient + a brighter hairline reads
/// as the same material.
pub fn card_rich(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(vertical_gradient(
            rgba(214, 216, 255, 0.085),
            rgba(150, 150, 210, 0.035),
        )),
        border: Border {
            color: rgba(255, 255, 255, 0.10),
            width: 1.0,
            radius: Radius::new(RADIUS_CARD),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
            offset: iced::Vector::new(0.0, 24.0),
            blur_radius: 64.0,
        },
        text_color: Some(TEXT_PRIMARY),
        ..container::Style::default()
    }
}

/// The live "Now playing" card: the accent bleeds into the panel itself.
pub fn panel_live(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(vertical_gradient(
            Color { a: 0.13, ..ACCENT },
            Color { a: 0.04, ..ACCENT },
        )),
        border: Border {
            color: Color { a: 0.35, ..ACCENT },
            width: 1.0,
            radius: Radius::new(RADIUS_PANEL),
        },
        text_color: Some(TEXT_PRIMARY),
        ..container::Style::default()
    }
}

// ---- bracket set cards ------------------------------------------------------
// Every node in the drawn bracket is a button (clicking one selects it), so
// these are button styles rather than container ones. The four variants are
// the only states a TO needs to tell apart at a glance across a whole tree:
// waiting, playing, done, and the one currently selected.

fn set_card(
    bg: Option<Background>,
    border: Color,
    width: f32,
    status: button::Status,
) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: if hovered {
            Some(Background::Color(SURFACE_HOVER))
        } else {
            bg
        },
        text_color: TEXT_PRIMARY,
        border: Border {
            color: border,
            width,
            radius: Radius::new(RADIUS_PANEL),
        },
        ..button::Style::default()
    }
}

/// Not played yet — the default node.
pub fn bracket_set(_theme: &Theme, status: button::Status) -> button::Style {
    set_card(Some(Background::Color(SURFACE_INSET)), LINE, 1.0, status)
}

/// In progress on start.gg right now. Reads the same as the "Now playing"
/// card on the main screen, deliberately.
pub fn bracket_set_live(_theme: &Theme, status: button::Status) -> button::Style {
    set_card(
        Some(vertical_gradient(
            Color { a: 0.16, ..ACCENT },
            Color { a: 0.05, ..ACCENT },
        )),
        Color { a: 0.45, ..ACCENT },
        1.0,
        status,
    )
}

/// Finished. Recedes so the unplayed part of the bracket is what stands out.
pub fn bracket_set_done(_theme: &Theme, status: button::Status) -> button::Style {
    set_card(
        Some(Background::Color(rgba(0, 0, 0, 0.14))),
        LINE_SUBTLE,
        1.0,
        status,
    )
}

/// Both seats filled, nobody has called it: the set a TO should be handing
/// to a free setup next. Warm rather than accent-coloured so it reads as
/// "needs you" against the live set's indigo, and so the two are still
/// distinguishable at a glance across a whole tree.
pub fn bracket_set_ready(_theme: &Theme, status: button::Status) -> button::Style {
    set_card(
        Some(vertical_gradient(
            Color {
                a: 0.13,
                ..TEXT_WARNING
            },
            Color {
                a: 0.04,
                ..TEXT_WARNING
            },
        )),
        Color {
            a: 0.40,
            ..TEXT_WARNING
        },
        1.0,
        status,
    )
}

/// The selected node, whatever its state — a heavier accent ring, so it stays
/// findable after the action bar below has pushed the tree around.
pub fn bracket_set_selected(_theme: &Theme, status: button::Status) -> button::Style {
    set_card(
        Some(Background::Color(Color { a: 0.22, ..ACCENT })),
        ACCENT_HOVER,
        2.0,
        status,
    )
}

/// Primary button with depth: accent gradient + a soft accent glow.
pub fn button_primary_rich(_theme: &Theme, status: button::Status) -> button::Style {
    let (top, bottom) = match status {
        button::Status::Hovered | button::Status::Pressed => {
            (rgba(140, 143, 255, 0.95), rgba(112, 114, 245, 0.95))
        }
        button::Status::Disabled => (Color { a: 0.30, ..ACCENT }, Color { a: 0.25, ..ACCENT }),
        _ => (rgba(118, 121, 250, 0.95), rgba(88, 91, 233, 0.95)),
    };
    button::Style {
        background: Some(vertical_gradient(top, bottom)),
        text_color: if matches!(status, button::Status::Disabled) {
            TEXT_MUTED
        } else {
            TEXT_PRIMARY
        },
        border: Border {
            color: rgba(255, 255, 255, 0.14),
            width: 1.0,
            radius: Radius::new(RADIUS_BUTTON),
        },
        shadow: Shadow {
            color: Color {
                a: if matches!(status, button::Status::Disabled) {
                    0.0
                } else {
                    0.35
                },
                ..ACCENT
            },
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 14.0,
        },
        ..button::Style::default()
    }
}

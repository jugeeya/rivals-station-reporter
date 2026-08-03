//! The app's design tokens — a 1:1 port of the Vue app's CSS custom
//! properties (src/styles/global.scss), so the native UI reads as the same
//! product. Names match the CSS variables they replace.

use iced::border::Radius;
use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Shadow, Theme};

pub const COLOR_BG: Color = Color::from_rgb(0x0e as f32 / 255.0, 0x0c as f32 / 255.0, 0x24 as f32 / 255.0);

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
pub const FONT_MONO: iced::Font = iced::Font::with_name("Ubuntu Sans Mono");

pub const FONT_BYTES: [&[u8]; 6] = [
    include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf"),
    include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf"),
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/UbuntuSansMono-Regular.ttf"),
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

/// Text-like button (.linkish): no chrome, accent on hover.
pub fn button_linkish(_theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => TEXT_PRIMARY,
            button::Status::Disabled => Color { a: 0.35, ..TEXT_MUTED },
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
        placeholder: Color { a: 0.4, ..Color::WHITE },
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

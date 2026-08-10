//! Character stock icons — the native CharIcon.vue. Icons are embedded from
//! assets/characters/ (see ATTRIBUTION.md there for provenance) and looked up
//! by slugified character name; an unknown character falls back to a text
//! chip, because a future character will ship in station data before its
//! icon lands here, and a hole would be a worse failure mode than a name.

use std::collections::HashMap;
use std::sync::OnceLock;

use iced::widget::{container, image, text, tooltip};
use iced::{Element, Length};

use crate::theme;

macro_rules! icons {
    ($($slug:literal),+ $(,)?) => {
        [$(($slug, include_bytes!(concat!("../../assets/characters/", $slug, ".png")).as_slice())),+]
    };
}

fn table() -> &'static HashMap<&'static str, image::Handle> {
    static TABLE: OnceLock<HashMap<&'static str, image::Handle>> = OnceLock::new();
    TABLE.get_or_init(|| {
        icons![
            "absa",
            "clairen",
            "etalus",
            "fleet",
            "forsburn",
            "galvan",
            "gouie",
            "kragg",
            "la-reina",
            "loxodont",
            "maypul",
            "olympia",
            "orcane",
            "ranno",
            "slade",
            "wrastor",
            "zetterburn",
        ]
        .into_iter()
        .map(|(slug, bytes)| (slug, image::Handle::from_bytes(bytes)))
        .collect()
    })
}

fn slugify(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

/// The icon (or text fallback), wrapped in a tooltip naming the character.
pub fn char_icon<'a, M: 'a>(character: Option<&str>, size: f32) -> Element<'a, M>
where
    M: Clone,
{
    char_icon_opacity(character, size, 1.0)
}

/// Same, at reduced opacity — the game strip dims the losing side.
pub fn char_icon_opacity<'a, M: 'a>(
    character: Option<&str>,
    size: f32,
    opacity: f32,
) -> Element<'a, M>
where
    M: Clone,
{
    let name = character.map(str::trim).filter(|s| !s.is_empty());
    let inner: Element<'a, M> = match name.and_then(|n| table().get(slugify(n).as_str())) {
        Some(handle) => image(handle.clone())
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .opacity(opacity)
            .into(),
        None => container(
            text(name.unwrap_or("?").to_string())
                .size((size * 0.4).max(8.0))
                .color(theme::TEXT_MUTED),
        )
        .style(theme::panel)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .center(Length::Fixed(size))
        .clip(true)
        .into(),
    };
    match name {
        Some(n) => tooltip(
            inner,
            container(text(n.to_string()).size(12))
                .style(theme::tooltip_bubble)
                .padding(6),
            tooltip::Position::Top,
        )
        .into(),
        None => inner,
    }
}

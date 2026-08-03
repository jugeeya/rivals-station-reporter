//! The app backdrop: the deep-indigo base with two soft "bloom" glows, a
//! port of the Vue app's `.bg` + `.bloom--a/--b` divs (blurred radial
//! gradients). Canvas draws each bloom as a stack of concentric circles with
//! falling alpha — visually equivalent to a blurred radial gradient without
//! needing one.

use iced::widget::canvas;
use iced::{mouse, Color, Element, Length, Point, Rectangle, Renderer, Theme};

use crate::theme;

pub fn backdrop<Message: 'static>() -> Element<'static, Message> {
    canvas(Backdrop)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

struct Backdrop;

impl<Message> canvas::Program<Message> for Backdrop {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::COLOR_BG);

        // Matches the web app's blooms: indigo top-left, violet bottom-right.
        bloom(
            &mut frame,
            Point::new(bounds.width * 0.18, bounds.height * 0.16),
            bounds.width.max(bounds.height) * 0.45,
            Color::from_rgba(99.0 / 255.0, 102.0 / 255.0, 241.0 / 255.0, 0.16),
        );
        bloom(
            &mut frame,
            Point::new(bounds.width * 0.85, bounds.height * 0.9),
            bounds.width.max(bounds.height) * 0.5,
            Color::from_rgba(168.0 / 255.0, 85.0 / 255.0, 247.0 / 255.0, 0.12),
        );

        vec![frame.into_geometry()]
    }
}

fn bloom(frame: &mut canvas::Frame, center: Point, radius: f32, color: Color) {
    // 24 rings, alpha easing out quadratically — reads as one soft glow.
    const STEPS: usize = 24;
    for i in 0..STEPS {
        let t = i as f32 / STEPS as f32;
        let r = radius * (1.0 - t);
        let ease = (1.0 - t) * (1.0 - t);
        let a = color.a * (1.0 - ease) / STEPS as f32 * 2.2;
        frame.fill(
            &canvas::Path::circle(center, r),
            Color { a, ..color },
        );
    }
}

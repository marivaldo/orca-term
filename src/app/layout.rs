//! App: where each part of the screen goes, by the screen's size (#174). At 150 columns and more
//! the side menu is 34 columns wide, from 100 to 149 it is 28, and under 100 only the centre
//! shows. The cards take every row but the last, which is the status bar; a column of canvas
//! separates the cards and frames the screen.

use ratatui::layout::Rect;

/// Below this width the side menu is hidden.
pub(crate) const SIDE_MENU_MIN_WIDTH: u16 = 100;

/// Below this size nothing but a word that the terminal is too small fits.
const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 10;

/// The parts of one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    /// The side menu's card, when the screen is wide enough for it.
    pub(crate) side_menu: Option<Rect>,
    /// The centre's card, which holds the agent's panel.
    pub(crate) centre: Rect,
    /// The agent's terminal inside the centre's card: under its tab row and a blank row, two
    /// columns in from either side, above its bottom edge.
    pub(crate) body: Rect,
    /// The last row.
    pub(crate) status: Rect,
    /// Whether the screen is too small to draw anything else.
    pub(crate) too_small: bool,
}

/// The parts of a screen of `area`'s size.
pub(crate) fn layout(area: Rect) -> Layout {
    let (w, h) = (area.width, area.height);
    let status = Rect::new(0, h.saturating_sub(1), w, 1);
    if w < MIN_WIDTH || h < MIN_HEIGHT {
        return Layout {
            side_menu: None,
            centre: Rect::default(),
            body: Rect::default(),
            status,
            too_small: true,
        };
    }
    let cards = h - 1;
    let side_width = match w {
        150.. => Some(34),
        SIDE_MENU_MIN_WIDTH..150 => Some(28),
        _ => None,
    };
    let side_menu = side_width.map(|width| Rect::new(1, 0, width, cards));
    let centre_x = side_width.map_or(1, |width| width + 2);
    let centre = Rect::new(centre_x, 0, w - centre_x - 1, cards);
    let body = Rect::new(
        centre.x + 2,
        centre.y + 3,
        centre.width - 4,
        centre.height - 4,
    );
    Layout {
        side_menu,
        centre,
        body,
        status,
        too_small: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_side_menu_narrows_then_goes_under_100_columns() {
        let wide = layout(Rect::new(0, 0, 180, 48));
        assert_eq!(wide.side_menu, Some(Rect::new(1, 0, 34, 47)));
        assert_eq!(wide.centre, Rect::new(36, 0, 143, 47));
        assert_eq!(wide.body, Rect::new(38, 3, 139, 43));
        assert_eq!(wide.status, Rect::new(0, 47, 180, 1));
        let middle = layout(Rect::new(0, 0, 120, 36));
        assert_eq!(middle.side_menu, Some(Rect::new(1, 0, 28, 35)));
        assert_eq!(middle.centre, Rect::new(30, 0, 89, 35));
        let narrow = layout(Rect::new(0, 0, 99, 30));
        assert_eq!(narrow.side_menu, None);
        assert_eq!(narrow.centre, Rect::new(1, 0, 97, 29));
    }

    #[test]
    fn a_tiny_screen_is_too_small() {
        assert!(layout(Rect::new(0, 0, 39, 30)).too_small);
        assert!(layout(Rect::new(0, 0, 80, 9)).too_small);
        assert!(!layout(Rect::new(0, 0, 40, 10)).too_small);
    }
}

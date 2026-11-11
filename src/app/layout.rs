//! App: where each part of the screen goes, by the screen's size and the view (#174, #176, #177).
//! At 150 columns and more the side menu is 34 columns wide and the right column 34 too, from 100
//! to 149 the side menu is 28 and the right column hides, and under 100 only the centre shows. The
//! centre holds one card, or the split's two side by side with a column of canvas between them;
//! zoomed, its one card fills the screen and the side menu and the right column go. Where the right
//! column cannot be a column, `^a f` lays it over the centre's right side. The cards take every row
//! but the last, which is the status bar; a column of canvas separates the cards and frames the
//! screen.

use ratatui::layout::Rect;

use crate::app::view::{Files, Tab, View};

/// Below this width the side menu is hidden.
pub(crate) const SIDE_MENU_MIN_WIDTH: u16 = 100;

/// From this width the side menu is 34 columns wide and the right column shows as a column.
pub(crate) const WIDE: u16 = 150;

/// The right column's width, and the side menu's on a wide screen.
const COLUMN_WIDTH: u16 = 34;

/// Below this size nothing but a word that the terminal is too small fits.
const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 10;

/// One panel's card in the centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Card {
    /// The panel it shows.
    pub(crate) tab: Tab,
    /// The card, its tab row first.
    pub(crate) area: Rect,
    /// The panel's terminal inside the card: under the tab row and a blank row, above the bottom
    /// edge, two columns in from either side (Neovim one, none in the split, so its sign column
    /// sits at the card's edge as in the design).
    pub(crate) body: Rect,
}

/// Where the right column goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RightCard {
    pub(crate) area: Rect,
    /// Whether it lies over the centre instead of beside it.
    pub(crate) overlay: bool,
}

/// The parts of one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout {
    /// The side menu's card, when the screen is wide enough for it and nothing is zoomed.
    pub(crate) side_menu: Option<Rect>,
    /// The right column's card, when it shows.
    pub(crate) right: Option<RightCard>,
    /// Whether the screen is wide enough for the right column as a column, and nothing is zoomed:
    /// `^a f` then shows or hides the column, else the overlay.
    pub(crate) column_fits: bool,
    /// The cards of the panels shown, left to right.
    pub(crate) cards: Vec<Card>,
    /// The agent's terminal where it shows alone in the centre: the size a panel starts at, and
    /// keeps while it is not on screen.
    pub(crate) body: Rect,
    /// The last row.
    pub(crate) status: Rect,
    /// Whether the screen is too small to draw anything else.
    pub(crate) too_small: bool,
}

impl Layout {
    /// The card showing `tab`, when it is on screen.
    pub(crate) fn card(&self, tab: Tab) -> Option<&Card> {
        self.cards.iter().find(|card| card.tab == tab)
    }

    /// Where `tab`'s terminal goes: its card's body when it shows, else [`Layout::body`].
    pub(crate) fn body_of(&self, tab: Tab) -> Rect {
        self.card(tab).map_or(self.body, |card| card.body)
    }
}

/// The parts of a screen of `area`'s size showing `view`.
pub(crate) fn layout(area: Rect, view: View) -> Layout {
    let (w, h) = (area.width, area.height);
    let status = Rect::new(0, h.saturating_sub(1), w, 1);
    if w < MIN_WIDTH || h < MIN_HEIGHT {
        return Layout {
            side_menu: None,
            right: None,
            column_fits: false,
            cards: Vec::new(),
            body: Rect::default(),
            status,
            too_small: true,
        };
    }
    let rows = h - 1;
    let side_width = match w {
        _ if view.zoom => None,
        WIDE.. => Some(COLUMN_WIDTH),
        SIDE_MENU_MIN_WIDTH..WIDE => Some(28),
        _ => None,
    };
    let side_menu = side_width.map(|width| Rect::new(1, 0, width, rows));
    let column_fits = w >= WIDE && !view.zoom;
    let right = right_card(area, column_fits, view.files);
    let column = right.is_some_and(|right| !right.overlay);
    let centre_x = side_width.map_or(1, |width| width + 2);
    let right_width = if column { COLUMN_WIDTH + 1 } else { 0 };
    let centre = Rect::new(centre_x, 0, w - centre_x - 1 - right_width, rows);
    let split = view.split && !view.zoom;
    let cards = match view.shown()[..] {
        [left, right] => {
            let left_width = (centre.width - 1) / 2;
            let left_area = Rect::new(centre.x, 0, left_width, rows);
            let right_area = Rect::new(
                left_area.right() + 1,
                0,
                centre.width - left_width - 1,
                rows,
            );
            vec![card(left, left_area, split), card(right, right_area, split)]
        }
        _ => vec![card(view.tab, centre, split)],
    };
    Layout {
        side_menu,
        right,
        column_fits,
        body: body(Tab::Agent, centre, false),
        cards,
        status,
        too_small: false,
    }
}

/// Where the right column goes on a screen of `area`: on its right as a column when `column_fits`
/// and it is wanted, else there over the centre while the overlay shows, else nowhere.
fn right_card(area: Rect, column_fits: bool, files: Files) -> Option<RightCard> {
    let at = Rect::new(
        area.width - COLUMN_WIDTH - 1,
        0,
        COLUMN_WIDTH,
        area.height - 1,
    );
    if column_fits && files.column {
        return Some(RightCard {
            area: at,
            overlay: false,
        });
    }
    files.overlay.then_some(RightCard {
        area: at,
        overlay: true,
    })
}

fn card(tab: Tab, area: Rect, split: bool) -> Card {
    Card {
        tab,
        area,
        body: body(tab, area, split),
    }
}

/// `tab`'s terminal inside its card `area` (see [`Card::body`]).
fn body(tab: Tab, area: Rect, split: bool) -> Rect {
    let inset = match (tab, split) {
        (Tab::Nvim, true) => 0,
        (Tab::Nvim, false) => 1,
        _ => 2,
    };
    Rect::new(
        area.x + inset,
        area.y + 3,
        area.width.saturating_sub(2 * inset),
        area.height.saturating_sub(4),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alone(tab: Tab) -> View {
        let mut view = View::default();
        view.show(tab);
        view
    }

    #[test]
    fn the_side_menu_narrows_then_goes_under_100_columns() {
        let wide = layout(Rect::new(0, 0, 180, 48), View::default());
        assert_eq!(wide.side_menu, Some(Rect::new(1, 0, 34, 47)));
        assert_eq!(wide.cards[0].area, Rect::new(36, 0, 108, 47));
        assert_eq!(wide.cards[0].body, Rect::new(38, 3, 104, 43));
        assert_eq!(wide.body, wide.cards[0].body);
        assert_eq!(wide.status, Rect::new(0, 47, 180, 1));
        let middle = layout(Rect::new(0, 0, 120, 36), View::default());
        assert_eq!(middle.side_menu, Some(Rect::new(1, 0, 28, 35)));
        assert_eq!(middle.cards[0].area, Rect::new(30, 0, 89, 35));
        assert_eq!(middle.right, None);
        let narrow = layout(Rect::new(0, 0, 99, 30), View::default());
        assert_eq!(narrow.side_menu, None);
        assert_eq!(narrow.cards[0].area, Rect::new(1, 0, 97, 29));
    }

    #[test]
    fn neovim_alone_keeps_one_column_of_margin() {
        let nvim = layout(Rect::new(0, 0, 180, 48), alone(Tab::Nvim));
        assert_eq!(nvim.cards[0].tab, Tab::Nvim);
        assert_eq!(nvim.cards[0].body, Rect::new(37, 3, 106, 43));
        assert_eq!(
            nvim.body_of(Tab::Agent),
            Rect::new(38, 3, 104, 43),
            "off screen"
        );
    }

    #[test]
    fn the_split_halves_the_centre_with_a_column_between() {
        let mut view = View::default();
        view.toggle_split();
        let split = layout(Rect::new(0, 0, 144, 48), view);
        let [agent, nvim] = split.cards[..] else {
            panic!("two cards: {:?}", split.cards);
        };
        assert_eq!((agent.tab, nvim.tab), (Tab::Agent, Tab::Nvim));
        assert_eq!(agent.area, Rect::new(30, 0, 56, 47));
        assert_eq!(nvim.area, Rect::new(87, 0, 56, 47));
        assert_eq!(agent.body, Rect::new(32, 3, 52, 43));
        assert_eq!(
            nvim.body,
            Rect::new(87, 3, 56, 43),
            "the sign column at the edge"
        );
    }

    #[test]
    fn the_zoom_fills_the_screen_and_hides_the_side_menu() {
        let mut view = alone(Tab::Nvim);
        view.toggle_split();
        view.toggle_zoom();
        let zoomed = layout(Rect::new(0, 0, 180, 48), view);
        assert_eq!(zoomed.side_menu, None);
        assert_eq!(zoomed.cards.len(), 1);
        assert_eq!(zoomed.cards[0].area, Rect::new(1, 0, 178, 47));
        assert_eq!(zoomed.cards[0].body, Rect::new(2, 3, 176, 43));
    }

    #[test]
    fn the_right_column_is_a_column_from_150_columns_and_an_overlay_under() {
        let wide = layout(Rect::new(0, 0, 180, 48), View::default());
        let column = RightCard {
            area: Rect::new(145, 0, 34, 47),
            overlay: false,
        };
        assert_eq!(wide.right, Some(column), "x145 w34, as the design draws it");
        assert!(wide.column_fits);
        let narrowest = layout(Rect::new(0, 0, 150, 40), View::default());
        assert_eq!(narrowest.cards[0].area, Rect::new(36, 0, 78, 39));
        assert_eq!(narrowest.right.unwrap().area, Rect::new(115, 0, 34, 39));

        let hidden = View {
            files: Files {
                column: false,
                overlay: false,
            },
            ..View::default()
        };
        let without = layout(Rect::new(0, 0, 180, 48), hidden);
        assert_eq!(without.right, None, "^a f hid it");
        assert_eq!(without.cards[0].area, Rect::new(36, 0, 143, 47));

        let over = View {
            files: Files {
                column: true,
                overlay: true,
            },
            ..View::default()
        };
        let middle = layout(Rect::new(0, 0, 120, 36), over);
        assert!(!middle.column_fits);
        let overlay = RightCard {
            area: Rect::new(85, 0, 34, 35),
            overlay: true,
        };
        assert_eq!(middle.right, Some(overlay));
        assert_eq!(
            middle.cards[0].area,
            Rect::new(30, 0, 89, 35),
            "the centre stays under it"
        );
    }

    #[test]
    fn zoomed_the_right_column_goes_and_only_the_overlay_shows() {
        let mut view = View::default();
        view.toggle_zoom();
        let zoomed = layout(Rect::new(0, 0, 180, 48), view);
        assert_eq!(zoomed.right, None);
        assert!(!zoomed.column_fits);
        view.files.overlay = true;
        assert!(
            layout(Rect::new(0, 0, 180, 48), view)
                .right
                .unwrap()
                .overlay
        );
    }

    #[test]
    fn a_tiny_screen_is_too_small() {
        let view = View::default();
        assert!(layout(Rect::new(0, 0, 39, 30), view).too_small);
        assert!(layout(Rect::new(0, 0, 80, 9), view).too_small);
        assert!(!layout(Rect::new(0, 0, 40, 10), view).too_small);
    }
}

//! App: one frame of the application, drawn from its state in the "Quiet v2" look (ADR 0012,
//! `prototype/terminal-application/design/`): the side menu, the centre with the agent's panel,
//! and the status bar.

pub(crate) mod centre;
pub(crate) mod side_menu;
pub(crate) mod status;

use ratatui::Frame;

use crate::app::paint::{self, Mode, Painter, fg};
use crate::app::state::App;
use crate::app::theme;

/// Draws `app` into `frame` at `now_ms` (milliseconds since the Unix epoch, for the ages), for a
/// terminal that shows colours as `mode` says.
pub(crate) fn frame(frame: &mut Frame<'_>, app: &App, now_ms: u64, mode: Mode) {
    let area = frame.area();
    {
        let mut p = Painter {
            buf: frame.buffer_mut(),
        };
        p.fill(area, theme::CANVAS);
        if app.layout.too_small {
            p.text(1, 0, "too small for orca-term", fg(theme::TEXT));
        } else {
            if let Some(card) = app.layout.side_menu {
                side_menu::draw(&mut p, app, card, now_ms);
            }
            centre::draw(&mut p, app);
            status::draw(&mut p, app);
        }
        paint::finish(p.buf, mode);
    }
    if let Some(cursor) = centre::cursor(app) {
        frame.set_cursor_position(cursor);
    }
}

#[cfg(test)]
mod tests;

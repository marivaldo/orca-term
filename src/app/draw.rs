//! App: one frame of the application, drawn from its state in the "Quiet v2" look (ADR 0012,
//! `prototype/terminal-application/design/`): the side menu, the centre with the open worktree's
//! panels (or a failed agent's card, or its log), the right column (over the centre when it is an
//! overlay), the status bar, and the picker or help floating over them.

pub(crate) mod centre;
pub(crate) mod failure;
pub(crate) mod help;
pub(crate) mod picker;
pub(crate) mod right_column;
pub(crate) mod side_menu;
pub(crate) mod status;

use ratatui::Frame;

use crate::app::paint::{self, Mode, Painter, fg};
use crate::app::state::{App, Overlay};
use crate::app::theme;
use crate::app::view::Tab;

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
            failed_agent(&mut p, app, now_ms);
            if let Some(right) = app.layout.right {
                right_column::draw(&mut p, app, right.area);
            }
            status::draw(&mut p, app);
            match &app.overlay {
                Some(Overlay::Picker(picker)) => picker::draw(&mut p, app, picker, area, now_ms),
                Some(Overlay::Help) => help::draw(&mut p, area),
                None => {}
            }
        }
        paint::finish(p.buf, mode);
    }
    if app.overlay.is_none()
        && let Some(cursor) = centre::cursor(app)
    {
        frame.set_cursor_position(cursor);
    }
}

/// Over the agent's card, when it shows: a failed agent's log, or its output and card.
fn failed_agent(p: &mut Painter<'_>, app: &App, now_ms: u64) {
    let Some(card) = app.layout.card(Tab::Agent) else {
        return;
    };
    if let Some(log) = app.log_shown() {
        failure::log(p, log, card.body);
    } else if let Some(end) = app.failure() {
        failure::draw(p, app, end, card.body, now_ms);
    }
}

#[cfg(test)]
mod tests;

//! App: which of the open worktree's panels the centre shows (#176): one of the three tabs
//! (`1 agent  2 nvim  3 shell`), the split of the agent beside Neovim (or the shell), and the zoom
//! that fills the screen with the focused panel; and whether the right column shows (#177): as a
//! column, wanted from 150 columns, or as an overlay on the centre, asked for under that. The view
//! is the same whichever worktree is open, so switching worktree keeps the tab, the split, the zoom
//! and the right column.

/// One of a worktree's three panels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Tab {
    Agent,
    Nvim,
    Shell,
}

impl Tab {
    /// Every tab, in the order the tab row shows them.
    pub(crate) const ALL: [Self; 3] = [Self::Agent, Self::Nvim, Self::Shell];

    /// The tab's label: its key after the prefix, and its name.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Agent => "1 agent",
            Self::Nvim => "2 nvim",
            Self::Shell => "3 shell",
        }
    }

    /// The tab's name, as the status bar and the zoom's header say it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Nvim => "nvim",
            Self::Shell => "shell",
        }
    }
}

/// What the centre shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct View {
    /// The panel with the focus, the one shown when there is no split.
    pub(crate) tab: Tab,
    /// The split's right half: Neovim or the shell, whichever was shown last.
    pub(crate) side: Tab,
    pub(crate) split: bool,
    pub(crate) zoom: bool,
    /// How the right column shows.
    pub(crate) files: Files,
}

/// How the right column (#177) shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Files {
    /// Whether it shows as a column where the screen is wide enough: on until `^a f` hides it.
    pub(crate) column: bool,
    /// Whether it shows over the centre, where it cannot be a column.
    pub(crate) overlay: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            tab: Tab::Agent,
            side: Tab::Nvim,
            split: false,
            zoom: false,
            files: Files {
                column: true,
                overlay: false,
            },
        }
    }
}

impl View {
    /// The panels on screen, left to right: the focused one alone, or the split's two.
    pub(crate) fn shown(self) -> Vec<Tab> {
        if self.split && !self.zoom {
            vec![Tab::Agent, self.side]
        } else {
            vec![self.tab]
        }
    }

    /// `^a 1`, `2` or `3`: the focus goes to `tab`, and Neovim or the shell becomes the split's
    /// right half.
    pub(crate) fn show(&mut self, tab: Tab) {
        self.tab = tab;
        if tab != Tab::Agent {
            self.side = tab;
        }
    }

    /// `^a h` in the centre. Returns `false` when the focus does not stay in the centre: the
    /// person moved left past the agent. In the split it moves from the right half to the agent;
    /// zoomed, it never leaves the centre.
    pub(crate) fn left(&mut self) -> bool {
        if self.split && self.tab != Tab::Agent {
            self.tab = Tab::Agent;
            return true;
        }
        self.zoom
    }

    /// `^a l` in the centre: in the split, from the agent to the right half. Returns whether the
    /// focus moved; when not, it may go on to the right column.
    pub(crate) fn right(&mut self) -> bool {
        if self.split && self.tab == Tab::Agent {
            self.tab = self.side;
            return true;
        }
        false
    }

    /// `^a |`: the split on or off. Turned on from the agent, the focus stays on the agent;
    /// from Neovim or the shell, it stays there, on the right.
    pub(crate) fn toggle_split(&mut self) {
        self.split = !self.split;
    }

    /// `^a z`: the focused panel fills the screen, or the view goes back as it was.
    pub(crate) fn toggle_zoom(&mut self) {
        self.zoom = !self.zoom;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_shows_alone_and_neovim_or_the_shell_becomes_the_splits_right_half() {
        let mut view = View::default();
        assert_eq!(view.shown(), [Tab::Agent]);
        view.show(Tab::Shell);
        assert_eq!(view.shown(), [Tab::Shell]);
        view.toggle_split();
        assert_eq!(view.shown(), [Tab::Agent, Tab::Shell]);
        assert_eq!(view.tab, Tab::Shell, "the focus stays on the shell");
        view.show(Tab::Nvim);
        assert_eq!(view.shown(), [Tab::Agent, Tab::Nvim]);
        view.show(Tab::Agent);
        assert_eq!(
            view.shown(),
            [Tab::Agent, Tab::Nvim],
            "the right half stays"
        );
    }

    #[test]
    fn h_and_l_move_between_the_splits_halves() {
        let mut view = View::default();
        view.toggle_split();
        assert!(view.right());
        assert_eq!(view.tab, Tab::Nvim);
        assert!(!view.right(), "nothing right of Neovim in the centre");
        assert_eq!(view.tab, Tab::Nvim);
        assert!(view.left());
        assert_eq!(view.tab, Tab::Agent);
        assert!(!view.left(), "left of the agent is the side menu");
    }

    #[test]
    fn the_zoom_shows_the_focused_panel_and_keeps_the_split() {
        let mut view = View::default();
        view.toggle_split();
        assert!(view.right());
        view.toggle_zoom();
        assert_eq!(view.shown(), [Tab::Nvim]);
        assert!(view.left(), "zoomed, the focus stays in the centre");
        assert_eq!(view.shown(), [Tab::Agent], "and the zoom switches panel");
        assert!(view.left());
        assert!(view.right());
        assert_eq!(view.shown(), [Tab::Nvim]);
        view.toggle_zoom();
        assert_eq!(view.shown(), [Tab::Agent, Tab::Nvim], "the split is back");
        assert_eq!(view.tab, Tab::Nvim, "with the focus where it was");
    }

    #[test]
    fn zoomed_without_the_split_h_stays_in_the_centre() {
        let mut view = View::default();
        view.show(Tab::Nvim);
        view.toggle_zoom();
        assert!(view.left());
        assert_eq!(view.tab, Tab::Nvim);
    }
}

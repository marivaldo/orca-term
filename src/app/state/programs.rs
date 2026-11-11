//! App: what the state does with the worktrees' Neovims and shells (#176): starting one the first
//! time its panel shows, its keys, its output and its end. The processes themselves live in the
//! run loop (`app.rs`); here each is its screen and whether it runs.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

use super::{App, Effect, size_of};
use crate::adapters::pty::Printed;
use crate::app::keys;
use crate::app::program::{Kind, Program, ProgramId, Status};

impl App {
    /// The open worktree's `kind` of program.
    fn program_id(&self, kind: Kind) -> Option<ProgramId> {
        let worktree = self.open.clone()?;
        Some(ProgramId { worktree, kind })
    }

    /// Starts each program shown in the centre that has not started yet, at its panel's size.
    pub(super) fn start_shown(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        for card in self.layout.cards.clone() {
            let Some(id) = Kind::of(card.tab).and_then(|kind| self.program_id(kind)) else {
                continue;
            };
            if !self.programs.contains_key(&id) {
                effects.push(self.run(id, card.body));
            }
        }
        effects
    }

    /// Starts `id` afresh in a panel of `body`.
    fn run(&mut self, id: ProgramId, body: Rect) -> Effect {
        self.programs.insert(id.clone(), Program::new(body));
        Effect::Run(id, size_of(body))
    }

    /// Makes the open worktree's `kind` of program `body` large, telling it when that changed.
    pub(super) fn resize_program(&mut self, kind: Kind, body: Rect) -> Option<Effect> {
        let id = self.program_id(kind)?;
        let program = self.programs.get_mut(&id)?;
        program
            .screen
            .resize(body)
            .then(|| Effect::Resize(id, size_of(body)))
    }

    /// A key for the open worktree's `kind` of program: every key goes to it while it runs; once
    /// it ended or could not start, `enter` starts it again.
    pub(super) fn program_key(&mut self, kind: Kind, key: KeyEvent) -> Vec<Effect> {
        let Some(id) = self.program_id(kind) else {
            return Vec::new();
        };
        let running = self.programs.get(&id).map(Program::running);
        match running {
            Some(true) => self.type_into(kind, |program| {
                keys::encode(key, program.screen.application_cursor())
            }),
            Some(false) if key.code == KeyCode::Enter => {
                let body = self.layout.body_of(kind.tab());
                vec![self.run(id, body)]
            }
            _ => Vec::new(),
        }
    }

    /// The bytes `encode` makes for the open worktree's `kind` of program, typed into it while it
    /// runs.
    pub(super) fn type_into(
        &self,
        kind: Kind,
        encode: impl FnOnce(&Program) -> Vec<u8>,
    ) -> Vec<Effect> {
        let Some(id) = self.program_id(kind) else {
            return Vec::new();
        };
        match self.programs.get(&id) {
            Some(program) if program.running() => {
                let bytes = encode(program);
                vec![Effect::TypeInto(id, bytes)]
            }
            _ => Vec::new(),
        }
    }

    /// What the program `id` printed, or its end. Its screen answers the questions it asked.
    pub(crate) fn printed(&mut self, id: &ProgramId, printed: Printed) -> Vec<Effect> {
        let Some(program) = self.programs.get_mut(id) else {
            return Vec::new();
        };
        let answers = program.printed(printed);
        if answers.is_empty() {
            return Vec::new();
        }
        vec![Effect::TypeInto(id.clone(), answers)]
    }

    /// The program `id` could not start, for `why`.
    pub(crate) fn run_failed(&mut self, id: &ProgramId, why: String) {
        if let Some(program) = self.programs.get_mut(id) {
            program.status = Status::Failed(why);
        }
    }
}

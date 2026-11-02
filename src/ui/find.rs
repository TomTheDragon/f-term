//! Searching in any window with Ctrl+F: lists jump to the next matching
//! row, text views scroll to the next matching line, and the matches are
//! highlighted. Ctrl+F finds the next one, Ctrl+G the previous, Esc ends.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::actions::PromptKind;
use super::dialogs::Dialog;
use super::widgets;
use super::App;

impl App {
    /// The search text, if a search is active in the top window.
    pub(super) fn dialog_find_query(&self) -> Option<String> {
        match &self.dialog_find {
            Some((q, depth)) if *depth == self.dialogs.len() => Some(q.clone()),
            _ => None,
        }
    }

    /// Ctrl+F, Ctrl+G and Esc in a window. Returns true when handled.
    pub(super) fn dialog_find_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match self.dialogs.last() {
            // Windows that are a text field themselves, or have nothing to search.
            None | Some(Dialog::Prompt(_) | Dialog::Confirm { .. } | Dialog::Image { .. } | Dialog::Status(_)) => return false,
            // The log browser's list searches all logs with Ctrl+F.
            Some(Dialog::Logs(l)) if l.stage == 0 => return false,
            _ => {}
        }
        let active = self.dialog_find_query().is_some();
        match key.code {
            KeyCode::Char('f') if ctrl => {
                if active {
                    self.dialog_find_step(true, false);
                } else {
                    self.prompt("Find in this window", "", PromptKind::DialogFind, false);
                }
                true
            }
            KeyCode::Char('g') if ctrl && active => {
                self.dialog_find_step(false, false);
                true
            }
            KeyCode::Esc if active => {
                self.dialog_find = None;
                true
            }
            _ => false,
        }
    }

    /// Start a search in the top window (from the prompt).
    pub(super) fn dialog_find_start(&mut self, text: &str) {
        let q = text.trim();
        if q.is_empty() {
            self.dialog_find = None;
            return;
        }
        self.dialog_find = Some((q.to_string(), self.dialogs.len()));
        self.dialog_find_step(true, true);
    }

    /// Go to the next (or previous) match, starting at the current one when `include`.
    fn dialog_find_step(&mut self, forward: bool, include: bool) {
        let Some(q) = self.dialog_find_query() else { return };
        // The rows of the frame drawn last: a focused list, or else a text view.
        if let Some((rows, sel)) = widgets::find_list() {
            if let Some(i) = widgets::find_row(&rows, &q, sel, forward, include) {
                if let Some(d) = self.dialogs.last_mut() {
                    d.select_row(i);
                }
                return;
            }
        }
        if let Some((rows, top)) = widgets::find_text() {
            // A match is shown in the second line of the view, so the
            // next search goes on from there.
            let found =
                if include { widgets::find_row(&rows, &q, top, true, true) } else { widgets::find_row(&rows, &q, top + 1, forward, false) };
            if let Some(i) = found {
                self.set_dialog_scroll(i.saturating_sub(1));
                return;
            }
        }
        self.flash(&format!("\"{q}\" not found"));
    }

    fn set_dialog_scroll(&mut self, line: usize) {
        match self.dialogs.last_mut() {
            Some(Dialog::Help { scroll } | Dialog::Text { scroll, .. }) => *scroll = line,
            Some(Dialog::Logs(l)) => l.scroll = line,
            Some(Dialog::Notes(n)) => n.scroll = line,
            Some(Dialog::Profile(p)) => p.scroll = line,
            Some(Dialog::Groups(g)) => g.scroll = line,
            _ => {}
        }
    }
}

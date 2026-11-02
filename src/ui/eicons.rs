//! Eicon picker (`/eicons`, Alt+E) and `[eicon]` completion.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;

use super::actions::Action;
use super::dialogs::Dialog;
use super::input::{EditResult, LineEditor};
use super::widgets;
use super::{ApiReply, App, AppEvent};
use crate::eicons::EiconIndex;

/// Results shown in the picker.
const LIMIT: usize = 300;

pub struct EiconPicker {
    pub filter: LineEditor,
    pub results: Vec<String>,
    pub sel: usize,
}

/// The `[eicon]` tag being typed before the cursor: (start of the name, name so far).
pub fn eicon_word(before: &str) -> Option<(usize, &str)> {
    let i = before.to_ascii_lowercase().rfind("[eicon]")? + "[eicon]".len();
    let word = &before[i..];
    (!word.contains(['[', ']', '\n'])).then_some((i, word))
}

impl App {
    /// Load the eicon list in the background (once).
    pub(super) fn ensure_eicons(&mut self) {
        if self.eicon_index.is_some() || self.eicon_loading {
            return;
        }
        self.eicon_loading = true;
        self.flash("Loading the eicon list…");
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = crate::eicons::load(&crate::api::agent()).map(Arc::new);
            let _ = tx.send(AppEvent::Api(ApiReply::Eicons(r)));
        });
    }

    pub(super) fn on_eicons(&mut self, r: Result<Arc<EiconIndex>, String>) {
        self.eicon_loading = false;
        match r {
            Ok(idx) => {
                self.flash(&format!("{} eicons known.", idx.count));
                self.eicon_index = Some(idx);
                if let Some(Dialog::Eicons(p)) = self.dialogs.last_mut() {
                    let q = p.filter.text().to_string();
                    p.results = self.eicon_index.as_ref().map(|i| i.search(&q, LIMIT)).unwrap_or_default();
                }
                self.completion_refresh();
            }
            Err(e) => {
                self.flash(&e);
                self.info(&format!("Could not load the eicon list: {e}"));
            }
        }
    }

    pub(super) fn open_eicon_picker(&mut self, filter: &str) {
        self.ensure_eicons();
        let results = self.eicon_index.as_ref().map(|i| i.search(filter, LIMIT)).unwrap_or_default();
        self.dialogs.push(Dialog::Eicons(Box::new(EiconPicker { filter: LineEditor::with_text(filter), results, sel: 0 })));
    }

    pub(super) fn eicon_picker_key(&mut self, key: KeyEvent) {
        let Some(Dialog::Eicons(p)) = self.dialogs.last_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                self.dialogs.pop();
                self.full_redraw = true;
            }
            KeyCode::Up => p.sel = p.sel.saturating_sub(1),
            KeyCode::Down => p.sel = (p.sel + 1).min(p.results.len().saturating_sub(1)),
            KeyCode::PageUp => p.sel = p.sel.saturating_sub(10),
            KeyCode::PageDown => p.sel = (p.sel + 10).min(p.results.len().saturating_sub(1)),
            KeyCode::Enter => {
                if let Some(name) = p.results.get(p.sel).cloned() {
                    self.dialogs.pop();
                    self.full_redraw = true;
                    // Complete a tag that is being typed, or insert a whole one.
                    let before = &self.input.text()[..self.input.cursor()];
                    let text = if eicon_word(before).map(|(_, w)| w.is_empty()).unwrap_or(false) {
                        format!("{name}[/eicon]")
                    } else {
                        format!("[eicon]{name}[/eicon]")
                    };
                    self.run_action(Action::InsertText(text));
                }
            }
            _ => {
                if let EditResult::Changed = p.filter.handle(key) {
                    let q = p.filter.text().to_string();
                    p.results = self.eicon_index.as_ref().map(|i| i.search(&q, LIMIT)).unwrap_or_default();
                    p.sel = 0;
                    self.full_redraw = true;
                }
            }
        }
    }

    /// Returns (dialog area, list area, list offset).
    pub(super) fn draw_eicon_picker(&mut self, buf: &mut Buffer, big: Rect, idx: usize) -> (Rect, Rect, usize) {
        let Dialog::Eicons(p) = &self.dialogs[idx] else { return (big, Rect::default(), 0) };
        let count = self.eicon_index.as_ref().map(|i| i.count).unwrap_or(0);
        let title = if count > 0 { format!("Eicons · {} of {count}", p.results.len()) } else { "Eicons".to_string() };
        let inner = widgets::popup(buf, big, &title, self.settings.ascii_borders);
        buf.set_string(inner.x, inner.y, "Search: ", Style::default().fg(Color::Gray));
        buf.set_string(inner.x + 8, inner.y, p.filter.text(), Style::default().fg(Color::Yellow));
        let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(3));
        let list_w = (body.width / 2).clamp(16, 32).min(body.width);
        let list_area = Rect::new(body.x, body.y, list_w, body.height);
        let items: Vec<Line<'static>> = p.results.iter().map(|n| Line::from(n.clone())).collect();
        let off = widgets::list(buf, list_area, &items, p.sel, true);
        if items.is_empty() {
            let msg = if self.eicon_loading { "Loading the eicon list…" } else { "Nothing found." };
            buf.set_string(body.x + 1, body.y, msg, Style::default().fg(Color::DarkGray));
        }
        let sel = p.results.get(p.sel).cloned();
        if let Some(name) = sel {
            let view = Rect::new(body.x + list_w + 2, body.y, body.width.saturating_sub(list_w + 2), body.height.min(12));
            if self.settings.active_image_mode() == crate::config::ImageMode::Off {
                buf.set_string(view.x, view.y, "Images are off (/images blocks).", Style::default().fg(Color::DarkGray));
            } else if view.width > 2 {
                let url = crate::images::eicon_url(&name);
                let (mode, proto, tc) = (self.settings.active_image_mode(), self.settings.graphics_protocol, self.settings.truecolor);
                self.images.request(&url, &self.tx, AppEvent::Image);
                let size = Rect::new(view.x, view.y, view.width.min(view.height * 2), view.height);
                self.images.draw(buf, size, &url, mode, proto, tc);
            }
        }
        let hints = [("type", "search"), ("Enter", "insert"), ("Esc", "close")];
        widgets::draw_hints(buf, inner.x, inner.bottom() - 1, &hints, inner.width);
        (big, list_area, off)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eicon_tag_at_cursor() {
        assert_eq!(eicon_word("hi [eicon]wav"), Some((10, "wav")));
        assert_eq!(eicon_word("hi [EICON]"), Some((10, "")));
        assert_eq!(eicon_word("hi [eicon]wave[/eicon] x"), None);
        assert_eq!(eicon_word("hello"), None);
    }
}

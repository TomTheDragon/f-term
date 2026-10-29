//! A small text editor for the input line and dialog fields.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Default)]
pub struct LineEditor {
    text: String,
    /// Cursor position as a byte offset (always on a char boundary).
    cursor: usize,
    history: Vec<String>,
    hist_pos: Option<usize>,
    stash: String,
    pub masked: bool,
    pub multiline: bool,
    kill_buffer: String,
}

pub enum EditResult {
    /// Key was not handled.
    Ignored,
    /// Text or cursor changed.
    Changed,
    /// Enter was pressed (single-line submit).
    Submit,
}

impl LineEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_text(text: &str) -> Self {
        let mut e = Self::default();
        e.set(text);
        e
    }

    pub fn masked() -> Self {
        LineEditor { masked: true, ..Default::default() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.hist_pos = None;
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn push_history(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        if self.history.last().map(|l| l != line).unwrap_or(true) {
            self.history.push(line.to_string());
        }
        if self.history.len() > 200 {
            self.history.remove(0);
        }
        self.hist_pos = None;
    }

    pub fn insert_str(&mut self, s: &str) {
        let s: String = if self.multiline { s.replace("\r\n", "\n").replace('\r', "\n") } else { s.replace(['\r', '\n'], " ") };
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    pub fn insert_char(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Replace the byte range [start, cursor) with `s`.
    pub fn replace_before_cursor(&mut self, start: usize, s: &str) {
        let start = start.min(self.cursor);
        self.text.replace_range(start..self.cursor, s);
        self.cursor = start + s.len();
    }

    fn prev_boundary(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map(|(p, _)| p).unwrap_or(0)
    }

    fn next_boundary(&self, i: usize) -> usize {
        self.text[i..].chars().next().map(|c| i + c.len_utf8()).unwrap_or(i)
    }

    fn word_left(&self) -> usize {
        let trimmed = self.text[..self.cursor].trim_end();
        match trimmed.char_indices().rev().find(|(_, c)| c.is_whitespace()) {
            Some((i, c)) => i + c.len_utf8(),
            None => 0,
        }
    }

    fn word_right(&self) -> usize {
        let mut it = self.text[self.cursor..].char_indices().peekable();
        let mut end = self.text.len() - self.cursor;
        while let Some(&(_, c)) = it.peek() {
            if !c.is_whitespace() {
                break;
            }
            it.next();
        }
        for (i, c) in it {
            if c.is_whitespace() {
                end = i;
                break;
            }
        }
        self.cursor + end
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map(|i| self.cursor + i).unwrap_or(self.text.len())
    }

    fn history_up(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let pos = match self.hist_pos {
            None => {
                self.stash = self.text.clone();
                self.history.len() - 1
            }
            Some(0) => return true,
            Some(p) => p - 1,
        };
        self.hist_pos = Some(pos);
        self.set(&self.history[pos].clone());
        true
    }

    fn history_down(&mut self) -> bool {
        match self.hist_pos {
            None => false,
            Some(p) if p + 1 >= self.history.len() => {
                self.hist_pos = None;
                let s = std::mem::take(&mut self.stash);
                self.set(&s);
                true
            }
            Some(p) => {
                self.hist_pos = Some(p + 1);
                self.set(&self.history[p + 1].clone());
                true
            }
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> EditResult {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Enter if self.multiline && (alt || shift) => {
                self.insert_char('\n');
            }
            KeyCode::Char('j') if ctrl && self.multiline => {
                self.insert_char('\n');
            }
            KeyCode::Enter => return EditResult::Submit,
            KeyCode::Char(c) if ctrl => match c {
                'a' => self.cursor = self.line_start(),
                'e' => self.cursor = self.line_end(),
                'b' => self.cursor = self.prev_boundary(self.cursor),
                'f' => self.cursor = self.next_boundary(self.cursor),
                'u' => {
                    let s = self.line_start();
                    self.kill_buffer = self.text[s..self.cursor].to_string();
                    self.text.replace_range(s..self.cursor, "");
                    self.cursor = s;
                }
                'k' => {
                    let e = self.line_end();
                    self.kill_buffer = self.text[self.cursor..e].to_string();
                    self.text.replace_range(self.cursor..e, "");
                }
                'w' => {
                    let s = self.word_left();
                    self.kill_buffer = self.text[s..self.cursor].to_string();
                    self.text.replace_range(s..self.cursor, "");
                    self.cursor = s;
                }
                'h' => {
                    if self.cursor > 0 {
                        let p = self.prev_boundary(self.cursor);
                        self.text.replace_range(p..self.cursor, "");
                        self.cursor = p;
                    }
                }
                'y' => {
                    let k = self.kill_buffer.clone();
                    self.insert_str(&k);
                }
                'd' if self.cursor < self.text.len() => {
                    let n = self.next_boundary(self.cursor);
                    self.text.replace_range(self.cursor..n, "");
                }
                _ => return EditResult::Ignored,
            },
            KeyCode::Char(c) if alt => match c {
                'b' => self.cursor = self.word_left(),
                'f' => self.cursor = self.word_right(),
                'd' => {
                    let e = self.word_right();
                    self.text.replace_range(self.cursor..e, "");
                }
                _ => return EditResult::Ignored,
            },
            KeyCode::Char(c) => self.insert_char(c),
            KeyCode::Backspace if ctrl || alt => {
                let s = self.word_left();
                self.text.replace_range(s..self.cursor, "");
                self.cursor = s;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let p = self.prev_boundary(self.cursor);
                    self.text.replace_range(p..self.cursor, "");
                    self.cursor = p;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.text.len() {
                    let n = self.next_boundary(self.cursor);
                    self.text.replace_range(self.cursor..n, "");
                }
            }
            KeyCode::Left if ctrl || alt => self.cursor = self.word_left(),
            KeyCode::Right if ctrl || alt => self.cursor = self.word_right(),
            KeyCode::Left => self.cursor = self.prev_boundary(self.cursor),
            KeyCode::Right => self.cursor = self.next_boundary(self.cursor),
            KeyCode::Home if ctrl => self.cursor = 0,
            KeyCode::End if ctrl => self.cursor = self.text.len(),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Up => {
                if self.multiline && self.text[..self.cursor].contains('\n') {
                    let ls = self.line_start();
                    let col = self.text[ls..self.cursor].chars().count();
                    let prev_end = ls - 1;
                    let prev_start = self.text[..prev_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
                    let target: usize = self.text[prev_start..prev_end].chars().take(col).map(char::len_utf8).sum();
                    self.cursor = prev_start + target;
                } else if !self.history_up() {
                    return EditResult::Ignored;
                }
            }
            KeyCode::Down => {
                if self.multiline && self.text[self.cursor..].contains('\n') {
                    let ls = self.line_start();
                    let col = self.text[ls..self.cursor].chars().count();
                    let next_start = self.line_end() + 1;
                    let next_end = self.text[next_start..].find('\n').map(|i| next_start + i).unwrap_or(self.text.len());
                    let target: usize = self.text[next_start..next_end].chars().take(col).map(char::len_utf8).sum();
                    self.cursor = next_start + target;
                } else if !self.history_down() {
                    return EditResult::Ignored;
                }
            }
            _ => return EditResult::Ignored,
        }
        EditResult::Changed
    }

    /// Visible text for display (masked for passwords).
    pub fn display(&self) -> String {
        if self.masked {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }

    /// Lay out the text wrapped into `width` columns.
    /// Returns the lines and the cursor position (col, row).
    pub fn layout(&self, width: u16) -> (Vec<String>, (u16, u16)) {
        let width = width.max(2) as usize;
        let shown = self.display();
        // Map the cursor byte offset into the displayed text.
        let cursor_chars = self.text[..self.cursor].chars().count();
        let mut lines = vec![String::new()];
        let mut col = 0usize;
        let mut pos = (0u16, 0u16);
        let mut found = false;
        for (i, c) in shown.chars().enumerate() {
            if i == cursor_chars {
                pos = (col as u16, (lines.len() - 1) as u16);
                found = true;
            }
            if c == '\n' {
                lines.push(String::new());
                col = 0;
                continue;
            }
            let w = c.width().unwrap_or(0);
            if col + w > width - 1 {
                lines.push(String::new());
                col = 0;
                if i == cursor_chars {
                    pos = (0, (lines.len() - 1) as u16);
                }
            }
            lines.last_mut().unwrap().push(c);
            col += w;
        }
        if !found {
            if col >= width - 1 {
                lines.push(String::new());
                col = 0;
            }
            pos = (col as u16, (lines.len() - 1) as u16);
        }
        (lines, pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn editing() {
        let mut e = LineEditor::new();
        for c in "héllo wörld".chars() {
            e.handle(key(KeyCode::Char(c)));
        }
        e.handle(key(KeyCode::Backspace));
        assert_eq!(e.text(), "héllo wörl");
        e.handle(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(&e.text()[e.cursor()..], "wörl");
        e.handle(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "wörl");
        let (lines, pos) = LineEditor::with_text("abcdef").layout(4);
        assert_eq!(lines, vec!["abc", "def", ""]);
        assert_eq!(pos, (0, 2));
    }

    #[test]
    fn history() {
        let mut e = LineEditor::new();
        e.push_history("one");
        e.push_history("two");
        e.set("draft");
        e.handle(key(KeyCode::Up));
        assert_eq!(e.text(), "two");
        e.handle(key(KeyCode::Up));
        assert_eq!(e.text(), "one");
        e.handle(key(KeyCode::Down));
        e.handle(key(KeyCode::Down));
        assert_eq!(e.text(), "draft");
    }
}

//! Rendering helpers shared by the chat view and dialogs.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};
use unicode_width::UnicodeWidthChar;

pub const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

pub fn block(title: &str, ascii: bool, focused: bool) -> Block<'static> {
    let mut b = Block::default().borders(Borders::ALL).title(Span::styled(
        format!(" {title} "),
        Style::default().add_modifier(Modifier::BOLD).fg(if focused { Color::Yellow } else { Color::Cyan }),
    ));
    b = if ascii { b.border_set(ASCII_BORDER) } else { b.border_type(BorderType::Rounded) };
    b.border_style(Style::default().fg(if focused { Color::Yellow } else { Color::DarkGray }))
}

pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

/// Clear an area and draw a bordered popup; returns the inner area.
pub fn popup(buf: &mut Buffer, area: Rect, title: &str, ascii: bool) -> Rect {
    // Drawing outside the buffer panics; keep popups on screen on small terminals.
    let area = area.intersection(buf.area);
    Clear.render(area, buf);
    let b = block(title, ascii, true);
    let inner = b.inner(area);
    b.render(area, buf);
    inner
}

pub fn str_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Truncate to a display width, adding an ellipsis if cut.
pub fn trunc(s: &str, width: usize) -> String {
    if str_width(s) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Word-wrap a styled line into lines of at most `width` columns.
/// Continuation lines start with `indent` spaces.
pub fn wrap(line: &Line<'static>, width: usize, indent: usize) -> Vec<Line<'static>> {
    let width = width.max(4);
    let indent = indent.min(width / 2);
    // Flatten into (char, style) cells.
    let cells: Vec<(char, Style)> = line.spans.iter().flat_map(|s| s.content.chars().map(move |c| (c, s.style))).collect();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut i = 0;
    let mut first = true;
    while i < cells.len() || first {
        let avail = if first { width } else { width - indent };
        // Find how many cells fit.
        let mut w = 0;
        let mut end = i;
        let mut last_space: Option<usize> = None;
        while end < cells.len() {
            let cw = cells[end].0.width().unwrap_or(0);
            if w + cw > avail {
                break;
            }
            if cells[end].0 == ' ' {
                last_space = Some(end);
            }
            w += cw;
            end += 1;
        }
        let mut cut = end;
        if end < cells.len() && cells[end].0 != ' ' {
            if let Some(sp) = last_space {
                if sp > i {
                    cut = sp + 1;
                }
            }
            if cut == i {
                cut = (i + 1).max(end);
            }
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        if !first && indent > 0 {
            spans.push(Span::raw(" ".repeat(indent)));
        }
        let mut cur = String::new();
        let mut cur_style = None;
        for &(c, st) in &cells[i..cut] {
            if Some(st) != cur_style {
                if let Some(s) = cur_style {
                    spans.push(Span::styled(std::mem::take(&mut cur), s));
                }
                cur_style = Some(st);
            }
            cur.push(c);
        }
        if let Some(s) = cur_style {
            // Trim trailing spaces at wrap points.
            let text = if cut < cells.len() { cur.trim_end().to_string() } else { cur };
            spans.push(Span::styled(text, s));
        }
        out.push(Line::from(spans));
        i = cut;
        // Skip leading spaces on continuation lines.
        while i < cells.len() && cells[i].0 == ' ' && cut < cells.len() {
            i += 1;
        }
        first = false;
        if i >= cells.len() {
            break;
        }
    }
    out
}

pub fn wrap_all(lines: &[Line<'static>], width: usize, indent: usize) -> Vec<Line<'static>> {
    lines.iter().flat_map(|l| wrap(l, width, indent)).collect()
}

/// Draw a selectable list. Returns the scroll offset used.
pub fn list(buf: &mut Buffer, area: Rect, items: &[Line<'static>], sel: usize, focused: bool) -> usize {
    if focused {
        FIND_LIST.with(|f| *f.borrow_mut() = Some((items.iter().map(line_text).collect(), sel.min(items.len().saturating_sub(1)))));
    }
    if area.height == 0 {
        return 0;
    }
    let h = area.height as usize;
    let sel = sel.min(items.len().saturating_sub(1));
    let offset = if sel >= h { sel + 1 - h } else { 0 };
    for (row, item) in items.iter().skip(offset).take(h).enumerate() {
        let y = area.y + row as u16;
        let selected = offset + row == sel;
        if selected {
            let st = if focused { Style::default().bg(Color::Rgb(40, 60, 100)) } else { Style::default().bg(Color::Rgb(40, 40, 40)) };
            buf.set_style(Rect::new(area.x, y, area.width, 1), st);
            buf.set_string(area.x, y, if focused { "›" } else { " " }, Style::default().fg(Color::Yellow));
        }
        buf.set_line(area.x + 1, y, item, area.width.saturating_sub(1));
    }
    if items.len() > h {
        // Scroll indicator.
        let pos = (offset * (h - 1)) / (items.len() - h).max(1);
        buf.set_string(area.right() - 1, area.y + pos.min(h - 1) as u16, "▐", Style::default().fg(Color::DarkGray));
    }
    offset
}

// ---- searching in dialogs (Ctrl+F) ---------------------------------------------------

thread_local! {
    /// The focused list of the dialog drawn last: (rows as text, selected row).
    static FIND_LIST: std::cell::RefCell<Option<(Vec<String>, usize)>> = const { std::cell::RefCell::new(None) };
    /// The scrolling text of the dialog drawn last: (lines as text, first line shown).
    static FIND_TEXT: std::cell::RefCell<Option<(Vec<String>, usize)>> = const { std::cell::RefCell::new(None) };
}

pub fn line_text(l: &Line<'_>) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Forget the rows recorded for the last dialog (before drawing the top one).
pub fn clear_find_rows() {
    FIND_LIST.with(|f| *f.borrow_mut() = None);
    FIND_TEXT.with(|f| *f.borrow_mut() = None);
}

/// Record the lines of a scrolling text view (already wrapped) and its scroll position.
pub fn note_text(lines: &[Line<'static>], scroll: usize) {
    FIND_TEXT.with(|f| *f.borrow_mut() = Some((lines.iter().map(line_text).collect(), scroll)));
}

pub fn find_list() -> Option<(Vec<String>, usize)> {
    FIND_LIST.with(|f| f.borrow().clone())
}

pub fn find_text() -> Option<(Vec<String>, usize)> {
    FIND_TEXT.with(|f| f.borrow().clone())
}

/// The next row containing `query` (case-insensitive) after `from`, or before
/// it going back, wrapping around. `include` also tests `from` itself.
pub fn find_row(rows: &[String], query: &str, from: usize, forward: bool, include: bool) -> Option<usize> {
    let q = query.to_lowercase();
    let n = rows.len();
    if n == 0 || q.is_empty() {
        return None;
    }
    let from = from.min(n - 1);
    let start = if include { 0 } else { 1 };
    (start..=n).map(|d| if forward { (from + d) % n } else { (from + n * 2 - d) % n }).find(|&i| rows[i].to_lowercase().contains(&q))
}

/// Mark every occurrence of `query` (case-insensitive) in `area`.
pub fn highlight(buf: &mut Buffer, area: Rect, query: &str) {
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return;
    }
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        // One char per cell; wide characters' second cells are skipped.
        let cells: Vec<(u16, char)> = (area.left()..area.right())
            .filter_map(|x| buf[(x, y)].symbol().chars().next().map(|c| (x, c.to_lowercase().next().unwrap_or(c))))
            .collect();
        let mut i = 0;
        while i + q.len() <= cells.len() {
            if cells[i..i + q.len()].iter().map(|c| c.1).eq(q.iter().copied()) {
                for (x, _) in &cells[i..i + q.len()] {
                    buf[(*x, y)].set_style(Style::default().fg(Color::Black).bg(Color::Yellow));
                }
                i += q.len();
            } else {
                i += 1;
            }
        }
    }
}

// ---- clickable key hints ------------------------------------------------------------

thread_local! {
    /// Key hints drawn in the current frame, for mouse clicks.
    static KEY_HITS: std::cell::RefCell<Vec<(Rect, KeyEvent)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Forget the clickable areas of the last frame.
pub fn clear_key_hits() {
    KEY_HITS.with(|h| h.borrow_mut().clear());
}

/// Make an area act like pressing `key` when clicked.
pub fn add_key_hit(area: Rect, key: KeyEvent) {
    KEY_HITS.with(|h| h.borrow_mut().push((area, key)));
}

/// The key of the clickable area at a position (the last drawn wins, so
/// popups beat what is under them).
pub fn key_at(col: u16, row: u16) -> Option<KeyEvent> {
    let p = ratatui::layout::Position { x: col, y: row };
    KEY_HITS.with(|h| h.borrow().iter().rev().find(|(r, _)| r.contains(p)).map(|(_, k)| *k))
}

/// The key a hint label like "Enter", "Ctrl+S", "F2" or "y/Enter" stands for
/// (the first one if there are several). None for labels like "↑↓" or "type".
pub fn parse_key(label: &str) -> Option<KeyEvent> {
    let first = label.split('/').next()?;
    let mut mods = KeyModifiers::NONE;
    let mut rest = first;
    loop {
        if let Some(r) = rest.strip_prefix("Ctrl+") {
            mods |= KeyModifiers::CONTROL;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Alt+") {
            mods |= KeyModifiers::ALT;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Shift+") {
            mods |= KeyModifiers::SHIFT;
            rest = r;
        } else {
            break;
        }
    }
    let code = match rest {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Tab" if mods.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        "Tab" => KeyCode::Tab,
        "Space" => KeyCode::Char(' '),
        "Del" | "Delete" => KeyCode::Delete,
        "PgUp" => KeyCode::PageUp,
        "PgDn" => KeyCode::PageDown,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "Backspace" => KeyCode::Backspace,
        f if f.len() > 1 && f.starts_with('F') && f[1..].parse::<u8>().is_ok() => KeyCode::F(f[1..].parse().ok()?),
        c if c.chars().count() == 1 && c.chars().all(|c| c.is_ascii_graphic()) => {
            let ch = c.chars().next()?;
            KeyCode::Char(if mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) { ch.to_ascii_lowercase() } else { ch })
        }
        _ => return None,
    };
    Some(KeyEvent::new(code, mods))
}

/// Draw a hint line (like `hint_line`) and make each key in it clickable.
pub fn draw_hints(buf: &mut Buffer, x: u16, y: u16, parts: &[(&str, &str)], max_width: u16) {
    let line = hint_line(parts);
    buf.set_line(x, y, &line, max_width);
    let end = x.saturating_add(max_width);
    let mut cx = x;
    for (i, (k, v)) in parts.iter().enumerate() {
        if i > 0 {
            cx = cx.saturating_add(2);
        }
        // The key and its description are one click target.
        let w = (str_width(k) + 1 + str_width(v)) as u16;
        if let Some(key) = parse_key(k) {
            if cx < end {
                add_key_hit(Rect::new(cx, y, w.min(end - cx), 1), key);
            }
        }
        cx = cx.saturating_add(w);
    }
}

pub fn hint_line(parts: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (k, v)) in parts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(k.to_string(), Style::default().fg(Color::Black).bg(Color::Gray)));
        spans.push(Span::styled(format!(" {v}"), Style::default().fg(Color::Gray)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_keys() {
        assert_eq!(parse_key("Enter"), Some(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(parse_key("y/Enter"), Some(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)));
        assert_eq!(parse_key("Ctrl+S"), Some(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)));
        assert_eq!(parse_key("Alt+A"), Some(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT)));
        assert_eq!(parse_key("F12"), Some(KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE)));
        assert_eq!(parse_key("↑↓"), None);
        assert_eq!(parse_key("type"), None);
        clear_key_hits();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        draw_hints(&mut buf, 2, 0, &[("Tab", "next"), ("↑↓", "move"), ("Esc", "quit")], 30);
        assert_eq!(key_at(2, 0).map(|k| k.code), Some(KeyCode::Tab));
        assert_eq!(key_at(9, 0).map(|k| k.code), Some(KeyCode::Tab));
        assert_eq!(key_at(13, 0), None);
        assert_eq!(key_at(22, 0).map(|k| k.code), Some(KeyCode::Esc));
    }
    #[test]
    fn wraps_words() {
        let l = Line::from("hello world this is a test");
        let w = wrap(&l, 11, 2);
        let texts: Vec<String> = w.iter().map(|l| l.spans.iter().map(|s| s.content.to_string()).collect()).collect();
        assert_eq!(texts[0], "hello world");
        assert!(texts.iter().all(|t| str_width(t) <= 11));
        let long = Line::from("x".repeat(25));
        assert_eq!(wrap(&long, 10, 0).len(), 3);
        assert_eq!(wrap(&Line::default(), 10, 0).len(), 1);
    }
}

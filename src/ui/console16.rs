//! Terminal backend for consoles limited to 16 colours.
//!
//! On the legacy Windows console (Windows 9x/ME, and Windows 7/8 without ANSI
//! support) crossterm draws through the console API, which knows only the 16
//! standard colours and turns RGB and 256-colour values into black. This
//! wrapper maps those to the nearest standard colour before drawing.

use std::io;

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::Color;

pub struct ConsoleBackend<W: io::Write> {
    inner: CrosstermBackend<W>,
    map_colors: bool,
}

impl<W: io::Write> ConsoleBackend<W> {
    pub fn new(writer: W) -> Self {
        Self { inner: CrosstermBackend::new(writer), map_colors: is_legacy_console() }
    }
}

/// True on the legacy Windows console (no ANSI support): it can only show the
/// 16 standard colours, and crossterm draws through the console API.
pub fn is_legacy_console() -> bool {
    #[cfg(windows)]
    {
        !crossterm::ansi_support::supports_ansi()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// The 16 standard colours in xterm index order.
const STANDARD: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

/// RGB value of an xterm 256-colour index (16..=255).
pub(super) fn indexed_rgb(i: u8) -> [u8; 3] {
    if i >= 232 {
        let v = 8 + (i - 232) * 10;
        return [v, v, v];
    }
    let i = i - 16;
    let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
    [level(i / 36), level((i / 6) % 6), level(i % 6)]
}

/// Map any colour to one of the 16 standard console colours, keeping its hue.
pub fn to_16(color: Color) -> Color {
    let [r, g, b] = match color {
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(i) if i < 16 => return STANDARD[i as usize],
        Color::Indexed(i) => indexed_rgb(i),
        other => return other,
    };
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let chroma = max - min;
    if chroma < 40 {
        // Greyish: pick a grey by brightness.
        return match max {
            0..=0x3f => Color::Black,
            0x40..=0x9f => Color::DarkGray,
            0xa0..=0xdf => Color::Gray,
            _ => Color::White,
        };
    }
    // Which channels are strong relative to the colour's range.
    let on = |c: u8| c - min >= chroma / 2;
    let index = on(r) as usize | (on(g) as usize) << 1 | (on(b) as usize) << 2;
    STANDARD[index + if max >= 0xc0 { 8 } else { 0 }]
}

impl<W: io::Write> Backend for ConsoleBackend<W> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        if !self.map_colors {
            return self.inner.draw(content);
        }
        let cells: Vec<(u16, u16, Cell)> = content
            .map(|(x, y, c)| {
                let mut c = c.clone();
                c.fg = to_16(c.fg);
                c.bg = to_16(c.bg);
                (x, y, c)
            })
            .collect();
        self.inner.draw(cells.iter().map(|(x, y, c)| (*x, *y, c)))
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }
    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }
    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }
    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_to_standard_colors() {
        assert_eq!(to_16(Color::Rgb(0xff, 0x40, 0x40)), Color::LightRed);
        assert_eq!(to_16(Color::Rgb(0x60, 0x80, 0xff)), Color::LightBlue);
        assert_eq!(to_16(Color::Rgb(0xff, 0xff, 0xff)), Color::White);
        assert_eq!(to_16(Color::Rgb(0xa0, 0xa0, 0xa0)), Color::Gray);
        assert_eq!(to_16(Color::Rgb(0x50, 0x50, 0x50)), Color::DarkGray);
        assert_eq!(to_16(Color::Rgb(0xb0, 0x70, 0x30)), Color::Yellow);
        assert_eq!(to_16(Color::Rgb(0xc0, 0x60, 0xff)), Color::LightMagenta);
        assert_eq!(to_16(Color::Indexed(9)), Color::LightRed);
        assert_eq!(to_16(Color::Indexed(231)), Color::White);
        assert_eq!(to_16(Color::Indexed(16)), Color::Black);
        assert_eq!(to_16(Color::Cyan), Color::Cyan);
        assert_eq!(to_16(Color::Reset), Color::Reset);
    }
}

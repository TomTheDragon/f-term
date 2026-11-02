//! GUI front end (`gui` feature, the `*-gui` builds): draws the text screen in
//! an own window instead of a terminal.
//!
//! The window runs on its own thread (on macOS the app logic does instead,
//! since windows must live on the main thread there). `GuiBackend`, a ratatui
//! backend, writes cells into a shared grid and asks the window to repaint;
//! the window turns keyboard and mouse input into crossterm events, so the app
//! treats it like a terminal.
//!
//! - `win32.rs`: Windows 95 to 11, drawn with GDI and the system fonts.
//! - `winit.rs` + `soft.rs`: Linux and macOS, drawn in software with an
//!   embedded font (DejaVu Sans Mono).
//!
//! Box-drawing and block characters are drawn as shapes on both, so they line
//! up and don't depend on the font.

use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crossterm::event::Event;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

use crate::config::Settings;
use crate::images::{self, Pixels, Placement};

#[cfg(not(windows))]
mod soft;
#[cfg(windows)]
mod win32;
#[cfg(not(windows))]
mod winit;

#[cfg(windows)]
pub use win32::{run, CliConsole};
#[cfg(not(windows))]
pub use winit::run;

// ---- settings ---------------------------------------------------------------------

/// What the window needs from the settings.
pub struct GuiConfig {
    /// Font name (Windows only; empty = default).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub font: String,
    /// Font height in pixels.
    pub font_size: u16,
    pub cols: u16,
    pub rows: u16,
}

impl GuiConfig {
    pub fn new(s: &Settings) -> GuiConfig {
        GuiConfig {
            font: s.gui_font.trim().to_string(),
            font_size: s.gui_font_size.clamp(6, 72),
            cols: s.gui_columns.clamp(20, 400),
            rows: s.gui_rows.clamp(10, 200),
        }
    }
}

// ---- colours ----------------------------------------------------------------------

pub(super) const BOLD: u8 = 1;
pub(super) const ITALIC: u8 = 2;
pub(super) const UNDERLINE: u8 = 4;
pub(super) const CROSSED: u8 = 8;

/// Colours are 0x00RRGGBB.
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// The 16 standard colours in xterm index order.
const PALETTE: [u32; 16] = [
    rgb(0x0c, 0x0c, 0x0c),
    rgb(0xc5, 0x0f, 0x1f),
    rgb(0x13, 0xa1, 0x0e),
    rgb(0xc1, 0x9c, 0x00),
    rgb(0x00, 0x37, 0xda),
    rgb(0x88, 0x17, 0x98),
    rgb(0x3a, 0x96, 0xdd),
    rgb(0xcc, 0xcc, 0xcc),
    rgb(0x76, 0x76, 0x76),
    rgb(0xe7, 0x48, 0x56),
    rgb(0x16, 0xc6, 0x0c),
    rgb(0xf9, 0xf1, 0xa5),
    rgb(0x3b, 0x78, 0xff),
    rgb(0xb4, 0x00, 0x9e),
    rgb(0x61, 0xd6, 0xd6),
    rgb(0xf2, 0xf2, 0xf2),
];
pub(super) const DEFAULT_FG: u32 = PALETTE[7];
pub(super) const DEFAULT_BG: u32 = rgb(0, 0, 0);

fn color_value(c: Color, default: u32) -> u32 {
    let i = match c {
        Color::Reset => return default,
        Color::Rgb(r, g, b) => return rgb(r, g, b),
        Color::Indexed(i) if i >= 16 => {
            let [r, g, b] = super::console16::indexed_rgb(i);
            return rgb(r, g, b);
        }
        Color::Indexed(i) => i as usize,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
    };
    PALETTE[i]
}

/// Mix `a` into `b`: `a_parts` of 4 parts are `a`.
pub(super) fn blend(a: u32, b: u32, a_parts: u32) -> u32 {
    let ch = |c: u32, s: u32| (c >> s) & 0xff;
    let mix = |s: u32| (ch(a, s) * a_parts + ch(b, s) * (4 - a_parts)) / 4;
    mix(0) | mix(8) << 8 | mix(16) << 16
}

// ---- screen -----------------------------------------------------------------------

#[derive(Clone, PartialEq)]
pub(super) struct GCell {
    pub sym: String,
    pub fg: u32,
    pub bg: u32,
    pub flags: u8,
}

impl GCell {
    fn blank() -> GCell {
        GCell { sym: " ".into(), fg: DEFAULT_FG, bg: DEFAULT_BG, flags: 0 }
    }

    fn from_cell(c: &Cell) -> GCell {
        let m = c.modifier;
        let (mut fg, mut bg) = (color_value(c.fg, DEFAULT_FG), color_value(c.bg, DEFAULT_BG));
        if m.contains(Modifier::REVERSED) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if m.contains(Modifier::DIM) {
            fg = blend(fg, bg, 2);
        }
        if m.contains(Modifier::HIDDEN) {
            fg = bg;
        }
        let mut flags = 0;
        for (bit, f) in
            [(Modifier::BOLD, BOLD), (Modifier::ITALIC, ITALIC), (Modifier::UNDERLINED, UNDERLINE), (Modifier::CROSSED_OUT, CROSSED)]
        {
            if m.contains(bit) {
                flags |= f;
            }
        }
        GCell { sym: c.symbol().to_string(), fg, bg, flags }
    }

    /// Columns the symbol covers (1 or 2).
    pub fn width(&self) -> usize {
        self.sym.width().clamp(1, 2)
    }

    /// The box-drawing or block shape to draw instead of a glyph, if any.
    pub fn shape(&self) -> Option<Shape> {
        let mut it = self.sym.chars();
        let c = it.next()?;
        if it.next().is_some() {
            return None;
        }
        shape_of(c)
    }
}

pub(super) struct Screen {
    /// Text selected with the mouse: (start, end) as (column, row), in reading order.
    pub selection: Option<((u16, u16), (u16, u16))>,
    /// Images to paint into the cells marked with `images::IMAGE_CELL`.
    pub images: Vec<Placement>,
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<GCell>,
    pub dirty: Vec<bool>,
    pub cursor: (u16, u16),
    pub cursor_visible: bool,
    pub cell_w: i32,
    pub cell_h: i32,
}

impl Screen {
    pub fn new() -> Screen {
        Screen {
            selection: None,
            images: Vec::new(),
            cols: 0,
            rows: 0,
            cells: Vec::new(),
            dirty: Vec::new(),
            cursor: (0, 0),
            cursor_visible: false,
            cell_w: 8,
            cell_h: 16,
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.selection = None;
        self.cols = cols;
        self.rows = rows;
        self.cells = vec![GCell::blank(); cols as usize * rows as usize];
        self.dirty = vec![true; rows as usize];
    }

    pub fn mark(&mut self, row: u16) {
        if let Some(d) = self.dirty.get_mut(row as usize) {
            *d = true;
        }
    }

    #[cfg_attr(windows, allow(dead_code))]
    pub fn mark_all(&mut self) {
        self.dirty.iter_mut().for_each(|d| *d = true);
    }

    fn clear_range(&mut self, from: usize, to: usize) {
        let cols = self.cols.max(1) as usize;
        for i in from..to.min(self.cells.len()) {
            self.cells[i] = GCell::blank();
            self.dirty[i / cols] = true;
        }
    }

    /// Select from `a` to `b` (either order), or nothing; marks the rows to repaint.
    pub fn set_selection(&mut self, sel: Option<((u16, u16), (u16, u16))>) {
        let sel = sel.map(|(a, b)| if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) });
        if sel == self.selection {
            return;
        }
        for (s, e) in [self.selection, sel].into_iter().flatten() {
            for r in s.1..=e.1 {
                self.mark(r);
            }
        }
        self.selection = sel;
    }

    pub fn is_selected(&self, x: u16, y: u16) -> bool {
        match self.selection {
            Some((s, e)) => (y, x) >= (s.1, s.0) && (y, x) <= (e.1, e.0),
            None => false,
        }
    }

    /// Colours to draw a cell with: swapped while it is selected.
    pub fn colors(&self, x: u16, y: u16, c: &GCell) -> (u32, u32) {
        if self.is_selected(x, y) {
            (c.bg, c.fg)
        } else {
            (c.fg, c.bg)
        }
    }

    /// The selected text, one line per row, without trailing spaces.
    pub fn selected_text(&self) -> String {
        let Some((s, e)) = self.selection else { return String::new() };
        let mut lines = Vec::new();
        for y in s.1..=e.1.min(self.rows.saturating_sub(1)) {
            let cells = self.row(y);
            let from = if y == s.1 { s.0 as usize } else { 0 };
            let to = if y == e.1 { (e.0 as usize + 1).min(cells.len()) } else { cells.len() };
            let mut line = String::new();
            for c in cells.get(from..to).unwrap_or(&[]) {
                if c.sym == images::IMAGE_CELL {
                    line.push(' ');
                } else {
                    // Cells covered by a wide character before them are empty.
                    line.push_str(&c.sym);
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    pub fn row(&self, row: u16) -> &[GCell] {
        let cols = self.cols as usize;
        &self.cells[row as usize * cols..(row as usize + 1) * cols]
    }
}

pub(super) type Shared = Arc<Mutex<Screen>>;

pub(super) fn lock(s: &Shared) -> MutexGuard<'_, Screen> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- mouse selection ------------------------------------------------------------------

/// Selecting text by dragging with the left mouse button. A plain click
/// (no movement to another cell) selects nothing.
#[derive(Default)]
pub(super) struct Selector {
    anchor: Option<(u16, u16)>,
    dragged: bool,
}

impl Selector {
    pub fn press(&mut self, screen: &Shared, cell: (u16, u16)) {
        self.anchor = Some(cell);
        self.dragged = false;
        lock(screen).set_selection(None);
    }

    /// The mouse moved to `cell` with the button down. True if the selection changed.
    pub fn drag(&mut self, screen: &Shared, cell: (u16, u16)) -> bool {
        let Some(a) = self.anchor else { return false };
        if cell == a && !self.dragged {
            return false;
        }
        self.dragged = true;
        lock(screen).set_selection(Some((a, cell)));
        true
    }

    /// The button was let go: the selected text to copy, if any.
    pub fn release(&mut self, screen: &Shared) -> Option<String> {
        let dragged = std::mem::take(&mut self.dragged);
        self.anchor = None;
        let text = if dragged { lock(screen).selected_text() } else { String::new() };
        (!text.is_empty()).then_some(text)
    }
}

// ---- window-wide hooks --------------------------------------------------------------

type Hook = Box<dyn Fn() + Send>;

/// Rings the bell in the open window; set while a window is open.
static BELL: Mutex<Option<Hook>> = Mutex::new(None);

pub(super) fn set_bell(hook: Option<Hook>) {
    *BELL.lock().unwrap_or_else(|e| e.into_inner()) = hook;
}

/// Whether a GUI window is open (so there is no terminal to use).
pub fn active() -> bool {
    BELL.lock().map(|b| b.is_some()).unwrap_or(false)
}

/// Notification sound or a flash of the window.
pub fn bell() {
    if let Some(b) = BELL.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        b();
    }
}

/// Show an error in a message box (Windows has no console in the GUI build).
#[cfg(windows)]
pub fn show_error(text: &str) {
    win32::message_box(text);
}

// ---- backend ----------------------------------------------------------------------

/// Draws into the shared grid; `wake` asks the window to repaint.
pub struct GuiBackend {
    screen: Shared,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Backend for GuiBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut s = lock(&self.screen);
        for (x, y, c) in content {
            if x < s.cols && y < s.rows {
                let i = y as usize * s.cols as usize + x as usize;
                s.cells[i] = GCell::from_cell(c);
                s.dirty[y as usize] = true;
            }
        }
        Ok(())
    }

    fn append_lines(&mut self, _n: u16) -> io::Result<()> {
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        let mut s = lock(&self.screen);
        if s.cursor_visible {
            s.cursor_visible = false;
            let row = s.cursor.1;
            s.mark(row);
        }
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        let mut s = lock(&self.screen);
        if !s.cursor_visible {
            s.cursor_visible = true;
            let row = s.cursor.1;
            s.mark(row);
        }
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        let (x, y) = lock(&self.screen).cursor;
        Ok(Position { x, y })
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let p = position.into();
        let mut s = lock(&self.screen);
        let old = s.cursor.1;
        s.cursor = (p.x, p.y);
        s.mark(old);
        s.mark(p.y);
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        let mut s = lock(&self.screen);
        let n = s.cells.len();
        s.clear_range(0, n);
        Ok(())
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        let mut s = lock(&self.screen);
        let cols = s.cols as usize;
        let (x, y) = (s.cursor.0 as usize, s.cursor.1 as usize);
        let at = y * cols + x;
        let n = s.cells.len();
        match clear_type {
            ClearType::All => s.clear_range(0, n),
            ClearType::AfterCursor => s.clear_range(at, n),
            ClearType::BeforeCursor => s.clear_range(0, at),
            ClearType::CurrentLine => s.clear_range(y * cols, (y + 1) * cols),
            ClearType::UntilNewLine => s.clear_range(at, (y + 1) * cols),
        }
        Ok(())
    }

    fn size(&self) -> io::Result<Size> {
        let s = lock(&self.screen);
        Ok(Size { width: s.cols, height: s.rows })
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let s = lock(&self.screen);
        Ok(WindowSize {
            columns_rows: Size { width: s.cols, height: s.rows },
            pixels: Size { width: (s.cols as i32 * s.cell_w) as u16, height: (s.rows as i32 * s.cell_h) as u16 },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        // Images that appeared, moved or changed need their rows repainted.
        let placed = images::placements();
        {
            let mut s = lock(&self.screen);
            if placed != s.images {
                let rows: Vec<u16> = s.images.iter().chain(placed.iter()).flat_map(|p| p.area.top()..p.area.bottom()).collect();
                for r in rows {
                    s.mark(r);
                }
                s.images = placed;
            }
        }
        (self.wake)();
        Ok(())
    }
}

// ---- events -----------------------------------------------------------------------

/// Input from the window, as crossterm events.
pub struct GuiEvents {
    rx: Receiver<Event>,
    /// Closes the window when the app is done.
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

impl GuiEvents {
    /// The next event, waiting up to `timeout`. Fails with `BrokenPipe`
    /// once the window is gone.
    pub fn next(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        let r = if timeout.is_zero() {
            self.rx.try_recv().map_err(|e| match e {
                mpsc::TryRecvError::Empty => RecvTimeoutError::Timeout,
                mpsc::TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
            })
        } else {
            self.rx.recv_timeout(timeout)
        };
        match r {
            Ok(ev) => Ok(Some(ev)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(io::Error::new(io::ErrorKind::BrokenPipe, "window closed")),
        }
    }
}

impl Drop for GuiEvents {
    fn drop(&mut self) {
        set_bell(None);
        images::set_pixel_output(None);
        if let Some(f) = self.on_drop.take() {
            f();
        }
    }
}

// ---- box drawing and block characters ---------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Shape {
    /// Line weights up, right, down, left: 0 none, 1 light, 2 heavy, 3 double.
    Lines([u8; 4]),
    /// A filled part of the cell, in eighths: x0, x1, y0, y1.
    Block(u8, u8, u8, u8),
    /// Shade with this many quarters of the foreground colour.
    Shade(u32),
    /// Braille dots (the bits of U+2800 + n).
    Braille(u8),
}

fn shape_of(c: char) -> Option<Shape> {
    let l = |u, r, d, l| Some(Shape::Lines([u, r, d, l]));
    match c as u32 {
        0x2500 | 0x2504 | 0x2508 | 0x254C => l(0, 1, 0, 1),
        0x2501 | 0x2505 | 0x2509 | 0x254D => l(0, 2, 0, 2),
        0x2502 | 0x2506 | 0x250A | 0x254E => l(1, 0, 1, 0),
        0x2503 | 0x2507 | 0x250B | 0x254F => l(2, 0, 2, 0),
        0x250C | 0x256D => l(0, 1, 1, 0),
        0x250F => l(0, 2, 2, 0),
        0x2510 | 0x256E => l(0, 0, 1, 1),
        0x2513 => l(0, 0, 2, 2),
        0x2514 | 0x2570 => l(1, 1, 0, 0),
        0x2517 => l(2, 2, 0, 0),
        0x2518 | 0x256F => l(1, 0, 0, 1),
        0x251B => l(2, 0, 0, 2),
        0x251C => l(1, 1, 1, 0),
        0x2523 => l(2, 2, 2, 0),
        0x2524 => l(1, 0, 1, 1),
        0x252B => l(2, 0, 2, 2),
        0x252C => l(0, 1, 1, 1),
        0x2533 => l(0, 2, 2, 2),
        0x2534 => l(1, 1, 0, 1),
        0x253B => l(2, 2, 0, 2),
        0x253C => l(1, 1, 1, 1),
        0x254B => l(2, 2, 2, 2),
        0x2550 => l(0, 3, 0, 3),
        0x2551 => l(3, 0, 3, 0),
        0x2554 => l(0, 3, 3, 0),
        0x2557 => l(0, 0, 3, 3),
        0x255A => l(3, 3, 0, 0),
        0x255D => l(3, 0, 0, 3),
        0x2560 => l(3, 3, 3, 0),
        0x2563 => l(3, 0, 3, 3),
        0x2566 => l(0, 3, 3, 3),
        0x2569 => l(3, 3, 0, 3),
        0x256C => l(3, 3, 3, 3),
        0x2574 => l(0, 0, 0, 1),
        0x2575 => l(1, 0, 0, 0),
        0x2576 => l(0, 1, 0, 0),
        0x2577 => l(0, 0, 1, 0),
        0x2578 => l(0, 0, 0, 2),
        0x2579 => l(2, 0, 0, 0),
        0x257A => l(0, 2, 0, 0),
        0x257B => l(0, 0, 2, 0),
        0x2580 => Some(Shape::Block(0, 8, 0, 4)),
        n @ 0x2581..=0x2587 => Some(Shape::Block(0, 8, 8 - (n - 0x2580) as u8, 8)),
        0x2588 => Some(Shape::Block(0, 8, 0, 8)),
        n @ 0x2589..=0x258F => Some(Shape::Block(0, 8 - (n - 0x2588) as u8, 0, 8)),
        0x2590 => Some(Shape::Block(4, 8, 0, 8)),
        0x2591 => Some(Shape::Shade(1)),
        0x2592 => Some(Shape::Shade(2)),
        0x2593 => Some(Shape::Shade(3)),
        0x2594 => Some(Shape::Block(0, 8, 0, 1)),
        0x2595 => Some(Shape::Block(7, 8, 0, 8)),
        n @ 0x2800..=0x28FF => Some(Shape::Braille((n - 0x2800) as u8)),
        _ => None,
    }
}

/// A rectangle in pixels (right and bottom exclusive).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct PxRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Draw a shape in the cell `r` (already filled with the background) as
/// filled rectangles.
pub(super) fn draw_shape(shape: Shape, fg: u32, bg: u32, r: PxRect, fill: &mut dyn FnMut(PxRect, u32)) {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    match shape {
        Shape::Block(x0, x1, y0, y1) => {
            let px = |e: u8| r.left + w * e as i32 / 8;
            let py = |e: u8| r.top + h * e as i32 / 8;
            fill(PxRect { left: px(x0), right: px(x1), top: py(y0), bottom: py(y1) }, fg);
        }
        Shape::Shade(q) => fill(r, blend(fg, bg, q)),
        Shape::Braille(bits) => {
            // 2 columns x 4 rows of square dots.
            const BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
            // Dots fill about 3/4 of their 2x4 grid square, like Braille fonts.
            let d = ((w / 2).min(h / 4) * 3 / 4).max(1);
            for (row, rb) in BITS.iter().enumerate() {
                for (col, b) in rb.iter().enumerate() {
                    if bits & b != 0 {
                        let cx = r.left + w * (1 + 2 * col as i32) / 4;
                        let cy = r.top + h * (1 + 2 * row as i32) / 8;
                        fill(PxRect { left: cx - d / 2, top: cy - d / 2, right: cx - d / 2 + d, bottom: cy - d / 2 + d }, fg);
                    }
                }
            }
        }
        Shape::Lines(arms) => {
            let t = (w / 8).max(1);
            let cx = r.left + (w - t) / 2;
            let cy = r.top + (h - t) / 2;
            let gap = t + 1;
            for (dir, &weight) in arms.iter().enumerate() {
                let (th, offsets): (i32, &[i32]) = match weight {
                    0 => continue,
                    1 => (t, &[0]),
                    2 => (t * 2, &[0]),
                    _ => (t, &[-1, 1]),
                };
                for &o in offsets {
                    let o = o * gap;
                    // Heavy lines grow around the centre.
                    let (lx, ly) = (cx + o - (th - t) / 2, cy + o - (th - t) / 2);
                    let reach = if weight == 3 { gap + t } else { th };
                    let back = if weight == 3 { gap } else { 0 };
                    let rect = match dir {
                        0 => PxRect { left: lx, right: lx + th, top: r.top, bottom: cy + reach },
                        1 => PxRect { left: cx - back, right: r.right, top: ly, bottom: ly + th },
                        2 => PxRect { left: lx, right: lx + th, top: cy - back, bottom: r.bottom },
                        _ => PxRect { left: r.left, right: cx + reach, top: ly, bottom: ly + th },
                    };
                    fill(rect, fg);
                }
            }
        }
    }
}

// ---- images -------------------------------------------------------------------------

/// Scaled copies of the images on screen, so they aren't scaled on every paint.
#[derive(Default)]
pub(super) struct ImageCache {
    /// (original, scaled)
    scaled: Vec<(Arc<Pixels>, Arc<Pixels>)>,
}

impl ImageCache {
    fn scaled(&mut self, p: &Placement) -> Arc<Pixels> {
        if let Some((_, s)) = self.scaled.iter().find(|(o, s)| Arc::ptr_eq(o, &p.pixels) && s.width == p.width && s.height == p.height) {
            return s.clone();
        }
        let s = Arc::new(images::resample(&p.pixels, p.width, p.height));
        if self.scaled.len() >= 256 {
            self.scaled.remove(0);
        }
        self.scaled.push((p.pixels.clone(), s.clone()));
        s
    }

    /// The pixels (0x00RRGGBB, `cells.len() * cw` wide and `ch` high) of the
    /// image `p` in the given cells of `row`, blended onto `bg`.
    pub fn block(&mut self, p: &Placement, row: u16, cells: std::ops::Range<u16>, (cw, ch): (i32, i32), bg: u32) -> Vec<u32> {
        let (x0, x1) = (cells.start, cells.end);
        let img = self.scaled(p);
        let w = (x1 - x0) as i32 * cw;
        let mut out = vec![bg; (w * ch) as usize];
        let ox = (x0 - p.area.x) as i32 * cw;
        let oy = (row - p.area.y) as i32 * ch;
        for y in 0..ch {
            let iy = oy + y;
            if iy < 0 || iy >= img.height as i32 {
                continue;
            }
            for x in 0..w {
                let ix = ox + x;
                if ix < 0 || ix >= img.width as i32 {
                    continue;
                }
                let [r, g, b, a] = img.get(ix as u32, iy as u32);
                let a = a as u32;
                let mix = |c: u8, s: u32| (c as u32 * a + ((bg >> s) & 0xff) * (255 - a)) / 255;
                out[(y * w + x) as usize] = mix(r, 16) << 16 | mix(g, 8) << 8 | mix(b, 0);
            }
        }
        out
    }
}

/// Runs of image cells in a row: (placement index, first cell, end cell).
pub(super) fn image_runs(s: &Screen, row: u16) -> Vec<(usize, u16, u16)> {
    let cells = s.row(row);
    let mut runs: Vec<(usize, u16, u16)> = Vec::new();
    for (x, c) in cells.iter().enumerate() {
        if c.sym != images::IMAGE_CELL {
            continue;
        }
        let x = x as u16;
        let Some(i) = s.images.iter().position(|p| p.area.contains(ratatui::layout::Position { x, y: row })) else { continue };
        match runs.last_mut() {
            Some((pi, _, end)) if *pi == i && *end == x => *end = x + 1,
            _ => runs.push((i, x, x + 1)),
        }
    }
    runs
}

/// Shared parts handed to a platform window: the grid, the event sender and
/// the receiving end wrapped for the app.
pub(super) fn channel() -> (mpsc::Sender<Event>, Receiver<Event>) {
    mpsc::channel()
}

pub(super) fn backend(screen: Shared, wake: Arc<dyn Fn() + Send + Sync>) -> GuiBackend {
    GuiBackend { screen, wake }
}

pub(super) fn events(rx: Receiver<Event>, on_drop: Box<dyn FnOnce() + Send>) -> GuiEvents {
    GuiEvents { rx, on_drop: Some(on_drop) }
}

/// Draw one frame the way the window does and save it as a PNG (used for
/// the README screenshots, see `screenshots` in src/ui/tests.rs).
/// `pixels`: images as real pixels like the window shows them; otherwise as
/// in a terminal (block characters etc.).
#[cfg(all(test, feature = "images", not(windows)))]
pub(crate) fn save_png(cols: u16, rows: u16, px: f32, pixels: bool, path: &std::path::Path, draw: impl FnOnce(&mut ratatui::Frame)) {
    use ratatui::backend::Backend as _;
    let mut r = soft::Renderer::new(px);
    images::set_pixel_output(pixels.then_some((r.cell_w as u16, r.cell_h as u16)));
    let screen: Shared = Arc::new(Mutex::new(Screen::new()));
    {
        let mut s = screen.lock().unwrap();
        s.resize(cols, rows);
        (s.cell_w, s.cell_h) = (r.cell_w, r.cell_h);
    }
    let mut term = ratatui::Terminal::new(backend(screen.clone(), Arc::new(|| {}))).unwrap();
    term.draw(draw).unwrap();
    term.backend_mut().flush().unwrap();
    images::set_pixel_output(None);
    let mut s = screen.lock().unwrap();
    s.cursor_visible = false;
    let (w, h) = (cols as usize * r.cell_w as usize, rows as usize * r.cell_h as usize);
    let mut pixels = vec![DEFAULT_BG; w * h];
    r.render(&mut soft::Canvas { pixels: &mut pixels, width: w, height: h }, &mut s);
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    image::RgbImage::from_raw(w as u32, h as u32, rgb).unwrap().save(path).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(shape_of('─'), Some(Shape::Lines([0, 1, 0, 1])));
        assert_eq!(shape_of('╭'), Some(Shape::Lines([0, 1, 1, 0])));
        assert_eq!(shape_of('▄'), Some(Shape::Block(0, 8, 4, 8)));
        assert_eq!(shape_of('▌'), Some(Shape::Block(0, 4, 0, 8)));
        assert_eq!(shape_of('⣿'), Some(Shape::Braille(0xff)));
        let mut dots = 0;
        draw_shape(Shape::Braille(0x41), 1, 0, PxRect { left: 0, top: 0, right: 8, bottom: 16 }, &mut |r, _| {
            assert!(r.left >= 0 && r.right <= 8 && r.top >= 0 && r.bottom <= 16 && r.right > r.left);
            dots += 1;
        });
        assert_eq!(dots, 2);
        assert_eq!(shape_of('a'), None);
        let mut n = 0;
        draw_shape(Shape::Lines([1, 1, 1, 1]), 1, 0, PxRect { left: 0, top: 0, right: 8, bottom: 16 }, &mut |r, _| {
            assert!(r.left >= 0 && r.right <= 8 && r.top >= 0 && r.bottom <= 16);
            n += 1;
        });
        assert_eq!(n, 4);
    }

    #[test]
    fn selection() {
        let screen: Shared = Arc::new(Mutex::new(Screen::new()));
        {
            let mut s = lock(&screen);
            s.resize(6, 3);
            for (y, t) in ["ab    ", "cdef  ", "gh  ij"].iter().enumerate() {
                for (x, ch) in t.chars().enumerate() {
                    s.cells[y * 6 + x].sym = ch.to_string();
                }
            }
            s.dirty.iter_mut().for_each(|d| *d = false);
        }
        let mut sel = Selector::default();
        // A click selects nothing.
        sel.press(&screen, (1, 0));
        assert_eq!(sel.release(&screen), None);
        // Dragging backwards from row 2 to row 0.
        sel.press(&screen, (1, 2));
        assert!(sel.drag(&screen, (1, 0)));
        assert!(lock(&screen).dirty.iter().all(|d| *d));
        assert_eq!(sel.release(&screen).as_deref(), Some("b\ncdef\ngh"));
        let s = lock(&screen);
        assert!(s.is_selected(5, 1) && !s.is_selected(0, 0) && !s.is_selected(2, 2));
        let c = &s.cells[0];
        assert_eq!(s.colors(0, 1, c), (c.bg, c.fg));
    }

    #[test]
    fn colors() {
        assert_eq!(color_value(Color::Rgb(1, 2, 3), 0), 0x010203);
        assert_eq!(color_value(Color::Reset, 7), 7);
        assert_eq!(blend(0xffffff, 0, 2), 0x7f7f7f);
    }
}

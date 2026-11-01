//! GUI window on Linux (X11, Wayland) and macOS, using winit and softbuffer.
//!
//! The window must run on the main thread (macOS requires it), so the app
//! runs on a second thread here. Text is drawn by `soft::Renderer`.

use std::io;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton as WinitButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{UserAttentionType, Window, WindowId};

use super::soft::{Canvas, Renderer};
use super::{backend, channel, events, lock, set_bell, GuiBackend, GuiConfig, GuiEvents, Screen, Selector, Shared};
use crate::config::Settings;

enum UserEvent {
    /// The app drew something.
    Redraw,
    Bell,
    /// The app is done.
    Quit,
}

/// Open the window and run `app` (on a second thread) with the backend to
/// draw with and the window's input events.
pub fn run(settings: &Settings, app: impl FnOnce(GuiBackend, GuiEvents) -> io::Result<()> + Send) -> io::Result<()> {
    let config = GuiConfig::new(settings);
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().map_err(io::Error::other)?;
    let proxy = Arc::new(Mutex::new(event_loop.create_proxy()));
    let send = move |p: &Arc<Mutex<winit::event_loop::EventLoopProxy<UserEvent>>>, e: UserEvent| {
        if let Ok(p) = p.lock() {
            let _ = p.send_event(e);
        }
    };
    let screen: Shared = Arc::new(Mutex::new(Screen::new()));
    // Images are painted as pixels; the window sets the exact cell size.
    let first = Renderer::new(config.font_size as f32);
    crate::images::set_pixel_output(Some((first.cell_w as u16, first.cell_h as u16)));
    // A size for the app's first frame; the window sets the real one.
    lock(&screen).resize(config.cols, config.rows);
    let (tx, rx) = channel();

    let p = proxy.clone();
    let wake = Arc::new(move || send(&p, UserEvent::Redraw));
    let p = proxy.clone();
    set_bell(Some(Box::new(move || send(&p, UserEvent::Bell))));
    let p = proxy.clone();
    let close = Box::new(move || send(&p, UserEvent::Quit));

    let mut handler = Handler {
        config,
        screen: screen.clone(),
        tx: Some(tx),
        window: None,
        surface: None,
        renderer: None,
        frame: Vec::new(),
        frame_size: (0, 0),
        mods: ModifiersState::empty(),
        mouse_px: (0.0, 0.0),
        last_cell: (u16::MAX, u16::MAX),
        buttons: [false; 3],
        wheel: 0.0,
        selector: Selector::default(),
        clipboard: None,
    };
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || app(backend(screen, wake), events(rx, close)));
        let r = event_loop.run_app(&mut handler);
        // If the window went away first, the app sees the closed channel and ends.
        handler.tx = None;
        let result = worker.join().unwrap_or_else(|_| Err(io::Error::other("the app thread panicked")));
        r.map_err(io::Error::other)?;
        result
    })
}

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

struct Handler {
    config: GuiConfig,
    screen: Shared,
    tx: Option<Sender<Event>>,
    window: Option<Rc<Window>>,
    surface: Option<Surface>,
    renderer: Option<Renderer>,
    /// The window contents, redrawn row by row as rows change.
    frame: Vec<u32>,
    frame_size: (u32, u32),
    mods: ModifiersState,
    mouse_px: (f64, f64),
    last_cell: (u16, u16),
    /// Left, right, middle held down.
    buttons: [bool; 3],
    /// Pixel scroll not yet turned into whole lines.
    wheel: f64,
    /// Text selection with the mouse.
    selector: Selector,
    /// Kept open: on X11, copied text is only available while its owner lives.
    clipboard: Option<arboard::Clipboard>,
}

impl Handler {
    fn send(&self, ev: Event) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(ev);
        }
    }

    fn key(&self, code: KeyCode, mods: KeyModifiers) {
        self.send(Event::Key(KeyEvent::new(code, mods)));
    }

    fn cell_size(&self) -> (i32, i32) {
        self.renderer.as_ref().map(|r| (r.cell_w, r.cell_h)).unwrap_or((8, 16))
    }

    fn set_renderer(&mut self, scale: f64) {
        let r = Renderer::new((self.config.font_size as f64 * scale) as f32);
        crate::images::set_pixel_output(Some((r.cell_w as u16, r.cell_h as u16)));
        let mut s = lock(&self.screen);
        s.cell_w = r.cell_w;
        s.cell_h = r.cell_h;
        s.mark_all();
        drop(s);
        self.renderer = Some(r);
    }

    fn resized(&mut self, size: PhysicalSize<u32>) {
        let (cw, ch) = self.cell_size();
        let cols = (size.width as i32 / cw).clamp(1, u16::MAX as i32) as u16;
        let rows = (size.height as i32 / ch).clamp(1, u16::MAX as i32) as u16;
        self.frame_size = (size.width.max(1), size.height.max(1));
        self.frame = vec![super::DEFAULT_BG; self.frame_size.0 as usize * self.frame_size.1 as usize];
        let changed = {
            let mut s = lock(&self.screen);
            let changed = (s.cols, s.rows) != (cols, rows);
            if changed {
                s.resize(cols, rows);
            }
            s.mark_all();
            changed
        };
        if changed {
            self.send(Event::Resize(cols, rows));
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn redraw(&mut self) {
        let (Some(surface), Some(renderer)) = (self.surface.as_mut(), self.renderer.as_mut()) else { return };
        let (w, h) = self.frame_size;
        if w == 0 || self.frame.is_empty() {
            return;
        }
        {
            let mut s = lock(&self.screen);
            let mut canvas = Canvas { pixels: &mut self.frame, width: w as usize, height: h as usize };
            renderer.render(&mut canvas, &mut s);
        }
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else { return };
        if surface.resize(nw, nh).is_err() {
            return;
        }
        if let Ok(mut buf) = surface.buffer_mut() {
            if buf.len() == self.frame.len() {
                buf.copy_from_slice(&self.frame);
                let _ = buf.present();
            }
        }
    }

    fn modifiers(&self) -> KeyModifiers {
        let mut m = KeyModifiers::NONE;
        if self.mods.shift_key() {
            m |= KeyModifiers::SHIFT;
        }
        if self.mods.control_key() {
            m |= KeyModifiers::CONTROL;
        }
        if self.mods.alt_key() {
            m |= KeyModifiers::ALT;
        }
        m
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()
    }

    fn paste(&mut self) {
        if let Some(text) = self.clipboard().and_then(|c| c.get_text().ok()) {
            self.send(Event::Paste(text.replace("\r\n", "\n")));
        }
    }

    fn copy(&mut self, text: &str) {
        if let Some(c) = self.clipboard() {
            let _ = c.set_text(text.to_string());
        }
    }

    /// Copy the selected text; false if nothing is selected.
    fn copy_selection(&mut self) -> bool {
        let text = lock(&self.screen).selected_text();
        if text.is_empty() {
            return false;
        }
        self.copy(&text);
        true
    }

    fn clear_selection(&mut self) {
        let mut s = lock(&self.screen);
        if s.selection.is_some() {
            s.set_selection(None);
            drop(s);
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn keyboard(&mut self, ev: &winit::event::KeyEvent) {
        if ev.state != ElementState::Pressed {
            return;
        }
        // Ctrl+C, Ctrl+Insert (or Cmd+C on macOS) copies while text is
        // selected; otherwise the key goes to the app.
        let copy_key = match &ev.logical_key {
            Key::Character(s) => {
                s.eq_ignore_ascii_case("c") && (self.mods.control_key() || (cfg!(target_os = "macos") && self.mods.super_key()))
            }
            Key::Named(NamedKey::Insert) => self.mods.control_key(),
            _ => false,
        };
        if copy_key && self.copy_selection() {
            self.clear_selection();
            return;
        }
        if !matches!(&ev.logical_key, Key::Named(NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super)) {
            self.clear_selection();
        }
        let mut mods = self.modifiers();
        let shift = self.mods.shift_key();
        // On macOS, Option types characters (e.g. @ on German keyboards), so
        // Cmd works as Alt for shortcuts on letter keys.
        let (char_alt, cmd) =
            if cfg!(target_os = "macos") { (self.mods.super_key(), self.mods.super_key()) } else { (self.mods.alt_key(), false) };
        match &ev.logical_key {
            Key::Named(named) => {
                if cmd {
                    mods |= KeyModifiers::ALT;
                }
                let code = match named {
                    NamedKey::Enter => KeyCode::Enter,
                    NamedKey::Tab if shift => KeyCode::BackTab,
                    NamedKey::Tab => KeyCode::Tab,
                    NamedKey::Backspace => KeyCode::Backspace,
                    NamedKey::Escape => KeyCode::Esc,
                    NamedKey::ArrowLeft => KeyCode::Left,
                    NamedKey::ArrowRight => KeyCode::Right,
                    NamedKey::ArrowUp => KeyCode::Up,
                    NamedKey::ArrowDown => KeyCode::Down,
                    NamedKey::Home => KeyCode::Home,
                    NamedKey::End => KeyCode::End,
                    NamedKey::PageUp => KeyCode::PageUp,
                    NamedKey::PageDown => KeyCode::PageDown,
                    NamedKey::Insert if shift => return self.paste(),
                    NamedKey::Insert => KeyCode::Insert,
                    NamedKey::Delete => KeyCode::Delete,
                    NamedKey::Space => KeyCode::Char(' '),
                    NamedKey::F1 => KeyCode::F(1),
                    NamedKey::F2 => KeyCode::F(2),
                    NamedKey::F3 => KeyCode::F(3),
                    NamedKey::F4 => KeyCode::F(4),
                    NamedKey::F5 => KeyCode::F(5),
                    NamedKey::F6 => KeyCode::F(6),
                    NamedKey::F7 => KeyCode::F(7),
                    NamedKey::F8 => KeyCode::F(8),
                    NamedKey::F9 => KeyCode::F(9),
                    NamedKey::F10 => KeyCode::F(10),
                    NamedKey::F11 => KeyCode::F(11),
                    NamedKey::F12 => KeyCode::F(12),
                    _ => return,
                };
                self.key(code, mods);
            }
            Key::Character(s) => {
                let Some(c) = s.chars().next() else { return };
                let lower = c.to_lowercase().next().unwrap_or(c);
                let ctrl = self.mods.control_key();
                if (ctrl && !char_alt && lower == 'v') || (cmd && lower == 'v') {
                    return self.paste();
                }
                if cmd && lower == 'q' {
                    return self.key(KeyCode::Char('q'), KeyModifiers::CONTROL);
                }
                if ctrl || char_alt {
                    // A shortcut: the key itself, not the text it would type.
                    let mut m = mods & (KeyModifiers::CONTROL | KeyModifiers::SHIFT);
                    if char_alt {
                        m |= KeyModifiers::ALT;
                    }
                    self.key(KeyCode::Char(if ctrl { lower } else { c }), m);
                    return;
                }
                let text = ev.text.as_ref().map(|t| t.as_str()).unwrap_or(s.as_str());
                for ch in text.chars().filter(|c| !c.is_control()) {
                    self.key(KeyCode::Char(ch), mods & KeyModifiers::SHIFT);
                }
            }
            _ => {}
        }
    }

    fn mouse(&mut self, kind: MouseEventKind) {
        let (column, row) = self.last_cell;
        if column == u16::MAX {
            return;
        }
        let modifiers = self.modifiers();
        self.send(Event::Mouse(MouseEvent { kind, column, row, modifiers }));
    }
}

impl ApplicationHandler<UserEvent> for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Size the window for the wanted grid at a scale of 1; the system scales it.
        let base = Renderer::new(self.config.font_size as f32);
        let size = LogicalSize::new(self.config.cols as f64 * base.cell_w as f64, self.config.rows as f64 * base.cell_h as f64);
        let attrs = Window::default_attributes().with_title("F-Term").with_inner_size(size);
        // The app id (Wayland) / WM_CLASS (X11) links the window to its desktop file,
        // so the desktop shows the right name and icon for it.
        #[cfg(all(unix, not(target_os = "macos")))]
        let attrs = winit::platform::x11::WindowAttributesExtX11::with_name(attrs, "io.github.tomthedragon.f-term", "f-term");
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("Could not open a window: {e}");
                event_loop.exit();
                return;
            }
        };
        let surface = softbuffer::Context::new(window.clone()).and_then(|ctx| Surface::new(&ctx, window.clone()));
        match surface {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                eprintln!("Could not draw in the window: {e}");
                event_loop.exit();
                return;
            }
        }
        self.set_renderer(window.scale_factor());
        let size = window.inner_size();
        self.window = Some(window);
        self.resized(size);
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            // Closing the window works like Ctrl+Q (which asks first if set to).
            WindowEvent::CloseRequested => self.key(KeyCode::Char('q'), KeyModifiers::CONTROL),
            WindowEvent::Resized(size) => self.resized(size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.set_renderer(scale_factor);
                if let Some(size) = self.window.as_ref().map(|w| w.inner_size()) {
                    self.resized(size);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Focused(true) => self.send(Event::FocusGained),
            WindowEvent::Focused(false) => self.send(Event::FocusLost),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => self.keyboard(&event),
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_px = (position.x, position.y);
                let (cw, ch) = self.cell_size();
                let s = lock(&self.screen);
                let col = ((position.x.max(0.0) as i32) / cw).min(s.cols.saturating_sub(1) as i32) as u16;
                let row = ((position.y.max(0.0) as i32) / ch).min(s.rows.saturating_sub(1) as i32) as u16;
                drop(s);
                if self.buttons[0] && self.selector.drag(&self.screen, (col, row)) {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                if (col, row) != self.last_cell {
                    self.last_cell = (col, row);
                    let kind = match self.buttons {
                        [true, _, _] => MouseEventKind::Drag(MouseButton::Left),
                        [_, true, _] => MouseEventKind::Drag(MouseButton::Right),
                        [_, _, true] => MouseEventKind::Drag(MouseButton::Middle),
                        _ => MouseEventKind::Moved,
                    };
                    self.mouse(kind);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (i, b) = match button {
                    WinitButton::Left => (0, MouseButton::Left),
                    WinitButton::Right => (1, MouseButton::Right),
                    WinitButton::Middle => (2, MouseButton::Middle),
                    _ => return,
                };
                let down = state == ElementState::Pressed;
                self.buttons[i] = down;
                // Dragging with the left button selects text; letting go copies it.
                if i == 0 && self.last_cell.0 != u16::MAX {
                    if down {
                        self.selector.press(&self.screen, self.last_cell);
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    } else if let Some(text) = self.selector.release(&self.screen) {
                        self.copy(&text);
                    }
                }
                self.mouse(if down { MouseEventKind::Down(b) } else { MouseEventKind::Up(b) });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => {
                        self.wheel += p.y / self.cell_size().1 as f64;
                        let whole = self.wheel.trunc();
                        self.wheel -= whole;
                        whole
                    }
                };
                let kind = if lines > 0.0 { MouseEventKind::ScrollUp } else { MouseEventKind::ScrollDown };
                for _ in 0..lines.abs().round() as u32 {
                    self.mouse(kind);
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Redraw => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            UserEvent::Bell => {
                if let Some(w) = &self.window {
                    w.request_user_attention(Some(UserAttentionType::Informational));
                }
            }
            UserEvent::Quit => event_loop.exit(),
        }
    }
}

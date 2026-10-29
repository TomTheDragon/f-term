//! Login screen: account/password, saved logins and character selection.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::input::{EditResult, LineEditor};
use super::widgets;
use super::App;
use crate::config::{deobfuscate, Accounts};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Credentials,
    Character,
}

pub struct LoginScreen {
    pub account: LineEditor,
    pub password: LineEditor,
    pub save_password: bool,
    pub auto_login: bool,
    pub focus: usize,
    pub busy: bool,
    pub error: String,
    pub stage: Stage,
    pub characters: Vec<String>,
    pub char_sel: usize,
    pub char_filter: LineEditor,
    pub saved: Vec<String>,
    /// Pick the remembered character automatically (auto-login).
    pub auto_pick: bool,
    autologin: bool,
}

const FIELDS: usize = 5;

impl LoginScreen {
    pub fn new(accounts: &Accounts, preferred: Option<&str>, no_autologin: bool) -> LoginScreen {
        let mut l = LoginScreen {
            account: LineEditor::new(),
            password: LineEditor::masked(),
            save_password: false,
            auto_login: false,
            focus: 0,
            busy: false,
            error: String::new(),
            stage: Stage::Credentials,
            characters: Vec::new(),
            char_sel: 0,
            char_filter: LineEditor::new(),
            saved: accounts.accounts.iter().map(|a| a.account.clone()).collect(),
            auto_pick: false,
            autologin: false,
        };
        let name = preferred.map(String::from).unwrap_or_else(|| accounts.last_account.clone());
        if let Some(acc) = accounts.find(&name) {
            l.load_saved(acc);
            l.autologin = acc.auto_login && !acc.password.is_empty() && !no_autologin;
            l.auto_pick = l.autologin;
        } else {
            l.account.set(&name);
        }
        l.focus = if l.account.is_empty() {
            0
        } else if l.password.is_empty() {
            1
        } else {
            4
        };
        l
    }

    fn load_saved(&mut self, acc: &crate::config::SavedAccount) {
        self.account.set(&acc.account);
        let pw = deobfuscate(&acc.password);
        self.password.set(&pw);
        self.save_password = !pw.is_empty();
        self.auto_login = acc.auto_login;
    }

    pub fn wants_autologin(&self) -> bool {
        self.autologin && !self.account.is_empty() && !self.password.is_empty()
    }

    pub fn paste(&mut self, s: &str) {
        match (self.stage, self.focus) {
            (Stage::Credentials, 0) => self.account.insert_str(s.trim()),
            (Stage::Credentials, 1) => self.password.insert_str(s.trim_end_matches(['\r', '\n'])),
            (Stage::Character, _) => self.char_filter.insert_str(s.trim()),
            _ => {}
        }
    }

    pub(super) fn filtered(&self) -> Vec<String> {
        let f = self.char_filter.text().to_lowercase();
        self.characters.iter().filter(|c| c.to_lowercase().contains(&f)).cloned().collect()
    }
}

impl App {
    pub(super) fn login_key(&mut self, key: KeyEvent) {
        let accounts = self.accounts.clone();
        let Some(l) = &mut self.login else { return };
        if l.busy {
            if key.code == KeyCode::Esc {
                l.busy = false;
                self.api = None;
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match l.stage {
            Stage::Credentials => {
                match key.code {
                    KeyCode::Tab | KeyCode::Down if key.code == KeyCode::Tab || l.focus != 0 => {
                        l.focus = (l.focus + 1) % FIELDS;
                        return;
                    }
                    KeyCode::BackTab | KeyCode::Up if key.code == KeyCode::BackTab || l.focus != 0 => {
                        l.focus = (l.focus + FIELDS - 1) % FIELDS;
                        return;
                    }
                    KeyCode::Esc => {
                        self.quit = true;
                        return;
                    }
                    KeyCode::Char('d') if ctrl && l.focus == 0 => {
                        // Forget the selected saved account.
                        let name = l.account.text().to_string();
                        self.accounts.remove(&name);
                        let _ = self.accounts.save();
                        if let Some(l) = &mut self.login {
                            l.saved.retain(|a| !a.eq_ignore_ascii_case(&name));
                            l.password.clear();
                            l.save_password = false;
                            l.auto_login = false;
                            l.error = format!("Forgot saved login for {name}.");
                        }
                        return;
                    }
                    _ => {}
                }
                match l.focus {
                    0 => match key.code {
                        // Cycle through saved accounts.
                        KeyCode::Up | KeyCode::Down if !l.saved.is_empty() => {
                            let cur = l.saved.iter().position(|a| a.eq_ignore_ascii_case(l.account.text()));
                            let n = l.saved.len();
                            let next = match (cur, key.code) {
                                (None, _) => 0,
                                (Some(i), KeyCode::Down) => (i + 1) % n,
                                (Some(i), _) => (i + n - 1) % n,
                            };
                            let name = l.saved[next].clone();
                            if let Some(acc) = accounts.find(&name) {
                                l.load_saved(acc);
                            }
                        }
                        _ => {
                            if let EditResult::Submit = l.account.handle(key) {
                                l.focus = 1;
                            }
                        }
                    },
                    1 => {
                        if let EditResult::Submit = l.password.handle(key) {
                            self.submit_login();
                        }
                    }
                    2 => {
                        if matches!(key.code, KeyCode::Char(' ') | KeyCode::Enter) {
                            l.save_password = !l.save_password;
                            if !l.save_password {
                                l.auto_login = false;
                            }
                        }
                    }
                    3 => {
                        if matches!(key.code, KeyCode::Char(' ') | KeyCode::Enter) {
                            l.auto_login = !l.auto_login;
                            if l.auto_login {
                                l.save_password = true;
                            }
                        }
                    }
                    _ => {
                        if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
                            self.submit_login();
                        }
                    }
                }
            }
            Stage::Character => {
                let list = l.filtered();
                match key.code {
                    KeyCode::Up => l.char_sel = l.char_sel.saturating_sub(1),
                    KeyCode::Down => l.char_sel = (l.char_sel + 1).min(list.len().saturating_sub(1)),
                    KeyCode::PageUp => l.char_sel = l.char_sel.saturating_sub(10),
                    KeyCode::PageDown => l.char_sel = (l.char_sel + 10).min(list.len().saturating_sub(1)),
                    KeyCode::Esc => {
                        l.stage = Stage::Credentials;
                        l.char_filter.clear();
                        self.api = None;
                    }
                    KeyCode::Enter => {
                        if let Some(c) = list.get(l.char_sel.min(list.len().saturating_sub(1))).cloned() {
                            self.choose_character(&c);
                        }
                    }
                    _ => {
                        if let EditResult::Changed = l.char_filter.handle(key) {
                            l.char_sel = 0;
                        }
                    }
                }
            }
        }
    }

    fn submit_login(&mut self) {
        let Some(l) = &mut self.login else { return };
        let account = l.account.text().trim().to_string();
        let password = l.password.text().to_string();
        if account.is_empty() || password.is_empty() {
            l.error = "Enter your F-List account name and password.".into();
            return;
        }
        // The user pressed the button: pick the remembered character only with auto-login.
        l.auto_pick = l.auto_login;
        self.start_login(&account, &password);
    }

    pub(super) fn draw_login(&mut self, f: &mut Frame, area: Rect) {
        let ascii = self.settings.ascii_borders;
        let Some(l) = &self.login else { return };
        let buf = f.buffer_mut();
        let title_lines = [
            " _____     _____                   ",
            "|  ___|   |_   _|__ _ __ _ __ ___  ",
            "| |_ _____  | |/ _ \\ '__| '_ ` _ \\ ",
            "|  _|_____| | |  __/ |  | | | | | |",
            "|_|         |_|\\___|_|  |_| |_| |_|",
        ];
        let logo_h = if area.height > 28 { title_lines.len() as u16 + 1 } else { 0 };
        let h = match l.stage {
            Stage::Credentials => 14,
            Stage::Character => (l.characters.len() as u16 + 8).clamp(10, area.height.saturating_sub(logo_h + 2).max(10)),
        };
        let box_area = widgets::centered(Rect::new(area.x, area.y + logo_h, area.width, area.height.saturating_sub(logo_h)), 60, h);
        if logo_h > 0 {
            for (i, t) in title_lines.iter().enumerate() {
                let x = area.x + area.width.saturating_sub(35) / 2;
                buf.set_string(x, box_area.y.saturating_sub(logo_h) + i as u16, t, Style::default().fg(Color::Cyan));
            }
        }
        let title = match l.stage {
            Stage::Credentials => "F-Chat login".to_string(),
            Stage::Character => format!("Choose a character ({})", l.characters.len()),
        };
        let inner = widgets::popup(buf, box_area, &title, ascii);
        let label = Style::default().fg(Color::Gray);
        let focus_st = |on: bool| {
            if on {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 40))
            }
        };
        let mut cursor = None;
        self.hit.login_fields.clear();
        self.hit.login_list = Rect::default();
        match l.stage {
            Stage::Credentials => {
                let fw = inner.width.saturating_sub(14);
                let mut y = inner.y + 1;
                for (i, (name, ed)) in [("Account", &l.account), ("Password", &l.password)].iter().enumerate() {
                    buf.set_string(inner.x + 1, y, format!("{name:>10}"), label);
                    let (lines, (cx, _)) = ed.layout(fw.max(4) * 4);
                    let shown: String = lines.first().cloned().unwrap_or_default();
                    let skip = (cx as usize + 1).saturating_sub(fw as usize);
                    let visible: String = shown.chars().skip(skip).collect();
                    buf.set_style(Rect::new(inner.x + 12, y, fw, 1), focus_st(l.focus == i));
                    self.hit.login_fields.push((Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), 1), i));
                    buf.set_stringn(inner.x + 12, y, &visible, fw as usize, focus_st(l.focus == i));
                    if l.focus == i {
                        cursor = Some((inner.x + 12 + (cx as usize - skip) as u16, y));
                    }
                    y += 2;
                }
                let cb = |b: bool| if b { "[x]" } else { "[ ]" };
                buf.set_string(inner.x + 12, y, format!("{} Save password", cb(l.save_password)), focus_st(l.focus == 2));
                self.hit.login_fields.push((Rect::new(inner.x + 12, y, 17, 1), 2));
                y += 1;
                buf.set_string(inner.x + 12, y, format!("{} Log in automatically", cb(l.auto_login)), focus_st(l.focus == 3));
                self.hit.login_fields.push((Rect::new(inner.x + 12, y, 25, 1), 3));
                y += 2;
                self.hit.login_fields.push((Rect::new(inner.x + 12, y, 15, 1), 4));
                buf.set_string(
                    inner.x + 12,
                    y,
                    if l.busy { "  Logging in…  " } else { "  Log in  " },
                    focus_st(l.focus == 4).add_modifier(Modifier::BOLD),
                );
                y += 2;
                if !l.error.is_empty() {
                    buf.set_stringn(inner.x + 1, y, &l.error, inner.width as usize - 2, Style::default().fg(Color::LightRed));
                } else if !l.saved.is_empty() && l.focus == 0 {
                    buf.set_stringn(
                        inner.x + 1,
                        y,
                        "↑/↓ switch saved logins · Ctrl+D forget this login",
                        inner.width as usize - 2,
                        Style::default().fg(Color::DarkGray),
                    );
                } else if l.save_password {
                    buf.set_stringn(
                        inner.x + 1,
                        y,
                        "Saved passwords are only obfuscated on disk.",
                        inner.width as usize - 2,
                        Style::default().fg(Color::DarkGray),
                    );
                }
                widgets::draw_hints(
                    buf,
                    inner.x + 1,
                    inner.bottom() - 1,
                    &[("Tab", "next"), ("Enter", "log in"), ("Esc", "quit")],
                    inner.width - 1,
                );
            }
            Stage::Character => {
                buf.set_string(inner.x + 1, inner.y, "Filter: ", label);
                buf.set_string(inner.x + 9, inner.y, l.char_filter.text(), Style::default().fg(Color::Yellow));
                cursor = Some((inner.x + 9 + widgets::str_width(&l.char_filter.text()[..l.char_filter.cursor()]) as u16, inner.y));
                let list_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(4));
                let items: Vec<Line<'static>> = l.filtered().into_iter().map(|c| Line::from(Span::raw(c))).collect();
                let sel = l.char_sel;
                self.hit.login_list_offset = widgets::list(buf, list_area, &items, sel, true);
                self.hit.login_list = list_area;
                widgets::draw_hints(
                    buf,
                    inner.x + 1,
                    inner.bottom() - 1,
                    &[("↑↓", "select"), ("Enter", "connect"), ("type", "filter"), ("Esc", "back")],
                    inner.width - 1,
                );
            }
        }
        if let Some(pos) = cursor {
            if self.dialogs.is_empty() {
                f.set_cursor_position(pos);
            }
        }
    }
}

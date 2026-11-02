//! Modal dialogs.

use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::actions::{Action, PromptKind};
use super::input::{EditResult, LineEditor};
use super::profile::ProfileState;
use super::widgets::{self, trunc};
use super::{ApiReply, App};
use crate::api::FriendLists;
use crate::bbcode;
use crate::commands::{self, Perm};
use crate::config::{GraphicsProtocol, ImageMode, SETTINGS};
use crate::logs;
use crate::protocol;
use crate::state::Status;

pub struct MenuItem {
    pub label: String,
    pub action: Action,
    /// Triggered with the Delete key.
    pub alt: Option<Action>,
}

impl MenuItem {
    pub fn new(label: &str, action: Action) -> MenuItem {
        MenuItem { label: label.to_string(), action, alt: None }
    }
}

pub struct PromptState {
    pub title: String,
    pub editor: LineEditor,
    pub kind: PromptKind,
    pub extra: String,
}

#[derive(Default)]
pub struct ChannelsState {
    pub tab: usize,
    pub filter: LineEditor,
    pub sel: usize,
    pub sort_by_name: bool,
}

#[derive(Default)]
pub struct FriendsState {
    pub tab: usize,
    pub sel: usize,
    pub data: Option<Result<FriendLists, String>>,
}

#[derive(Default)]
pub struct SettingsState {
    pub sel: usize,
}

pub struct StatusState {
    pub status: Status,
    pub msg: LineEditor,
    pub recent: Vec<String>,
    pub focus: usize,
    pub recent_sel: usize,
}

impl StatusState {
    pub fn new(status: Status, msg: &str, recent: Vec<String>) -> StatusState {
        let status = if status == Status::Crown || status == Status::Offline { Status::Online } else { status };
        StatusState { status, msg: LineEditor::with_text(msg), recent, focus: 1, recent_sel: 0 }
    }
}

#[derive(Default)]
pub struct LogsState {
    pub stage: usize,
    pub convs: Vec<(String, String)>,
    pub filter: LineEditor,
    pub sel: usize,
    pub conv_key: String,
    pub conv_title: String,
    pub days: Vec<String>,
    pub day_sel: usize,
    pub day: String,
    pub lines: Vec<String>,
    pub scroll: usize,
    /// Results of a search over all logs (stage 3).
    pub hits: Vec<logs::Hit>,
    pub hit_sel: usize,
    pub query: String,
    /// The day view was opened from the search results.
    pub from_search: bool,
}

pub const LOG_SEARCH_LIMIT: usize = 500;

impl LogsState {
    /// Search all logs and show the results.
    pub fn search(&mut self, me: &str, query: &str) {
        self.query = query.trim().to_string();
        self.hits = logs::search(me, &self.query, LOG_SEARCH_LIMIT);
        self.hit_sel = 0;
        self.stage = 3;
        self.from_search = false;
        self.filter.clear();
    }

    pub fn new(me: &str) -> LogsState {
        LogsState { convs: logs::conversations(me), ..Default::default() }
    }

    fn filtered_convs(&self) -> Vec<(String, String)> {
        let f = self.filter.text().to_lowercase();
        self.convs.iter().filter(|(_, t)| t.to_lowercase().contains(&f)).cloned().collect()
    }

    pub fn open_conv(&mut self, me: &str, idx_in_all: usize) {
        let (k, t) = self.convs[idx_in_all].clone();
        self.days = logs::days(me, &k);
        self.conv_key = k;
        self.conv_title = t;
        self.day_sel = 0;
        self.stage = 1;
        self.filter.clear();
    }

    fn filtered_lines(&self) -> Vec<String> {
        let f = self.filter.text().to_lowercase();
        if f.is_empty() {
            return self.lines.clone();
        }
        self.lines.iter().filter(|l| l.to_lowercase().contains(&f)).cloned().collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchFocus {
    #[default]
    Options,
    Results,
}

#[derive(Default)]
pub struct SearchState {
    /// (field key, label, options as (id, label))
    pub cats: Vec<(String, String, Vec<(String, String)>)>,
    pub selected: Vec<BTreeSet<String>>,
    pub cat: usize,
    pub filter: LineEditor,
    pub sel: usize,
    pub results: Vec<String>,
    pub result_sel: usize,
    pub status: String,
    pub focus: SearchFocus,
}

impl SearchState {
    pub fn load_fields(&mut self, v: &Value) {
        let order = [
            ("kinks", "Kinks"),
            ("genders", "Genders"),
            ("orientations", "Orientations"),
            ("languages", "Languages"),
            ("furryprefs", "Furry prefs"),
            ("roles", "Roles"),
            ("positions", "Positions"),
        ];
        self.cats.clear();
        for (key, label) in order {
            let Some(arr) = v.get(key).and_then(Value::as_array) else { continue };
            let mut opts: Vec<(String, String)> = arr
                .iter()
                .filter_map(|o| match o {
                    Value::String(s) => Some((s.clone(), s.clone())),
                    Value::Object(_) => {
                        let id = ["fetish_id", "id", "kink_id"].iter().map(|k| protocol::vs(o, k)).find(|s| !s.is_empty())?;
                        Some((id, protocol::vs(o, "name")))
                    }
                    _ => None,
                })
                .collect();
            if key == "kinks" {
                opts.sort_by_key(|o| o.1.to_lowercase());
            }
            self.cats.push((key.to_string(), label.to_string(), opts));
        }
        self.selected = vec![BTreeSet::new(); self.cats.len()];
        self.status = "Enter: toggle · Ctrl+S: search (at least one kink is required)".into();
    }

    fn visible(&self) -> Vec<(String, String)> {
        let Some((_, _, opts)) = self.cats.get(self.cat) else { return Vec::new() };
        let f = self.filter.text().to_lowercase();
        opts.iter().filter(|(_, l)| l.to_lowercase().contains(&f)).cloned().collect()
    }
}

pub enum Dialog {
    Help { scroll: usize },
    Channels(ChannelsState),
    Friends(FriendsState),
    Profile(Box<ProfileState>),
    Settings(SettingsState),
    Status(StatusState),
    Logs(Box<LogsState>),
    Search(Box<SearchState>),
    Menu { title: String, items: Vec<MenuItem>, sel: usize },
    Prompt(PromptState),
    Confirm { text: String, action: Action },
    Image { title: String, url: String },
    Text { title: String, lines: Vec<Line<'static>>, scroll: usize },
    Notes(Box<super::site::NotesState>),
    Messages(Box<super::site::MessagesState>),
    Personals(Box<super::site::PersonalsState>),
    Eicons(Box<super::eicons::EiconPicker>),
    Browse(Box<super::groups::BrowseState>),
    Groups(Box<super::groups::GroupsState>),
}

impl Dialog {
    pub fn help() -> Dialog {
        Dialog::Help { scroll: 0 }
    }
    pub fn channels(tab: usize, filter: &str) -> Dialog {
        Dialog::Channels(ChannelsState { tab, filter: LineEditor::with_text(filter), ..Default::default() })
    }
    pub fn menu(title: &str, items: Vec<MenuItem>) -> Dialog {
        Dialog::Menu { title: title.to_string(), items, sel: 0 }
    }
    pub fn confirm(text: &str, action: Action) -> Dialog {
        Dialog::Confirm { text: text.to_string(), action }
    }
    pub fn prompt(title: &str, initial: &str, kind: PromptKind, multiline: bool) -> Dialog {
        let mut editor = LineEditor::with_text(initial);
        editor.multiline = multiline;
        Dialog::Prompt(PromptState { title: title.to_string(), editor, kind, extra: String::new() })
    }
    pub fn text_bbcode(title: &str, text: &str) -> Dialog {
        let lines = bbcode::to_lines(text, Style::default(), &|_| Style::default().add_modifier(Modifier::BOLD), true);
        Dialog::Text { title: title.to_string(), lines, scroll: 0 }
    }
    pub fn text_plain(title: &str, text: &str) -> Dialog {
        Dialog::Text { title: title.to_string(), lines: text.lines().map(|l| Line::from(l.to_string())).collect(), scroll: 0 }
    }

    pub fn paste(&mut self, s: &str) {
        match self {
            Dialog::Prompt(p) => p.editor.insert_str(s),
            Dialog::Channels(c) => c.filter.insert_str(s.trim()),
            Dialog::Status(st) => st.msg.insert_str(s),
            Dialog::Logs(l) => l.filter.insert_str(s.trim()),
            Dialog::Search(se) => se.filter.insert_str(s.trim()),
            Dialog::Personals(p) => p.filter.insert_str(s.trim()),
            Dialog::Eicons(p) => p.filter.insert_str(s.trim()),
            Dialog::Groups(g) if g.level() == 0 => g.filter.insert_str(s.trim()),
            _ => {}
        }
    }

    /// Select a list row (mouse click). Returns true if it should be activated.
    pub fn select_row(&mut self, row: usize) -> bool {
        match self {
            Dialog::Menu { sel, items, .. } if row < items.len() => {
                *sel = row;
                true
            }
            Dialog::Channels(c) => {
                c.sel = row;
                true
            }
            Dialog::Friends(f) => {
                f.sel = row;
                true
            }
            Dialog::Settings(s) => {
                s.sel = row.min(SETTINGS.len() - 1);
                true
            }
            Dialog::Logs(l) => {
                if l.stage == 0 {
                    l.sel = row;
                } else if l.stage == 1 {
                    l.day_sel = row;
                } else if l.stage == 3 {
                    l.hit_sel = row.min(l.hits.len().saturating_sub(1));
                } else {
                    return false;
                }
                true
            }
            Dialog::Notes(n) if n.view.is_none() => {
                if n.focus_notes {
                    n.sel = row;
                } else {
                    n.list_sel = row.min(n.entries().len() - 1);
                    return false;
                }
                true
            }
            Dialog::Messages(m) => {
                if m.focus_items {
                    m.sel = row;
                } else {
                    m.cat_sel = row;
                    return false;
                }
                true
            }
            Dialog::Personals(p) => {
                p.sel = row;
                true
            }
            Dialog::Eicons(p) => {
                p.sel = row.min(p.results.len().saturating_sub(1));
                true
            }
            Dialog::Browse(b) => {
                b.sel = row;
                true
            }
            Dialog::Groups(g) => g.select_row(row),
            Dialog::Search(s) => {
                if s.focus == SearchFocus::Results {
                    s.result_sel = row;
                } else {
                    s.sel = row;
                }
                true
            }
            Dialog::Profile(p) => {
                if p.tab == 4 {
                    p.friend_sel = row;
                    return true;
                }
                false
            }
            _ => false,
        }
    }
}

/// A row of the friends window.
pub struct FriendRow {
    pub action: Option<Action>,
    pub line: Line<'static>,
    /// The character it is about, for the details.
    pub name: String,
    /// What they are to you, shown with the details.
    pub note: String,
}

/// The nearest row with an action at or after `sel` (or before it, going
/// up; the other way when there is none).
fn selectable(rows: &[FriendRow], sel: usize, down: bool) -> usize {
    let sel = sel.min(rows.len().saturating_sub(1));
    let after = (sel..rows.len()).find(|&i| rows[i].action.is_some());
    let before = (0..=sel).rev().find(|&i| rows.get(i).is_some_and(|r| r.action.is_some()));
    if down { after.or(before) } else { before.or(after) }.unwrap_or(sel)
}

fn help_lines(is_op: bool) -> Vec<Line<'static>> {
    let h = |s: &str| Line::from(Span::styled(s.to_string(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
    let k = |key: &str, desc: &str| {
        Line::from(vec![Span::styled(format!("{key:<22}"), Style::default().fg(Color::Yellow)), Span::raw(desc.to_string())])
    };
    let mut v = vec![
        h("Keys"),
        k("Enter", "Send message"),
        k("Alt+Enter / Ctrl+J", "New line in the message"),
        k("Tab / Shift+Tab", "Complete command, argument or name; cycle"),
        k("Up / Down", "Input history (scroll when history is empty)"),
        k("PageUp / PageDown", "Scroll messages"),
        k("Ctrl+Home / Ctrl+End", "Scroll to top / bottom"),
        k("Ctrl+N / Ctrl+P", "Next / previous tab (also Alt+arrows)"),
        k("Alt+1..9, Alt+0", "Jump to tab"),
        k("Alt+A", "Jump to next tab with unread messages"),
        k("Alt+W", "Close tab (leave channel)"),
        k("Alt+J / Alt+P", "Join a channel / open a private chat by name"),
        k("Ctrl+F", "Search this tab (Ctrl+F older, Ctrl+G newer, Esc ends)"),
        k("Ctrl+F in a window", "Find in any list or text (Ctrl+F next, Ctrl+G previous, Esc ends)"),
        k("Alt+N / Alt+M", "F-List notes / site messages"),
        k("Alt+U / Alt+G", "F-List new/updated characters / groups"),
        k("Alt+E", "Eicon picker (also Tab after [eicon])"),
        k("Ctrl+O", "Focus member list / tab list / input"),
        k("Ctrl+L", "Redraw screen"),
        k("Ctrl+C", "Clear input (quit when empty)"),
        #[cfg(feature = "gui")]
        k("Copy / paste", "Drag with the mouse to select and copy (or Ctrl+C); Ctrl+V or Shift+Ins pastes"),
        #[cfg(not(feature = "gui"))]
        k("Copy / paste", "With your terminal (with mouse support on: hold Shift to select)"),
        k("Ctrl+Q", "Quit"),
        k("F1", "This help"),
        k("F2", "Channel browser"),
        k("F3", "Friends & bookmarks"),
        k("F4", "Profile of the private chat partner"),
        k("F5", "Set status"),
        k("F6", "Settings"),
        k("F7", "Chat logs"),
        k("F8", "Character search"),
        k("F9 / F10", "Toggle member list / tab list"),
        k("F11", "Links in this tab"),
        k("Editing", "Ctrl+A/E start/end, Ctrl+W/U/K delete, Ctrl+Y paste, Alt+B/F words"),
        Line::default(),
        h("Commands"),
    ];
    for c in commands::COMMANDS {
        if matches!(c.perm, Perm::ChatOp | Perm::Admin) && !is_op {
            continue;
        }
        let tag = match c.perm {
            Perm::ChanOp => " [chan op]",
            Perm::ChanOwner => " [owner]",
            Perm::ChatOp => " [chat op]",
            Perm::Admin => " [admin]",
            Perm::User => "",
        };
        v.push(Line::from(vec![
            Span::styled(format!("{:<46} ", c.usage), Style::default().fg(Color::LightCyan)),
            Span::raw(c.help.to_string()),
            Span::styled(tag.to_string(), Style::default().fg(Color::DarkGray)),
        ]));
    }
    v.push(Line::default());
    v.push(h("Tips"));
    v.push(Line::from("Arguments with several parts are separated by commas, e.g. /timeout Some Name, 30"));
    v.push(Line::from("Start a message with // to send text beginning with a slash."));
    v.push(Line::from("BBCode works in messages: [b]bold[/b] [i]italic[/i] [color=red]...[/color] [eicon]name[/eicon]"));
    v
}

impl App {
    pub(super) fn draw_dialog(&mut self, buf: &mut Buffer, area: Rect, idx: usize, top: bool) {
        let ascii = self.settings.ascii_borders;
        let big = Rect::new(area.x + 2, area.y + 1, area.width.saturating_sub(4), area.height.saturating_sub(2));
        if let Dialog::Profile(_) = &self.dialogs[idx] {
            self.draw_profile(buf, area, idx);
            return;
        }
        let mut list_area = Rect::default();
        let mut list_offset = 0;
        let dialog_area;
        match &self.dialogs[idx] {
            Dialog::Help { scroll } => {
                dialog_area = big;
                let title = format!("F-Term v{} help · ↑↓/PgUp/PgDn scroll · Esc close", crate::protocol::CLIENT_VERSION);
                let inner = widgets::popup(buf, big, &title, ascii);
                let is_op = self.chat.is_chatop(&self.chat.me);
                let lines = widgets::wrap_all(&help_lines(is_op), inner.width as usize, 47);
                let s = (*scroll).min(lines.len().saturating_sub(inner.height as usize));
                widgets::note_text(&lines, s);
                for (i, l) in lines.iter().skip(s).take(inner.height as usize).enumerate() {
                    buf.set_line(inner.x, inner.y + i as u16, l, inner.width);
                }
            }
            Dialog::Text { title, lines, scroll } => {
                dialog_area = big;
                let inner = widgets::popup(buf, big, &format!("{title} · Esc close"), ascii);
                let lines = widgets::wrap_all(lines, inner.width as usize, 0);
                let s = (*scroll).min(lines.len().saturating_sub(inner.height as usize));
                widgets::note_text(&lines, s);
                for (i, l) in lines.iter().skip(s).take(inner.height as usize).enumerate() {
                    buf.set_line(inner.x, inner.y + i as u16, l, inner.width);
                }
            }
            Dialog::Menu { title, items, sel } => {
                let w = (items.iter().map(|i| widgets::str_width(&i.label)).max().unwrap_or(10).max(widgets::str_width(title)) as u16 + 6)
                    .min(area.width.saturating_sub(4))
                    .max(30);
                let h = (items.len() as u16 + 2).min(area.height.saturating_sub(2));
                dialog_area = widgets::centered(area, w, h);
                let inner = widgets::popup(buf, dialog_area, title, ascii);
                let lines: Vec<Line<'static>> = items.iter().map(|i| Line::from(i.label.clone())).collect();
                list_offset = widgets::list(buf, inner, &lines, *sel, top);
                list_area = inner;
            }
            Dialog::Confirm { text, .. } => {
                dialog_area = widgets::centered(area, (widgets::str_width(text) as u16 + 6).clamp(30, area.width), 5);
                let inner = widgets::popup(buf, dialog_area, "Confirm", ascii);
                buf.set_stringn(inner.x + 1, inner.y, text, inner.width as usize - 1, Style::default());
                widgets::draw_hints(buf, inner.x + 1, inner.y + 2, &[("y/Enter", "yes"), ("n/Esc", "no")], inner.width - 1);
            }
            Dialog::Prompt(p) => {
                let multi = p.editor.multiline;
                let h = if multi { (area.height * 2 / 3).max(8) } else { 5 };
                dialog_area = widgets::centered(area, 76.min(area.width), h);
                let inner = widgets::popup(buf, dialog_area, &p.title, ascii);
                let edit_area = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), inner.height.saturating_sub(2));
                let (lines, (cx, cy)) = p.editor.layout(edit_area.width);
                let off = (cy as usize + 1).saturating_sub(edit_area.height as usize);
                buf.set_style(edit_area, Style::default().bg(Color::Rgb(30, 30, 30)));
                for (i, l) in lines.iter().skip(off).take(edit_area.height as usize).enumerate() {
                    buf.set_string(edit_area.x, edit_area.y + i as u16, l, Style::default().bg(Color::Rgb(30, 30, 30)));
                }
                let hints: &[(&str, &str)] = if multi {
                    &[("Ctrl+S", "save"), ("Enter", "new line"), ("Esc", "cancel")]
                } else {
                    &[("Enter", "OK"), ("Esc", "cancel")]
                };
                widgets::draw_hints(buf, inner.x + 1, inner.bottom() - 1, hints, inner.width - 1);
                if top {
                    let cell = (edit_area.x + cx, edit_area.y + (cy as usize - off) as u16);
                    buf.set_style(Rect::new(cell.0, cell.1, 1, 1), Style::default().add_modifier(Modifier::REVERSED));
                }
            }
            Dialog::Image { title, url } => {
                dialog_area = big;
                let inner = widgets::popup(buf, big, &format!("{title} · o open in browser · Esc close"), ascii);
                let (url, mode, proto, tc) =
                    (url.clone(), self.settings.active_image_mode(), self.settings.graphics_protocol, self.settings.truecolor);
                let view = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
                self.images.draw(buf, view, &url, mode, proto, tc);
                buf.set_stringn(inner.x, inner.bottom() - 1, &url, inner.width as usize, Style::default().fg(Color::DarkGray));
            }
            Dialog::Channels(c) => {
                dialog_area = big;
                let inner = widgets::popup(buf, big, "Channels", ascii);
                let tabs = ["Public channels", "Private rooms"];
                let mut x = inner.x;
                for (i, t) in tabs.iter().enumerate() {
                    let n = if i == 0 { self.chat.public_channels.len() } else { self.chat.private_rooms.len() };
                    let label = format!(" {t} ({n}) ");
                    let st = if i == c.tab { Style::default().fg(Color::Black).bg(Color::Cyan) } else { Style::default().fg(Color::Gray) };
                    buf.set_string(x, inner.y, &label, st);
                    x += label.chars().count() as u16 + 1;
                }
                buf.set_string(inner.x, inner.y + 1, "Filter: ", Style::default().fg(Color::Gray));
                buf.set_string(inner.x + 8, inner.y + 1, c.filter.text(), Style::default().fg(Color::Yellow));
                let list = self.channel_list(c);
                let items: Vec<Line<'static>> = list
                    .iter()
                    .map(|(id, title, users, mode)| {
                        let joined = self.chat.find_channel(id).map(|i| self.chat.conversations[i].joined).unwrap_or(false);
                        Line::from(vec![
                            Span::styled(if joined { "✓ " } else { "  " }, Style::default().fg(Color::LightGreen)),
                            Span::styled(format!("{:>5} ", users), Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                trunc(&bbcode::strip(title), inner.width.saturating_sub(20) as usize),
                                if joined { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() },
                            ),
                            Span::styled(
                                if mode.is_empty() || mode == "both" { String::new() } else { format!("  [{mode}]") },
                                Style::default().fg(Color::DarkGray),
                            ),
                        ])
                    })
                    .collect();
                list_area = Rect::new(inner.x, inner.y + 3, inner.width, inner.height.saturating_sub(4));
                list_offset = widgets::list(buf, list_area, &items, c.sel, top);
                if items.is_empty() {
                    buf.set_string(list_area.x + 1, list_area.y, "Loading… (or nothing matches)", Style::default().fg(Color::DarkGray));
                }
                let sort = if c.sort_by_name { "sort: name" } else { "sort: users" };
                widgets::draw_hints(
                    buf,
                    inner.x,
                    inner.bottom() - 1,
                    &[
                        ("Enter", "join/leave"),
                        ("Tab", "public/private"),
                        ("Ctrl+S", sort),
                        ("Ctrl+N", "new room"),
                        ("Ctrl+R", "refresh"),
                        ("Esc", "close"),
                    ],
                    inner.width,
                );
            }
            Dialog::Friends(fs) => {
                dialog_area = big;
                let inner = widgets::popup(buf, big, "Friends & bookmarks", ascii);
                let rows = self.friend_rows(fs);
                let (tab, sel) = (fs.tab, selectable(&rows, fs.sel, true));
                let state: Option<Result<(), String>> = match &fs.data {
                    None => None,
                    Some(Err(e)) => Some(Err(e.clone())),
                    Some(Ok(_)) => Some(Ok(())),
                };
                let tabs = ["Friends", "Bookmarks", "Incoming requests", "Sent requests"];
                let mut x = inner.x;
                for (i, t) in tabs.iter().enumerate() {
                    let label = format!(" {t} ");
                    let st = if i == tab { Style::default().fg(Color::Black).bg(Color::Cyan) } else { Style::default().fg(Color::Gray) };
                    buf.set_string(x, inner.y, &label, st);
                    x += label.chars().count() as u16 + 1;
                }
                widgets::draw_hints(
                    buf,
                    inner.x,
                    inner.bottom() - 1,
                    &[("Tab", "switch list"), ("Enter", "actions"), ("Ctrl+F", "find"), ("Ctrl+R", "refresh"), ("Esc", "close")],
                    inner.width,
                );
                let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(3));
                // Details of the selected character at the right, if there is room.
                let card = body.width >= 70;
                let left_w = if card { (body.width * 2 / 5).clamp(30, 50) } else { body.width };
                list_area = Rect::new(body.x, body.y, left_w, body.height);
                match state {
                    None => buf.set_string(list_area.x, list_area.y, "Loading…", Style::default().fg(Color::DarkGray)),
                    Some(Err(e)) => {
                        buf.set_stringn(
                            list_area.x,
                            list_area.y,
                            format!("Error: {e}"),
                            body.width as usize,
                            Style::default().fg(Color::LightRed),
                        );
                    }
                    Some(Ok(())) => {
                        let items: Vec<Line<'static>> = rows.iter().map(|r| r.line.clone()).collect();
                        list_offset = widgets::list(buf, list_area, &items, sel, top);
                        if items.is_empty() {
                            buf.set_string(list_area.x + 1, list_area.y, "Nothing here.", Style::default().fg(Color::DarkGray));
                        } else if card {
                            let r = &rows[sel.min(rows.len() - 1)];
                            let right = Rect::new(body.x + left_w + 2, body.y, body.width.saturating_sub(left_w + 2), body.height);
                            self.draw_friend_card(buf, right, &r.name, &r.note);
                        }
                    }
                }
            }
            Dialog::Settings(st) => {
                dialog_area = big;
                let inner = widgets::popup(buf, big, "Settings", ascii);
                let kw = SETTINGS.iter().map(|s| s.key.len()).max().unwrap_or(10);
                let items: Vec<Line<'static>> = SETTINGS
                    .iter()
                    .map(|s| {
                        let v = self.settings.get(s.key).unwrap_or_default();
                        let vs = match v.as_str() {
                            "true" => Span::styled("on", Style::default().fg(Color::LightGreen)),
                            "false" => Span::styled("off", Style::default().fg(Color::DarkGray)),
                            _ => Span::styled(trunc(&v, 40), Style::default().fg(Color::Yellow)),
                        };
                        Line::from(vec![Span::raw(format!("{:<kw$}  ", s.key)), vs])
                    })
                    .collect();
                list_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(3));
                list_offset = widgets::list(buf, list_area, &items, st.sel, top);
                let help = SETTINGS[st.sel.min(SETTINGS.len() - 1)].help;
                buf.set_stringn(inner.x + 1, inner.bottom() - 2, help, inner.width as usize - 1, Style::default().fg(Color::Gray));
                widgets::draw_hints(
                    buf,
                    inner.x,
                    inner.bottom() - 1,
                    &[("Enter", "toggle/edit"), ("←→", "cycle"), ("Esc", "close")],
                    inner.width,
                );
            }
            Dialog::Status(st) => {
                dialog_area = widgets::centered(area, 70, (14 + st.recent.len().min(8)) as u16);
                let inner = widgets::popup(buf, dialog_area, "Set status", ascii);
                let focus_st = |on: bool| if on && top { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::Gray) };
                buf.set_string(inner.x + 1, inner.y, "Status:", focus_st(st.focus == 0));
                let mut x = inner.x + 10;
                for s in Status::SETTABLE {
                    let label = format!(" {} ", s.label());
                    let style =
                        if s == st.status { Style::default().fg(Color::Black).bg(s.color()) } else { Style::default().fg(s.color()) };
                    buf.set_string(x, inner.y, &label, style);
                    x += label.chars().count() as u16;
                }
                buf.set_string(inner.x + 1, inner.y + 2, "Message:", focus_st(st.focus == 1));
                let ew = inner.width.saturating_sub(4);
                let (lines, (cx, cy)) = st.msg.layout(ew);
                let ea = Rect::new(inner.x + 2, inner.y + 3, ew, 3);
                buf.set_style(ea, Style::default().bg(Color::Rgb(30, 30, 30)));
                let off = (cy as usize + 1).saturating_sub(3);
                for (i, l) in lines.iter().skip(off).take(3).enumerate() {
                    buf.set_string(ea.x, ea.y + i as u16, l, Style::default().bg(Color::Rgb(30, 30, 30)));
                }
                if st.focus == 1 && top {
                    buf.set_style(
                        Rect::new(ea.x + cx, ea.y + (cy as usize - off) as u16, 1, 1),
                        Style::default().add_modifier(Modifier::REVERSED),
                    );
                }
                buf.set_string(inner.x + 1, inner.y + 7, "Recent messages:", focus_st(st.focus == 2));
                let items: Vec<Line<'static>> =
                    st.recent.iter().map(|r| Line::from(trunc(&bbcode::strip(r), inner.width as usize - 4))).collect();
                list_area = Rect::new(inner.x + 1, inner.y + 8, inner.width - 2, inner.height.saturating_sub(10));
                widgets::list(buf, list_area, &items, st.recent_sel, st.focus == 2 && top);
                widgets::draw_hints(
                    buf,
                    inner.x,
                    inner.bottom() - 1,
                    &[("Tab", "next field"), ("←→", "status"), ("Enter", "apply"), ("Esc", "cancel")],
                    inner.width,
                );
            }
            Dialog::Logs(l) => {
                dialog_area = big;
                let title = match l.stage {
                    0 => "Logs".to_string(),
                    1 => format!("Logs: {}", l.conv_title),
                    3 => format!(
                        "Log search: {} result{}{}",
                        l.hits.len(),
                        if l.hits.len() == 1 { "" } else { "s" },
                        if l.hits.len() >= LOG_SEARCH_LIMIT { " (limit reached)" } else { "" }
                    ),
                    _ => format!("Logs: {} — {}", l.conv_title, l.day),
                };
                let inner = widgets::popup(buf, big, &title, ascii);
                buf.set_string(inner.x, inner.y, if l.stage >= 2 { "Search: " } else { "Filter: " }, Style::default().fg(Color::Gray));
                let shown = if l.stage == 3 { l.query.as_str() } else { l.filter.text() };
                buf.set_string(inner.x + 8, inner.y, shown, Style::default().fg(Color::Yellow));
                let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(3));
                match l.stage {
                    0 => {
                        let items: Vec<Line<'static>> = l.filtered_convs().into_iter().map(|(_, t)| Line::from(t)).collect();
                        list_offset = widgets::list(buf, body, &items, l.sel, top);
                        list_area = body;
                        if items.is_empty() {
                            buf.set_string(body.x + 1, body.y, "No logs yet.", Style::default().fg(Color::DarkGray));
                        }
                    }
                    1 => {
                        let items: Vec<Line<'static>> = l.days.iter().map(|d| Line::from(d.clone())).collect();
                        list_offset = widgets::list(buf, body, &items, l.day_sel, top);
                        list_area = body;
                    }
                    3 => {
                        let items: Vec<Line<'static>> = l
                            .hits
                            .iter()
                            .map(|h| {
                                let text = match logs::parse_line(&h.line) {
                                    Some(m) if !m.sender.is_empty() => format!("{}: {}", m.sender, bbcode::strip(&m.text)),
                                    Some(m) => bbcode::strip(&m.text),
                                    None => h.line.clone(),
                                };
                                Line::from(vec![
                                    Span::styled(format!("{} ", h.day), Style::default().fg(Color::DarkGray)),
                                    Span::styled(format!("{}  ", h.title), Style::default().fg(Color::LightCyan)),
                                    Span::raw(text.replace('\n', " ")),
                                ])
                            })
                            .collect();
                        list_offset = widgets::list(buf, body, &items, l.hit_sel, top);
                        list_area = body;
                        if items.is_empty() {
                            buf.set_string(body.x + 1, body.y, "Nothing found.", Style::default().fg(Color::DarkGray));
                        }
                    }
                    _ => {
                        let ns = |n: &str| self.name_style(n);
                        let lines: Vec<Line<'static>> = logs::merge_continuations(l.filtered_lines())
                            .iter()
                            .flat_map(|raw| {
                                let ls = match logs::parse_line(raw) {
                                    Some(m) => {
                                        let ts = crate::timeutil::fmt_time(m.ts, true);
                                        let head = match m.kind {
                                            crate::state::MsgKind::Ad => format!("[{ts}] [AD] [user]{}[/user]: ", m.sender),
                                            crate::state::MsgKind::Chat => format!("[{ts}] [user]{}[/user]: ", m.sender),
                                            crate::state::MsgKind::Action => format!("[{ts}] * "),
                                            _ => format!("[{ts}] -!- "),
                                        };
                                        bbcode::to_lines(&format!("{head}{}", m.text), Style::default(), &ns, true)
                                    }
                                    None => vec![Line::from(raw.clone())],
                                };
                                widgets::wrap_all(&ls, body.width as usize, 2)
                            })
                            .collect();
                        let s = l.scroll.min(lines.len().saturating_sub(body.height as usize));
                        widgets::note_text(&lines, s);
                        for (i, line) in lines.iter().skip(s).take(body.height as usize).enumerate() {
                            buf.set_line(body.x, body.y + i as u16, line, body.width);
                        }
                    }
                }
                let hints: &[(&str, &str)] = match l.stage {
                    0 => &[("Enter", "open"), ("type", "filter"), ("Ctrl+F", "search all logs"), ("Esc", "close")],
                    1 => &[("Enter", "read"), ("Esc", "back")],
                    3 => &[("Enter", "open day"), ("Esc", "close")],
                    _ => &[("↑↓ PgUp PgDn", "scroll"), ("type", "search"), ("[ ]", "prev/next day"), ("Esc", "back")],
                };
                widgets::draw_hints(buf, inner.x, inner.bottom() - 1, hints, inner.width);
            }
            Dialog::Search(s) => {
                dialog_area = big;
                let inner = widgets::popup(buf, big, "Character search", ascii);
                if s.cats.is_empty() {
                    buf.set_stringn(
                        inner.x,
                        inner.y,
                        if s.status.is_empty() { "Loading search fields…" } else { &s.status },
                        inner.width as usize,
                        Style::default().fg(Color::DarkGray),
                    );
                } else {
                    let mut x = inner.x;
                    for (i, (_, label, _)) in s.cats.iter().enumerate() {
                        let n = s.selected.get(i).map(|x| x.len()).unwrap_or(0);
                        let t = if n > 0 { format!(" {label} ({n}) ") } else { format!(" {label} ") };
                        let st =
                            if i == s.cat { Style::default().fg(Color::Black).bg(Color::Cyan) } else { Style::default().fg(Color::Gray) };
                        if x + t.len() as u16 > inner.right() {
                            break;
                        }
                        buf.set_string(x, inner.y, &t, st);
                        x += t.chars().count() as u16 + 1;
                    }
                    let half = inner.width / 2;
                    buf.set_string(inner.x, inner.y + 1, "Filter: ", Style::default().fg(Color::Gray));
                    buf.set_string(inner.x + 8, inner.y + 1, s.filter.text(), Style::default().fg(Color::Yellow));
                    let left = Rect::new(inner.x, inner.y + 3, half - 1, inner.height.saturating_sub(5));
                    let sel_set = s.selected.get(s.cat).cloned().unwrap_or_default();
                    let items: Vec<Line<'static>> = s
                        .visible()
                        .into_iter()
                        .map(|(id, l)| {
                            Line::from(vec![
                                Span::styled(if sel_set.contains(&id) { "[x] " } else { "[ ] " }, Style::default().fg(Color::LightGreen)),
                                Span::raw(l),
                            ])
                        })
                        .collect();
                    let off = widgets::list(buf, left, &items, s.sel, top && s.focus == SearchFocus::Options);
                    let right = Rect::new(inner.x + half + 1, inner.y + 3, inner.width - half - 1, inner.height.saturating_sub(5));
                    buf.set_string(right.x, inner.y + 2, format!("Results ({})", s.results.len()), Style::default().fg(Color::Cyan));
                    let res: Vec<Line<'static>> = s
                        .results
                        .iter()
                        .map(|n| {
                            let st = self.chat.status_of(n);
                            Line::from(vec![
                                Span::styled(format!("{} ", st.symbol()), Style::default().fg(st.color())),
                                Span::styled(n.clone(), self.name_style(n)),
                            ])
                        })
                        .collect();
                    let roff = widgets::list(buf, right, &res, s.result_sel, top && s.focus == SearchFocus::Results);
                    if s.focus == SearchFocus::Results {
                        list_area = right;
                        list_offset = roff;
                    } else {
                        list_area = left;
                        list_offset = off;
                    }
                    buf.set_stringn(inner.x, inner.bottom() - 2, &s.status, inner.width as usize, Style::default().fg(Color::Gray));
                }
                widgets::draw_hints(
                    buf,
                    inner.x,
                    inner.bottom() - 1,
                    &[
                        ("Tab", "category"),
                        ("Enter", "toggle/open"),
                        ("Ctrl+S", "search"),
                        ("Ctrl+R", "results/options"),
                        ("Ctrl+X", "reset"),
                        ("Esc", "close"),
                    ],
                    inner.width,
                );
            }
            Dialog::Eicons(_) => {
                let (d, l, o) = self.draw_eicon_picker(buf, big, idx);
                dialog_area = d;
                list_area = l;
                list_offset = o;
            }
            Dialog::Browse(_) | Dialog::Groups(_) => {
                let (d, l, o) = if matches!(self.dialogs[idx], Dialog::Browse(_)) {
                    self.draw_browse(buf, big, idx, top)
                } else {
                    self.draw_groups(buf, big, idx, top)
                };
                dialog_area = d;
                list_area = l;
                list_offset = o;
            }
            Dialog::Notes(_) | Dialog::Messages(_) | Dialog::Personals(_) => {
                let (d, l, o) = self.draw_site_dialog(buf, big, idx, top);
                dialog_area = d;
                list_area = l;
                list_offset = o;
            }
            Dialog::Profile(_) => unreachable!(),
        }
        if top {
            self.hit.dialog = dialog_area;
            self.hit.dialog_list = list_area;
            self.hit.dialog_list_offset = list_offset;
        }
    }

    /// (id, title, users, mode) of the channel list matching the filter.
    fn channel_list(&self, c: &ChannelsState) -> Vec<(String, String, i64, String)> {
        let src = if c.tab == 0 { &self.chat.public_channels } else { &self.chat.private_rooms };
        let f = c.filter.text().to_lowercase();
        let mut v: Vec<(String, String, i64, String)> = src
            .iter()
            .filter(|ch| ch.title.to_lowercase().contains(&f) || ch.name.to_lowercase().contains(&f))
            .map(|ch| (ch.name.clone(), ch.title.clone(), ch.characters, ch.mode.clone()))
            .collect();
        if c.sort_by_name {
            v.sort_by_key(|a| a.1.to_lowercase());
        } else {
            v.sort_by_key(|a| (-a.2, a.1.to_lowercase()));
        }
        v
    }

    /// Rows for the friends dialog. Your characters head their friends.
    fn friend_rows(&self, fs: &FriendsState) -> Vec<FriendRow> {
        let Some(Ok(d)) = &fs.data else { return Vec::new() };
        let person = |name: &str, extra: String| -> Line<'static> {
            let st = self.chat.status_of(name);
            let msg = self.chat.character(name).map(|c| bbcode::strip(&c.status_msg)).unwrap_or_default();
            Line::from(vec![
                Span::styled(format!("{} ", st.symbol()), Style::default().fg(st.color())),
                Span::styled(name.to_string(), self.name_style(name)),
                Span::styled(extra, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if msg.is_empty() { String::new() } else { format!("  {}", trunc(&msg, 60)) },
                    Style::default().fg(Color::Gray),
                ),
            ])
        };
        let row = |action: Action, line: Line<'static>, name: &str, note: String| FriendRow {
            action: Some(action),
            line,
            name: name.to_string(),
            note,
        };
        if fs.tab == 0 {
            // One section per own character, the current one first.
            let me = self.chat.me.to_lowercase();
            let mut mine: Vec<String> = Vec::new();
            for (_, m, _) in &d.friends {
                if !mine.iter().any(|x| x.eq_ignore_ascii_case(m)) {
                    mine.push(m.clone());
                }
            }
            mine.sort_by_key(|m| (m.to_lowercase() != me, m.to_lowercase()));
            let mut rows = Vec::new();
            for m in mine {
                let mut list: Vec<&String> = d.friends.iter().filter(|f| f.1.eq_ignore_ascii_case(&m)).map(|f| &f.0).collect();
                list.sort_by_key(|t| (!self.chat.is_online(t), t.to_lowercase()));
                let online = list.iter().filter(|t| self.chat.is_online(t)).count();
                let count = format!("{} friend{}, {online} online", list.len(), if list.len() == 1 { "" } else { "s" });
                let head = Line::from(vec![
                    Span::styled(m.clone(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" · {count}"), Style::default().fg(Color::DarkGray)),
                ]);
                rows.push(row(Action::Profile(m.clone()), head, &m, format!("Your character · {count}")));
                for t in list {
                    let mut line = person(t, String::new());
                    line.spans.insert(0, Span::raw("  "));
                    rows.push(row(Action::FriendOf(t.clone(), m.clone()), line, t, format!("♥ Friend of {m}")));
                }
            }
            return rows;
        }
        let mut rows: Vec<(bool, String, FriendRow)> = match fs.tab {
            1 => d
                .bookmarks
                .iter()
                .map(|b| {
                    let note = if self.chat.is_any_friend(b) { "★ Bookmarked · ♥ friend" } else { "★ Bookmarked" };
                    (self.chat.is_online(b), b.to_lowercase(), row(Action::Profile(b.clone()), person(b, String::new()), b, note.into()))
                })
                .collect(),
            2 => d
                .incoming
                .iter()
                .map(|r| {
                    let a = Action::FriendRequestAction("incoming".into(), r.id.clone());
                    let line = person(&r.source, format!("  → {}", r.dest));
                    (true, r.source.to_lowercase(), row(a, line, &r.source, format!("Wants to be friends with {}", r.dest)))
                })
                .collect(),
            _ => d
                .pending
                .iter()
                .map(|r| {
                    let a = Action::FriendRequestAction("cancel".into(), r.id.clone());
                    let line = person(&r.dest, format!("  (from {})", r.source));
                    (true, r.dest.to_lowercase(), row(a, line, &r.dest, format!("Your request from {}", r.source)))
                })
                .collect(),
        };
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        rows.into_iter().map(|r| r.2).collect()
    }

    /// Details of a character in the friends window.
    fn draw_friend_card(&mut self, buf: &mut Buffer, area: Rect, name: &str, note: &str) {
        let st = self.chat.status_of(name);
        let msg = self.chat.character(name).map(|c| bbcode::strip(&c.status_msg)).unwrap_or_default();
        let mut extra = vec![
            Line::from(Span::styled(note.to_string(), Style::default().fg(Color::Yellow))),
            Line::from(Span::styled(format!("{} {}", st.symbol(), st.label()), Style::default().fg(st.color()))),
        ];
        if !msg.trim().is_empty() {
            extra.push(Line::from(Span::styled(msg.trim().to_string(), Style::default().fg(Color::Gray))));
        }
        let card = self.char_card(name);
        let map = self.mapping.clone();
        let info: Vec<(String, String)> = match (&card, &map) {
            (Some(Ok(v)), Some(map)) => super::profile::info_groups(v, map).into_iter().flat_map(|(_, rows)| rows).collect(),
            (Some(Err(e)), _) => vec![("Error".into(), e.clone())],
            _ => vec![("".into(), "Loading…".into())],
        };
        let gender = info.iter().find(|i| i.0 == "Gender").map(|i| i.1.clone()).unwrap_or_else(|| self.chat.gender_of(name));
        self.draw_character_card(buf, area, name, &gender, &info, extra);
    }

    pub(super) fn dialog_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(self.dialogs.last(), Some(Dialog::Profile(_))) {
            return self.profile_key(key);
        }
        let Some(d) = self.dialogs.last_mut() else { return };
        let mut close = false;
        let mut action: Option<Action> = None;
        match d {
            Dialog::Help { scroll } | Dialog::Text { scroll, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter | KeyCode::F(1) => close = true,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(15),
                KeyCode::PageDown | KeyCode::Char(' ') => *scroll += 15,
                KeyCode::Home => *scroll = 0,
                _ => {}
            },
            Dialog::Menu { items, sel, .. } => match key.code {
                KeyCode::Esc => close = true,
                KeyCode::Up | KeyCode::Char('k') => *sel = (*sel + items.len() - 1) % items.len().max(1),
                KeyCode::Down | KeyCode::Char('j') => *sel = (*sel + 1) % items.len().max(1),
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if let Some(i) = items.get(*sel) {
                        action = Some(i.action.clone());
                    }
                    close = true;
                }
                KeyCode::Delete => {
                    if let Some(alt) = items.get(*sel).and_then(|i| i.alt.clone()) {
                        items.remove(*sel);
                        *sel = (*sel).min(items.len().saturating_sub(1));
                        action = Some(alt);
                        if items.is_empty() {
                            close = true;
                        }
                    }
                }
                _ => {}
            },
            Dialog::Confirm { action: a, .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    action = Some(a.clone());
                    close = true;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => close = true,
                _ => {}
            },
            Dialog::Prompt(p) => {
                if key.code == KeyCode::Esc {
                    close = true;
                } else if ctrl && key.code == KeyCode::Char('s') {
                    let (kind, v, e) = (p.kind, p.editor.text().to_string(), p.extra.clone());
                    self.dialogs.pop();
                    self.on_prompt(kind, v, e);
                    return;
                } else if let EditResult::Submit = p.editor.handle(key) {
                    let (kind, v, e) = (p.kind, p.editor.text().to_string(), p.extra.clone());
                    self.dialogs.pop();
                    self.on_prompt(kind, v, e);
                    return;
                }
            }
            Dialog::Image { url, .. } => match key.code {
                KeyCode::Char('o') => {
                    action = Some(Action::OpenUrl(url.clone()));
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => close = true,
                _ => {}
            },
            Dialog::Channels(_) => return self.channels_key(key),
            Dialog::Friends(_) => return self.friends_key(key),
            Dialog::Settings(_) => return self.settings_key(key),
            Dialog::Status(_) => return self.status_key(key),
            Dialog::Logs(_) => return self.logs_key(key),
            Dialog::Search(_) => return self.search_key(key),
            Dialog::Notes(_) => return self.notes_key(key),
            Dialog::Messages(_) => return self.messages_key(key),
            Dialog::Personals(_) => return self.personals_key(key),
            Dialog::Eicons(_) => return self.eicon_picker_key(key),
            Dialog::Browse(_) => return self.browse_key(key),
            Dialog::Groups(_) => return self.groups_key(key),
            Dialog::Profile(_) => unreachable!(),
        }
        if close {
            self.dialogs.pop();
            self.full_redraw = true;
        }
        if let Some(a) = action {
            self.run_action(a);
        }
    }

    fn channels_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Dialog::Channels(c)) = self.dialogs.last() else { return };
        let list = self.channel_list(c);
        let Some(Dialog::Channels(c)) = self.dialogs.last_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                self.dialogs.pop();
            }
            KeyCode::Tab | KeyCode::BackTab => {
                c.tab = 1 - c.tab;
                c.sel = 0;
            }
            KeyCode::Up => c.sel = c.sel.saturating_sub(1),
            KeyCode::Down => c.sel = (c.sel + 1).min(list.len().saturating_sub(1)),
            KeyCode::PageUp => c.sel = c.sel.saturating_sub(15),
            KeyCode::PageDown => c.sel = (c.sel + 15).min(list.len().saturating_sub(1)),
            KeyCode::Char('s') if ctrl => c.sort_by_name = !c.sort_by_name,
            KeyCode::Char('r') if ctrl => {
                self.send(protocol::cha());
                self.send(protocol::ors());
            }
            KeyCode::Char('n') if ctrl => {
                let title = c.filter.text().trim().to_string();
                if title.is_empty() {
                    self.prompt("Title of the new private room", "", PromptKind::Command, false);
                    if let Some(Dialog::Prompt(p)) = self.dialogs.last_mut() {
                        p.editor.set("/makeroom ");
                    }
                } else {
                    self.send(protocol::chan_only("CCR", &title));
                    self.dialogs.pop();
                }
            }
            KeyCode::Enter => {
                if let Some((id, _, _, _)) = list.get(c.sel.min(list.len().saturating_sub(1))) {
                    let id = id.clone();
                    match self.chat.find_channel(&id) {
                        Some(i) if self.chat.conversations[i].joined => self.close_conversation(i),
                        _ => self.send(protocol::jch(&id)),
                    }
                }
            }
            _ => {
                if let EditResult::Changed = c.filter.handle(key) {
                    c.sel = 0;
                }
            }
        }
    }

    fn friends_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Dialog::Friends(fs)) = self.dialogs.last() else { return };
        let rows = self.friend_rows(fs);
        let Some(Dialog::Friends(fs)) = self.dialogs.last_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                self.dialogs.pop();
            }
            KeyCode::Tab | KeyCode::Right => {
                fs.tab = (fs.tab + 1) % 4;
                fs.sel = 0;
            }
            KeyCode::BackTab | KeyCode::Left => {
                fs.tab = (fs.tab + 3) % 4;
                fs.sel = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let cur = selectable(&rows, fs.sel, true);
                fs.sel = selectable(&rows, cur.saturating_sub(1), false);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let cur = selectable(&rows, fs.sel, true);
                fs.sel = selectable(&rows, (cur + 1).min(rows.len().saturating_sub(1)), true);
            }
            KeyCode::Char('r') if ctrl => {
                fs.data = None;
                self.api_call(|api| ApiReply::FriendLists(api.friend_lists()));
            }
            KeyCode::Enter => {
                let sel = selectable(&rows, fs.sel, true);
                let Some(FriendRow { action: Some(action), .. }) = rows.into_iter().nth(sel) else { return };
                match action {
                    Action::Profile(name) => self.open_user_menu(&name),
                    Action::FriendOf(theirs, mine) => self.open_friend_menu(&theirs, &mine),
                    Action::FriendRequestAction(kind, id) if kind == "incoming" => {
                        let items = vec![
                            MenuItem::new("Accept", Action::FriendRequestAction("accept".into(), id.clone())),
                            MenuItem::new("Deny", Action::FriendRequestAction("deny".into(), id)),
                        ];
                        self.dialogs.push(Dialog::menu("Friend request", items));
                    }
                    Action::FriendRequestAction(_, id) => {
                        self.dialogs.push(Dialog::confirm("Cancel this friend request?", Action::FriendRequestAction("cancel".into(), id)));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn settings_key(&mut self, key: KeyEvent) {
        let Some(Dialog::Settings(st)) = self.dialogs.last_mut() else { return };
        let n = SETTINGS.len();
        let key_name = SETTINGS[st.sel.min(n - 1)].key;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.dialogs.pop();
            }
            KeyCode::Up | KeyCode::Char('k') => st.sel = (st.sel + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => st.sel = (st.sel + 1) % n,
            KeyCode::PageUp => st.sel = st.sel.saturating_sub(10),
            KeyCode::PageDown => st.sel = (st.sel + 10).min(n - 1),
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                let back = key.code == KeyCode::Left;
                if self.settings.is_bool(key_name) {
                    let _ = self.settings.set(key_name, "toggle");
                } else if key_name == "image_mode" || key_name == "gui_image_mode" {
                    let all = ImageMode::ALL;
                    let field = if key_name == "image_mode" { &mut self.settings.image_mode } else { &mut self.settings.gui_image_mode };
                    let i = all.iter().position(|m| *m == *field).unwrap_or(0);
                    *field = all[if back { (i + all.len() - 1) % all.len() } else { (i + 1) % all.len() }];
                } else if key_name == "notify_online" {
                    let all = crate::config::WatchNotify::ALL;
                    let i = all.iter().position(|m| *m == self.settings.notify_online).unwrap_or(0);
                    self.settings.notify_online = all[if back { (i + all.len() - 1) % all.len() } else { (i + 1) % all.len() }];
                } else if key_name == "graphics_protocol" {
                    let all = GraphicsProtocol::ALL;
                    let i = all.iter().position(|m| *m == self.settings.graphics_protocol).unwrap_or(0);
                    self.settings.graphics_protocol = all[if back { (i + all.len() - 1) % all.len() } else { (i + 1) % all.len() }];
                } else if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
                    let cur = self.settings.get(key_name).unwrap_or_default();
                    let mut d = Dialog::prompt(&format!("{key_name}: {}", SETTINGS[st.sel].help), &cur, PromptKind::SaveSetting, false);
                    if let Dialog::Prompt(p) = &mut d {
                        p.extra = key_name.to_string();
                    }
                    self.dialogs.push(d);
                    return;
                } else {
                    return;
                }
                self.save_settings();
                self.settings_changed();
            }
            _ => {}
        }
    }

    fn status_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Dialog::Status(st)) = self.dialogs.last_mut() else { return };
        let apply = |app: &mut App| {
            if let Some(Dialog::Status(st)) = app.dialogs.pop() {
                let msg = st.msg.text().trim().to_string();
                app.set_status(st.status, &msg, true);
                app.flash(&format!("Status: {}", st.status.label()));
            }
        };
        match key.code {
            KeyCode::Esc => {
                self.dialogs.pop();
            }
            KeyCode::Tab => st.focus = (st.focus + 1) % 3,
            KeyCode::BackTab => st.focus = (st.focus + 2) % 3,
            KeyCode::Char('s') if ctrl => apply(self),
            _ => match st.focus {
                0 => {
                    let all = Status::SETTABLE;
                    let i = all.iter().position(|s| *s == st.status).unwrap_or(0);
                    match key.code {
                        KeyCode::Left | KeyCode::Up => st.status = all[(i + all.len() - 1) % all.len()],
                        KeyCode::Right | KeyCode::Down => st.status = all[(i + 1) % all.len()],
                        KeyCode::Enter => apply(self),
                        _ => {}
                    }
                }
                1 => match key.code {
                    KeyCode::Up if !st.msg.text().contains('\n') => {
                        let all = Status::SETTABLE;
                        let i = all.iter().position(|s| *s == st.status).unwrap_or(0);
                        st.status = all[(i + all.len() - 1) % all.len()];
                    }
                    _ => {
                        if let EditResult::Submit = st.msg.handle(key) {
                            apply(self);
                        }
                    }
                },
                _ => match key.code {
                    KeyCode::Up => st.recent_sel = st.recent_sel.saturating_sub(1),
                    KeyCode::Down => st.recent_sel = (st.recent_sel + 1).min(st.recent.len().saturating_sub(1)),
                    KeyCode::Enter => {
                        if let Some(r) = st.recent.get(st.recent_sel).cloned() {
                            st.msg.set(&r);
                            st.focus = 1;
                        }
                    }
                    _ => {}
                },
            },
        }
    }

    fn logs_key(&mut self, key: KeyEvent) {
        let me = self.chat.me.clone();
        let Some(Dialog::Logs(l)) = self.dialogs.last_mut() else { return };
        match (l.stage, key.code) {
            (0, KeyCode::Esc) => {
                self.dialogs.pop();
            }
            (0, KeyCode::Char('f')) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let q = l.filter.text().to_string();
                if !q.trim().is_empty() {
                    l.search(&me, &q);
                }
            }
            (3, KeyCode::Esc) => {
                self.dialogs.pop();
            }
            (3, KeyCode::Up) => l.hit_sel = l.hit_sel.saturating_sub(1),
            (3, KeyCode::Down) => l.hit_sel = (l.hit_sel + 1).min(l.hits.len().saturating_sub(1)),
            (3, KeyCode::PageUp) => l.hit_sel = l.hit_sel.saturating_sub(15),
            (3, KeyCode::PageDown) => l.hit_sel = (l.hit_sel + 15).min(l.hits.len().saturating_sub(1)),
            (3, KeyCode::Enter) => {
                if let Some(h) = l.hits.get(l.hit_sel).cloned() {
                    if let Some(i) = l.convs.iter().position(|c| c.0 == h.conv_key) {
                        l.open_conv(&me, i);
                        l.day_sel = l.days.iter().position(|d| *d == h.day).unwrap_or(0);
                        l.lines = logs::read_day(&me, &l.conv_key, &h.day);
                        l.day = h.day;
                        l.stage = 2;
                        l.scroll = 0;
                        l.from_search = true;
                        let q = l.query.clone();
                        l.filter.insert_str(&q);
                    }
                }
            }
            (0, KeyCode::Up) => l.sel = l.sel.saturating_sub(1),
            (0, KeyCode::Down) => l.sel = (l.sel + 1).min(l.filtered_convs().len().saturating_sub(1)),
            (0, KeyCode::PageUp) => l.sel = l.sel.saturating_sub(15),
            (0, KeyCode::PageDown) => l.sel = (l.sel + 15).min(l.filtered_convs().len().saturating_sub(1)),
            (0, KeyCode::Enter) => {
                let f = l.filtered_convs();
                if let Some((k, _)) = f.get(l.sel) {
                    if let Some(i) = l.convs.iter().position(|c| &c.0 == k) {
                        l.open_conv(&me, i);
                    }
                }
            }
            (1, KeyCode::Esc) => {
                l.stage = 0;
                l.filter.clear();
            }
            (1, KeyCode::Up) => l.day_sel = l.day_sel.saturating_sub(1),
            (1, KeyCode::Down) => l.day_sel = (l.day_sel + 1).min(l.days.len().saturating_sub(1)),
            (1, KeyCode::Enter) => {
                if let Some(d) = l.days.get(l.day_sel).cloned() {
                    l.lines = logs::read_day(&me, &l.conv_key, &d);
                    l.day = d;
                    l.stage = 2;
                    l.scroll = usize::MAX / 2;
                    l.filter.clear();
                }
            }
            (2, KeyCode::Esc) => {
                if l.from_search && (l.filter.is_empty() || l.filter.text() == l.query) {
                    l.stage = 3;
                    l.from_search = false;
                    l.filter.clear();
                } else if l.filter.is_empty() {
                    l.stage = 1;
                } else {
                    l.filter.clear();
                }
            }
            (2, KeyCode::Up) => l.scroll = l.scroll.min(l.lines.len()).saturating_sub(1),
            (2, KeyCode::Down) => l.scroll = l.scroll.min(l.lines.len()) + 1,
            (2, KeyCode::PageUp) => l.scroll = l.scroll.min(l.lines.len()).saturating_sub(20),
            (2, KeyCode::PageDown) => l.scroll = l.scroll.min(l.lines.len()) + 20,
            (2, KeyCode::Home) => l.scroll = 0,
            (2, KeyCode::End) => l.scroll = usize::MAX / 2,
            (2, KeyCode::Char('[')) | (2, KeyCode::Char(']')) if l.filter.is_empty() => {
                // Older / newer day.
                let older = key.code == KeyCode::Char('[');
                let next = if older { l.day_sel + 1 } else { l.day_sel.wrapping_sub(1) };
                if let Some(d) = l.days.get(next).cloned() {
                    l.day_sel = next;
                    l.lines = logs::read_day(&me, &l.conv_key, &d);
                    l.day = d;
                    l.scroll = if older { usize::MAX / 2 } else { 0 };
                }
            }
            (stage, _) if stage == 0 || stage == 2 => {
                if let EditResult::Changed = l.filter.handle(key) {
                    l.sel = 0;
                    if stage == 2 {
                        l.scroll = 0;
                    }
                }
            }
            _ => {}
        }
    }

    fn search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Dialog::Search(s)) = self.dialogs.last_mut() else { return };
        if key.code == KeyCode::Esc {
            if s.focus == SearchFocus::Results {
                s.focus = SearchFocus::Options;
            } else {
                self.dialogs.pop();
            }
            return;
        }
        if s.cats.is_empty() {
            return;
        }
        match key.code {
            KeyCode::Char('s') if ctrl => {
                let fields: Vec<(String, Vec<String>)> =
                    s.cats.iter().zip(s.selected.iter()).map(|(c, sel)| (c.0.clone(), sel.iter().cloned().collect())).collect();
                let get = |k: &str| fields.iter().find(|f| f.0 == k).map(|f| f.1.clone()).unwrap_or_default();
                if get("kinks").is_empty() {
                    s.status = "Select at least one kink.".into();
                    return;
                }
                s.status = "Searching…".into();
                let line = protocol::fks(
                    &get("kinks"),
                    &get("genders"),
                    &get("orientations"),
                    &get("languages"),
                    &get("furryprefs"),
                    &get("roles"),
                );
                self.send(line);
            }
            KeyCode::Char('r') if ctrl => {
                s.focus = if s.focus == SearchFocus::Options { SearchFocus::Results } else { SearchFocus::Options };
            }
            KeyCode::Char('x') if ctrl => {
                for set in &mut s.selected {
                    set.clear();
                }
                s.results.clear();
            }
            KeyCode::Tab => {
                s.cat = (s.cat + 1) % s.cats.len();
                s.sel = 0;
                s.filter.clear();
                s.focus = SearchFocus::Options;
            }
            KeyCode::BackTab => {
                s.cat = (s.cat + s.cats.len() - 1) % s.cats.len();
                s.sel = 0;
                s.filter.clear();
                s.focus = SearchFocus::Options;
            }
            _ if s.focus == SearchFocus::Results => match key.code {
                KeyCode::Up => s.result_sel = s.result_sel.saturating_sub(1),
                KeyCode::Down => s.result_sel = (s.result_sel + 1).min(s.results.len().saturating_sub(1)),
                KeyCode::Enter => {
                    if let Some(n) = s.results.get(s.result_sel).cloned() {
                        self.open_user_menu(&n);
                    }
                }
                _ => {}
            },
            KeyCode::Up => s.sel = s.sel.saturating_sub(1),
            KeyCode::Down => s.sel = (s.sel + 1).min(s.visible().len().saturating_sub(1)),
            KeyCode::PageUp => s.sel = s.sel.saturating_sub(15),
            KeyCode::PageDown => s.sel = (s.sel + 15).min(s.visible().len().saturating_sub(1)),
            KeyCode::Enter => {
                if let Some((id, _)) = s.visible().get(s.sel) {
                    let set = &mut s.selected[s.cat];
                    if !set.remove(id) {
                        set.insert(id.clone());
                    }
                }
            }
            _ => {
                if let EditResult::Changed = s.filter.handle(key) {
                    s.sel = 0;
                }
            }
        }
    }
}

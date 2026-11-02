//! Drawing of the main chat screen.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};
use ratatui::Frame;

use super::widgets::{self, str_width, trunc};
use super::{App, ConnState, Focus};
use crate::bbcode::{self, Embed};
use crate::config::ImageMode;
use crate::state::{gender_color, ConvKind, Message, MsgKind, Status, Typing};
use crate::timeutil;

/// Screens up to this wide use the compact layout.
pub const COMPACT_MAX_WIDTH: u16 = 80;

/// A rendered row in the message view.
enum Row {
    Text(Line<'static>),
    /// Reserved rows for inline images; `first` marks the top row.
    Images {
        urls: Vec<String>,
        first: bool,
    },
}

impl App {
    pub(super) fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        widgets::clear_key_hits();
        // The layouts assume some room; draw nothing but a note in tiny windows.
        if area.width < 40 || area.height < 16 {
            let msg = format!("Window too small ({}x{})", area.width, area.height);
            f.buffer_mut().set_stringn(area.x, area.y, msg, area.width as usize, Style::default().fg(Color::Yellow));
            return;
        }
        if self.login.is_some() {
            self.draw_login(f, area);
        } else {
            self.draw_chat(f, area);
            self.draw_toasts(f.buffer_mut(), area);
        }
        let n = self.dialogs.len();
        for i in 0..n {
            if i + 1 == n {
                widgets::clear_find_rows();
            }
            self.draw_dialog(f.buffer_mut(), area, i, i + 1 == n);
        }
        // Matches of the search in the top window (Ctrl+F).
        if let Some(q) = self.dialog_find_query() {
            widgets::highlight(f.buffer_mut(), self.hit.dialog, &q);
        }
    }

    pub fn name_style(&self, name: &str) -> Style {
        let mut st = Style::default().add_modifier(Modifier::BOLD);
        if self.settings.color_names_by_gender {
            let g = self.chat.gender_of(name);
            st = st.fg(if g.is_empty() { Color::Gray } else { gender_color(&g) });
        }
        st
    }

    fn draw_chat(&mut self, f: &mut Frame, area: Rect) {
        // On screens of 80 columns or less (like the Windows 9x console),
        // save space on borders so more of the width goes to messages.
        self.compact = area.width <= COMPACT_MAX_WIDTH;
        let input_lines = {
            let w = area.width.saturating_sub(if self.compact { 0 } else { 2 }).max(10);
            let (lines, _) = self.input.layout(w);
            lines.len().clamp(1, 6) as u16
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(input_lines + if self.compact { 1 } else { 2 }), Constraint::Length(1)])
            .split(area);
        let (main, input_area, status_area) = (rows[0], rows[1], rows[2]);

        let is_channel = self.chat.active_conv().channel_id().is_some();
        let side_w = if !self.settings.show_sidebar || main.width <= 60 {
            0
        } else if self.compact {
            self.settings.sidebar_width.clamp(10, 16)
        } else {
            self.settings.sidebar_width.clamp(10, 50)
        };
        let is_console = self.chat.active_conv().kind == ConvKind::Console;
        let users_w = if self.settings.show_user_list && is_channel && main.width > 80 {
            self.settings.user_list_width.clamp(10, 50)
        } else if self.settings.show_online_friends && is_console && main.width >= 60 {
            // Online friends/bookmarks; narrower on small screens.
            self.settings.user_list_width.clamp(10, if main.width > 80 { 50 } else { 18 })
        } else {
            0
        };
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(side_w), Constraint::Min(20), Constraint::Length(users_w)])
            .split(main);

        self.hit.tabs = Rect::default();
        self.hit.users = Rect::default();
        self.hit.friends = Rect::default();
        self.hit.friend_rows.clear();
        if side_w > 0 {
            self.draw_tabs(f.buffer_mut(), cols[0]);
        }
        self.friends_panel_shown = users_w > 0 && is_console;
        self.draw_center(f.buffer_mut(), cols[1]);
        if users_w > 0 && is_console {
            self.draw_online_friends(f.buffer_mut(), cols[2]);
        } else if users_w > 0 {
            self.draw_users(f.buffer_mut(), cols[2]);
        }
        self.draw_input(f, input_area);
        self.draw_status(f.buffer_mut(), status_area);
        self.draw_completion(f.buffer_mut(), input_area);
    }

    fn draw_tabs(&mut self, buf: &mut Buffer, area: Rect) {
        let b = if self.compact {
            // Only a line on the right instead of a box.
            let focused = self.focus == Focus::Tabs;
            Block::default()
                .borders(Borders::RIGHT)
                .border_set(if self.settings.ascii_borders { widgets::ASCII_BORDER } else { ratatui::symbols::border::PLAIN })
                .border_style(Style::default().fg(if focused { Color::Yellow } else { Color::DarkGray }))
        } else {
            widgets::block("Tabs", self.settings.ascii_borders, self.focus == Focus::Tabs)
        };
        let inner = b.inner(area);
        b.render(area, buf);
        self.hit.tabs = inner;
        self.hit.tab_rows.clear();
        let h = inner.height as usize;
        let n = self.chat.conversations.len();
        let offset = if self.chat.active >= h { self.chat.active + 1 - h } else { 0 };
        for (y, i) in (inner.y..inner.bottom()).zip(offset..n) {
            let c = &self.chat.conversations[i];
            let (icon, icon_style) = match &c.kind {
                ConvKind::Console => ("»", Style::default().fg(Color::Cyan)),
                ConvKind::Channel(_) => {
                    (if c.joined { "#" } else { "×" }, Style::default().fg(if c.joined { Color::Cyan } else { Color::DarkGray }))
                }
                ConvKind::Private(n) => {
                    let st = self.chat.status_of(n);
                    (if c.typing == Typing::Typing { "✎" } else { st.symbol() }, Style::default().fg(st.color()))
                }
            };
            let mut name_style = match &c.kind {
                ConvKind::Private(n) => self.name_style(n).remove_modifier(Modifier::BOLD),
                _ => Style::default(),
            };
            if c.unread_highlight {
                name_style = name_style.fg(Color::Yellow).add_modifier(Modifier::BOLD);
            } else if c.unread > 0 {
                name_style = name_style.add_modifier(Modifier::BOLD);
            }
            if c.muted {
                name_style = name_style.add_modifier(Modifier::DIM);
            }
            let num = if i < 10 { format!("{} ", (i + 1) % 10) } else { "  ".into() };
            let unread = if c.unread > 0 { format!(" {}", c.unread.min(999)) } else { String::new() };
            let avail = (inner.width as usize).saturating_sub(5 + unread.len());
            let mut spans = vec![
                Span::styled(num, Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{icon} "), icon_style),
                Span::styled(trunc(&c.title, avail), name_style),
            ];
            if !unread.is_empty() {
                spans.push(Span::styled(unread, Style::default().fg(if c.unread_highlight { Color::Yellow } else { Color::Gray })));
            }
            if i == self.chat.active {
                buf.set_style(Rect::new(inner.x, y, inner.width, 1), Style::default().bg(Color::Rgb(45, 45, 70)));
            }
            buf.set_line(inner.x, y, &Line::from(spans), inner.width);
            self.hit.tab_rows.push((y, i));
        }
    }

    /// Online friends and bookmarks with their status, as (name, status, is friend).
    pub(super) fn online_friends(&self) -> Vec<(String, Status, bool)> {
        let mut list: Vec<(String, Status, bool)> = self
            .chat
            .friends
            .iter()
            .chain(self.chat.bookmarks.iter())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter_map(|n| self.chat.characters.get(n).map(|c| (c.name.clone(), c.status, self.chat.is_friend(n))))
            .collect();
        list.sort_by_key(|(n, _, _)| n.to_lowercase());
        list
    }

    /// Online/offline notifications at the top right, on every tab except the
    /// console while it shows the online panel anyway.
    fn draw_toasts(&mut self, buf: &mut Buffer, area: Rect) {
        if self.toasts.is_empty() || self.friends_panel_shown {
            return;
        }
        let lines: Vec<Line<'static>> = self
            .toasts
            .iter()
            .map(|(name, online, _)| {
                let st = if *online { self.chat.status_of(name) } else { Status::Offline };
                let (mark, mark_color) = if self.chat.is_friend(name) { ("♥", Color::LightRed) } else { ("★", Color::Yellow) };
                Line::from(vec![
                    Span::styled(format!("{} ", st.symbol()), Style::default().fg(st.color())),
                    Span::styled(format!("{mark} "), Style::default().fg(mark_color)),
                    Span::styled(name.clone(), self.name_style(name)),
                    Span::styled(if *online { " is online" } else { " went offline" }, Style::default().fg(Color::Gray)),
                ])
            })
            .collect();
        let w = (lines.iter().map(|l| l.width()).max().unwrap_or(10) as u16 + 4).min(area.width);
        let h = (lines.len() as u16 + 2).min(area.height);
        let r = Rect::new(area.right().saturating_sub(w), area.y, w, h);
        let inner = widgets::popup(buf, r, "Notifications", self.settings.ascii_borders);
        for (i, l) in lines.iter().take(inner.height as usize).enumerate() {
            buf.set_line(inner.x + 1, inner.y + i as u16, l, inner.width.saturating_sub(1));
        }
    }

    /// The console tab's panel at the top right: online friends and bookmarks.
    fn draw_online_friends(&mut self, buf: &mut Buffer, area: Rect) {
        let list = self.online_friends();
        let h = (list.len().max(1) as u16 + 2).min(area.height);
        let area = Rect { height: h, ..area };
        let b = widgets::block(&format!("Online ({})", list.len()), self.settings.ascii_borders, false);
        let inner = b.inner(area);
        b.render(area, buf);
        self.hit.friends = inner;
        if list.is_empty() {
            buf.set_stringn(inner.x, inner.y, "No friends online", inner.width as usize, Style::default().fg(Color::DarkGray));
            return;
        }
        let rows = inner.height as usize;
        self.friends_scroll = self.friends_scroll.min(list.len().saturating_sub(rows));
        for (i, (name, st, friend)) in list.iter().skip(self.friends_scroll).take(rows).enumerate() {
            let y = inner.y + i as u16;
            let (mark, mark_color) = if *friend { ("♥", Color::LightRed) } else { ("★", Color::Yellow) };
            let label = st.label();
            // The status as a word too, if there is room for it.
            let show_label = inner.width as usize >= 4 + str_width(name) + 1 + label.len();
            let name_w = (inner.width as usize).saturating_sub(4 + if show_label { label.len() + 1 } else { 0 });
            let line = Line::from(vec![
                Span::styled(format!("{} ", st.symbol()), Style::default().fg(st.color())),
                Span::styled(format!("{mark} "), Style::default().fg(mark_color)),
                Span::styled(trunc(name, name_w), self.name_style(name).remove_modifier(Modifier::BOLD)),
            ]);
            buf.set_line(inner.x, y, &line, inner.width);
            if show_label {
                let x = inner.right().saturating_sub(label.len() as u16);
                buf.set_string(x, y, label, Style::default().fg(st.color()));
            }
            self.hit.friend_rows.push((y, name.clone()));
        }
        if list.len() > rows {
            buf.set_string(inner.right().saturating_sub(1), inner.bottom().saturating_sub(1), "↓", Style::default().fg(Color::DarkGray));
        }
    }

    fn draw_users(&mut self, buf: &mut Buffer, area: Rect) {
        let conv = self.chat.active_conv();
        let members = self.chat.sorted_members(conv);
        let title = format!("Users ({})", members.len());
        let b = widgets::block(&title, self.settings.ascii_borders, self.focus == Focus::Users);
        let inner = b.inner(area);
        b.render(area, buf);
        self.hit.users = inner;
        self.hit.user_rows.clear();
        if members.is_empty() {
            return;
        }
        self.user_sel = self.user_sel.min(members.len() - 1);
        let h = inner.height as usize;
        let offset = if self.user_sel >= h { self.user_sel + 1 - h } else { 0 };
        for (row, name) in members.iter().skip(offset).take(h).enumerate() {
            let y = inner.y + row as u16;
            let st = self.chat.status_of(name);
            let rank = if conv.is_owner(name) {
                ("~", Color::LightRed)
            } else if conv.is_op(name) {
                ("@", Color::LightGreen)
            } else if self.chat.is_chatop(name) {
                ("&", Color::LightMagenta)
            } else if self.chat.is_friend(name) {
                ("♥", Color::LightRed)
            } else if self.chat.is_bookmarked(name) {
                ("★", Color::Yellow)
            } else {
                (" ", Color::Reset)
            };
            let mut ns = self.name_style(name).remove_modifier(Modifier::BOLD);
            if self.chat.is_ignored(name) {
                ns = ns.add_modifier(Modifier::CROSSED_OUT | Modifier::DIM);
            }
            let line = Line::from(vec![
                Span::styled(format!("{} ", st.symbol()), Style::default().fg(st.color())),
                Span::styled(rank.0.to_string(), Style::default().fg(rank.1)),
                Span::styled(trunc(name, inner.width.saturating_sub(3) as usize), ns),
            ]);
            if self.focus == Focus::Users && offset + row == self.user_sel {
                buf.set_style(Rect::new(inner.x, y, inner.width, 1), Style::default().bg(Color::Rgb(40, 60, 100)));
            }
            buf.set_line(inner.x, y, &line, inner.width);
            self.hit.user_rows.push((y, name.clone()));
        }
    }

    fn header_lines(&self, width: usize) -> Vec<Line<'static>> {
        let conv = self.chat.active_conv();
        match &conv.kind {
            ConvKind::Console => {
                let users = self.chat.user_count.max(self.chat.characters.len() as i64);
                // The online panel shows friends/bookmarks itself.
                let text = if self.friends_panel_shown {
                    format!("  {users} users online")
                } else {
                    format!("  {users} users online · {} bookmarks/friends online", self.online_watched())
                };
                vec![Line::from(vec![
                    Span::styled("Console", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)),
                    Span::styled(format!(" · F-Term v{}", crate::protocol::CLIENT_VERSION), Style::default().fg(Color::DarkGray)),
                    Span::styled(text, Style::default().fg(Color::DarkGray)),
                ])]
            }
            ConvKind::Channel(id) => {
                let mode = match conv.mode.as_str() {
                    "chat" => "chat only",
                    "ads" => "ads only",
                    _ => "chat & ads",
                };
                let mut flags = Vec::new();
                if !conv.show_ads && conv.mode != "chat" {
                    flags.push("ads hidden");
                }
                if conv.muted {
                    flags.push("muted");
                }
                if !conv.joined {
                    flags.push("not joined");
                }
                let mut l1 = vec![
                    Span::styled(format!("#{}", conv.title), Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)),
                    Span::styled(format!("  {} users · {mode}", conv.members.len()), Style::default().fg(Color::DarkGray)),
                ];
                if id.starts_with("ADH-") {
                    l1.push(Span::styled(format!(" · {id}"), Style::default().fg(Color::DarkGray)));
                }
                if !flags.is_empty() {
                    l1.push(Span::styled(format!(" · {}", flags.join(", ")), Style::default().fg(Color::Yellow)));
                }
                let desc = bbcode::strip(&conv.description).replace(['\r', '\n'], " ");
                vec![Line::from(l1), Line::from(Span::styled(trunc(desc.trim(), width), Style::default().fg(Color::Gray)))]
            }
            ConvKind::Private(name) => {
                let ch = self.chat.character(name);
                let st = ch.map(|c| c.status).unwrap_or(Status::Offline);
                let mut l1 = vec![
                    Span::styled(name.clone(), self.name_style(name)),
                    Span::styled(format!("  {}", st.label()), Style::default().fg(st.color())),
                ];
                if let Some(c) = ch {
                    if !c.gender.is_empty() {
                        l1.push(Span::styled(format!(" · {}", c.gender), Style::default().fg(Color::DarkGray)));
                    }
                }
                match conv.typing {
                    Typing::Typing => l1.push(Span::styled("  is typing…", Style::default().fg(Color::LightGreen))),
                    Typing::Paused => l1.push(Span::styled("  has entered text", Style::default().fg(Color::DarkGray))),
                    Typing::Clear => {}
                }
                if self.chat.is_friend(name) {
                    l1.push(Span::styled("  ♥ friend", Style::default().fg(Color::LightRed)));
                } else if self.chat.is_bookmarked(name) {
                    l1.push(Span::styled("  ★ bookmark", Style::default().fg(Color::Yellow)));
                }
                if self.chat.is_ignored(name) {
                    l1.push(Span::styled("  (ignored)", Style::default().fg(Color::Red)));
                }
                let msg = ch.map(|c| bbcode::strip(&c.status_msg)).unwrap_or_default().replace(['\r', '\n'], " ");
                vec![Line::from(l1), Line::from(Span::styled(trunc(msg.trim(), width), Style::default().fg(Color::Gray)))]
            }
        }
    }

    fn online_watched(&self) -> usize {
        self.chat
            .friends
            .iter()
            .chain(self.chat.bookmarks.iter())
            .filter(|n| self.chat.characters.contains_key(*n))
            .collect::<std::collections::HashSet<_>>()
            .len()
    }

    fn draw_center(&mut self, buf: &mut Buffer, area: Rect) {
        let header = self.header_lines(area.width as usize);
        let hh = header.len() as u16;
        let parts =
            Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(hh + 1), Constraint::Min(1)]).split(area);
        let hb = Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray));
        let hinner = hb.inner(parts[0]);
        hb.render(parts[0], buf);
        for (i, l) in header.iter().enumerate() {
            buf.set_line(hinner.x + 1, hinner.y + i as u16, l, hinner.width.saturating_sub(1));
        }
        let msg_area = Rect::new(parts[1].x + 1, parts[1].y, parts[1].width.saturating_sub(1), parts[1].height);
        self.hit.messages = msg_area;
        self.draw_messages(buf, msg_area);
    }

    fn message_rows(&self, m: &Message, width: usize, show_ads: bool) -> Vec<Row> {
        if m.kind == MsgKind::Ad && !show_ads {
            return Vec::new();
        }
        let s = &self.settings;
        let mut prefix: Vec<Span<'static>> = Vec::new();
        if s.show_timestamps {
            let t = timeutil::fmt_time(m.ts, s.timestamp_seconds);
            let st = if m.highlight { Style::default().fg(Color::Black).bg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
            prefix.push(Span::styled(if self.compact { t } else { format!("[{t}]") }, st));
            prefix.push(Span::raw(" "));
        } else if m.highlight {
            prefix.push(Span::styled("!", Style::default().fg(Color::Black).bg(Color::Yellow)));
            prefix.push(Span::raw(" "));
        }
        let base = match m.kind {
            MsgKind::System => Style::default().fg(Color::Gray),
            MsgKind::Error => Style::default().fg(Color::LightRed),
            MsgKind::Event => Style::default().fg(Color::DarkGray),
            MsgKind::History => Style::default().fg(Color::DarkGray),
            MsgKind::Warn => Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD),
            MsgKind::Broadcast => Style::default().fg(Color::LightMagenta).add_modifier(Modifier::BOLD),
            _ => Style::default(),
        };
        let mut text = m.text.clone();
        match m.kind {
            MsgKind::Chat | MsgKind::Warn | MsgKind::History => {
                let ns = if m.kind == MsgKind::History {
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD)
                } else {
                    self.name_style(&m.sender)
                };
                prefix.push(Span::styled(m.sender.clone(), ns));
                prefix.push(Span::styled(": ", Style::default().fg(Color::DarkGray)));
            }
            MsgKind::Action => {
                prefix.push(Span::styled("* ", Style::default().fg(Color::Magenta)));
                if !m.sender.is_empty() {
                    prefix.push(Span::styled(m.sender.clone(), self.name_style(&m.sender).add_modifier(Modifier::ITALIC)));
                    if !text.starts_with('\'') {
                        prefix.push(Span::raw(" "));
                    }
                }
            }
            MsgKind::Ad => {
                prefix.push(Span::styled(" AD ", Style::default().fg(Color::Black).bg(Color::Green)));
                prefix.push(Span::raw(" "));
                prefix.push(Span::styled(m.sender.clone(), self.name_style(&m.sender)));
                prefix.push(Span::styled(": ", Style::default().fg(Color::DarkGray)));
            }
            MsgKind::Roll => prefix.push(Span::styled("» ", Style::default().fg(Color::LightCyan))),
            MsgKind::Broadcast => text = format!("[BROADCAST] {text}"),
            MsgKind::Event => prefix.push(Span::styled("→ ", Style::default().fg(Color::DarkGray))),
            MsgKind::System | MsgKind::Error => prefix.push(Span::styled("-!- ", base)),
        }
        let base = if m.kind == MsgKind::Action { base.add_modifier(Modifier::ITALIC) } else { base };
        let user_style = |n: &str| self.name_style(n);
        let body = bbcode::to_lines(&text, base, &user_style, s.show_eicon_names);
        let mut rows: Vec<Row> = Vec::new();
        let indent = 2;
        for (i, l) in body.into_iter().enumerate() {
            let line = if i == 0 {
                let mut spans = prefix.clone();
                spans.extend(l.spans);
                Line::from(spans)
            } else {
                l
            };
            for w in widgets::wrap(&line, width, indent) {
                rows.push(Row::Text(w));
            }
        }
        // The GUI window shows real images, so it has its own setting (on by default).
        let inline = if crate::images::pixel_output().is_some() { s.gui_inline_images } else { s.inline_images };
        if inline
            && s.active_image_mode() != ImageMode::Off
            && matches!(m.kind, MsgKind::Chat | MsgKind::Action | MsgKind::Ad | MsgKind::History)
        {
            let urls: Vec<String> = bbcode::embeds(&m.text)
                .into_iter()
                .map(|e| match e {
                    Embed::Eicon(n) => crate::images::eicon_url(&n),
                    Embed::Icon(n) => crate::images::avatar_url(&n),
                })
                .take(8)
                .collect();
            if !urls.is_empty() {
                let n = s.inline_image_rows.clamp(1, 20);
                for r in 0..n {
                    rows.push(Row::Images { urls: urls.clone(), first: r == 0 });
                }
            }
        }
        rows
    }

    /// Scroll the current tab so message `i` starts in the middle of the view.
    pub(super) fn scroll_to_message(&mut self, i: usize) {
        let width = (self.hit.messages.width as usize).max(5);
        let h = (self.hit.messages.height as usize).max(2);
        let conv = &self.chat.conversations[self.chat.active];
        let mut below = 0;
        for (k, m) in conv.messages.iter().enumerate().skip(i) {
            below += self.message_rows(m, width, conv.show_ads).len();
            if conv.marker == Some(k) && k > i {
                below += 1;
            }
        }
        self.chat.active_conv_mut().scroll = below.saturating_sub(h / 2);
    }

    fn draw_messages(&mut self, buf: &mut Buffer, area: Rect) {
        if area.height == 0 || area.width < 5 {
            return;
        }
        let width = area.width as usize;
        let h = area.height as usize;
        let idx = self.chat.active.min(self.chat.conversations.len() - 1);
        let conv = &self.chat.conversations[idx];
        let show_ads = conv.show_ads;
        let mut scroll = conv.scroll;

        // Collect rows from the newest message backwards until the view is filled.
        let mut rows: Vec<Row> = Vec::new();
        let need = h + scroll;
        let mut total_known_complete = true;
        let n_msgs = conv.messages.len();
        for (k, m) in conv.messages.iter().rev().enumerate() {
            let mut r = self.message_rows(m, width, show_ads);
            if conv.find_hit == Some(n_msgs - 1 - k) {
                for row in &mut r {
                    if let Row::Text(l) = row {
                        *l = std::mem::take(l).patch_style(Style::default().add_modifier(Modifier::REVERSED));
                    }
                }
            }
            if conv.marker == Some(n_msgs - 1 - k) {
                r.insert(0, Row::Text(new_messages_line(width, self.settings.ascii_borders)));
            }
            r.append(&mut rows);
            rows = r;
            if rows.len() > need {
                total_known_complete = false;
                break;
            }
        }
        // Clamp scrolling to the available history.
        let max_scroll = rows.len().saturating_sub(h);
        if total_known_complete && scroll > max_scroll {
            scroll = max_scroll;
            self.chat.conversations[idx].scroll = scroll;
        } else if scroll > max_scroll {
            scroll = max_scroll;
        }
        let end = rows.len() - scroll.min(rows.len());
        let start = end.saturating_sub(h);
        // Bottom-align when there are fewer rows than the height.
        let top_pad = h - (end - start);
        let mut images: Vec<(Rect, Vec<String>)> = Vec::new();
        for (i, row) in rows[start..end].iter().enumerate() {
            let y = area.y + (top_pad + i) as u16;
            match row {
                Row::Text(l) => {
                    buf.set_line(area.x, y, l, area.width);
                }
                Row::Images { urls, first } => {
                    let n = self.settings.inline_image_rows.clamp(1, 20) as usize;
                    // Only draw when all rows of the image block are visible.
                    if *first && top_pad + i + n <= h {
                        images.push((Rect::new(area.x + 2, y, area.width.saturating_sub(2), n as u16), urls.clone()));
                    }
                }
            }
        }
        let (mode, proto, tc) = (self.settings.active_image_mode(), self.settings.graphics_protocol, self.settings.truecolor);
        for (rect, urls) in images {
            let mut x = rect.x;
            let size = rect.height * 2;
            for url in urls {
                if x + 2 > rect.right() {
                    break;
                }
                self.images.request(&url, &self.tx, super::AppEvent::Image);
                let w = size.min(rect.right() - x);
                let (used, _) = self.images.draw(buf, Rect::new(x, rect.y, w, rect.height), &url, mode, proto, tc);
                x += used.max(1) + 1;
            }
        }
        if scroll > 0 {
            let label = format!(" ↓ {scroll} more lines (End/Ctrl+End) ");
            let x = area.right().saturating_sub(str_width(&label) as u16 + 1);
            buf.set_string(x, area.bottom() - 1, label, Style::default().fg(Color::Black).bg(Color::Yellow));
        }
        if self.chat.conversations[idx].messages.is_empty() && idx == 0 && self.conn_state == ConnState::Offline {
            buf.set_string(area.x, area.y, "Not connected.", Style::default().fg(Color::DarkGray));
        }
    }

    fn draw_input(&mut self, f: &mut Frame, area: Rect) {
        let conv = self.chat.active_conv();
        let (target, limit) = match &conv.kind {
            ConvKind::Console => ("console (commands only)".to_string(), 0),
            ConvKind::Channel(_) => (format!("#{}", conv.title), self.chat.vars.chat_max),
            ConvKind::Private(n) => (n.clone(), self.chat.vars.priv_max),
        };
        let len = self.input.text().len();
        let mut title = if limit > 0 && len > 0 { format!("→ {target}  {len}/{limit}") } else { format!("→ {target}") };
        if conv.mode != "chat" {
            if let Some(left) = conv.ad_wait(self.chat.vars.lfrp_flood) {
                title.push_str(&format!("  · next ad {}", crate::timeutil::mmss(left)));
            }
        }
        let mut b = widgets::block(&title, self.settings.ascii_borders, self.focus == Focus::Input);
        if self.compact {
            b = b.borders(Borders::TOP);
        }
        let inner = b.inner(area);
        b.render(area, f.buffer_mut());
        self.hit.input = area;
        let (lines, (cx, cy)) = self.input.layout(inner.width);
        let h = inner.height as usize;
        let offset = if (cy as usize) >= h { cy as usize + 1 - h } else { 0 };
        for (i, l) in lines.iter().skip(offset).take(h).enumerate() {
            let st = if l.starts_with('/') && i == 0 && offset == 0 { Style::default().fg(Color::LightCyan) } else { Style::default() };
            f.buffer_mut().set_string(inner.x, inner.y + i as u16, l, st);
        }
        if self.input.is_empty() && self.focus == Focus::Input {
            let hint = match conv.kind {
                ConvKind::Console => "Type /help for commands, /join <channel> or F2 to browse channels",
                _ => "Type a message. Enter sends, Alt+Enter adds a line, Tab completes",
            };
            f.buffer_mut().set_stringn(inner.x, inner.y, hint, inner.width as usize, Style::default().fg(Color::DarkGray));
        }
        if self.focus == Focus::Input && self.dialogs.is_empty() {
            f.set_cursor_position((inner.x + cx, inner.y + (cy as usize - offset) as u16));
        }
    }

    fn draw_status(&self, buf: &mut Buffer, area: Rect) {
        let me = &self.chat.me;
        let st = self.my_status;
        let conn = match self.conn_state {
            ConnState::Online => Span::styled(" online ", Style::default().fg(Color::Black).bg(Color::Green)),
            ConnState::Connecting => Span::styled(" connecting ", Style::default().fg(Color::Black).bg(Color::Yellow)),
            ConnState::Offline => {
                let txt = match self.reconnect_at {
                    Some(at) => format!(" offline · retry in {}s ", at.saturating_duration_since(std::time::Instant::now()).as_secs()),
                    None => " offline ".into(),
                };
                Span::styled(txt, Style::default().fg(Color::White).bg(Color::Red))
            }
        };
        let mut spans = vec![
            conn,
            Span::raw(" "),
            Span::styled(me.clone(), self.name_style(me)),
            Span::raw(" "),
            Span::styled(format!("[{}]", st.label()), Style::default().fg(st.color())),
        ];
        if !self.my_status_msg.is_empty() {
            spans.push(Span::styled(format!(" {}", trunc(&bbcode::strip(&self.my_status_msg), 30)), Style::default().fg(Color::DarkGray)));
        }
        if let Some(q) = self.dialog_find_query() {
            spans.push(Span::styled(
                format!("  find \"{}\": Ctrl+F next · Ctrl+G previous · Esc ends", trunc(&q, 20)),
                Style::default().fg(Color::LightCyan),
            ));
        } else if let Some(q) = &self.find {
            spans.push(Span::styled(
                format!("  find \"{}\": Ctrl+F older · Ctrl+G newer · Esc ends", trunc(q, 20)),
                Style::default().fg(Color::LightCyan),
            ));
        }
        if let Some((msg, _)) = &self.flash {
            spans.push(Span::styled(format!("  {msg}"), Style::default().fg(Color::Yellow)));
        }
        let unread: usize = self.chat.conversations.iter().filter(|c| c.unread_highlight).count();
        if unread > 0 {
            // Clicking it jumps to the tab, like Alt+A.
            let x = area.x + spans.iter().map(|s| str_width(&s.content)).sum::<usize>() as u16 + 2;
            let text = format!("⚑ {unread} (Alt+A)");
            widgets::add_key_hit(Rect::new(x, area.y, str_width(&text) as u16, 1), KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT));
            spans.push(Span::styled(format!("  {text}"), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
        }
        for (text, key) in self.site_counter_spans() {
            let x = area.x + spans.iter().map(|s| str_width(&s.content)).sum::<usize>() as u16 + 2;
            widgets::add_key_hit(Rect::new(x, area.y, str_width(&text) as u16, 1), key);
            spans.push(Span::styled(format!("  {text}"), Style::default().fg(Color::LightMagenta).add_modifier(Modifier::BOLD)));
        }
        let used: usize = spans.iter().map(|s| str_width(&s.content)).sum();
        buf.set_line(area.x, area.y, &Line::from(spans), area.width);
        // Function keys at the right, if they fit (clickable with the mouse).
        let hints = [("F1", "Help"), ("F2", "Chan"), ("F3", "Friends"), ("F4", "Profile"), ("F5", "Status"), ("F6", "Set"), ("F7", "Logs")];
        let hw: usize = hints.iter().map(|(k, v)| str_width(k) + 1 + str_width(v)).sum::<usize>() + 2 * (hints.len() - 1);
        if used + hw + 2 <= area.width as usize {
            widgets::draw_hints(buf, area.right() - hw as u16, area.y, &hints, hw as u16);
        }
    }

    fn draw_completion(&mut self, buf: &mut Buffer, input_area: Rect) {
        let Some(c) = &self.completion else { return };
        if c.items.is_empty() || self.focus != Focus::Input {
            return;
        }
        let n = c.items.len().min(10) as u16;
        let width = (c.items.iter().map(|(a, b)| str_width(a) + str_width(b) + 3).max().unwrap_or(10) as u16 + 4)
            .max(20)
            .min(input_area.width.saturating_sub(2));
        let y = input_area.y.saturating_sub(n + 2);
        let area = Rect::new(input_area.x + 1, y, width, n + 2);
        let inner =
            widgets::popup(buf, area, &format!("{} matches · Tab/↑↓ select · Enter accept", c.items.len()), self.settings.ascii_borders);
        let items: Vec<Line<'static>> = c
            .items
            .iter()
            .map(|(a, b)| {
                Line::from(vec![
                    Span::styled(a.clone(), Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD)),
                    Span::styled(if b.is_empty() { String::new() } else { format!("  {b}") }, Style::default().fg(Color::Gray)),
                ])
            })
            .collect();
        widgets::list(buf, inner, &items, c.sel, true);
    }
}

/// The "new messages" separator drawn above the first unread message.
fn new_messages_line(width: usize, ascii: bool) -> Line<'static> {
    let label = " new messages ";
    let dash = if ascii { "-" } else { "─" };
    let side = width.saturating_sub(label.len()) / 2;
    let rest = width.saturating_sub(label.len() + side);
    Line::from(Span::styled(format!("{}{label}{}", dash.repeat(side), dash.repeat(rest)), Style::default().fg(Color::LightRed)))
}

//! Character profile viewer.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::actions::Action;
use super::widgets::{self, trunc};
use super::{ApiReply, App, Dialog};
use crate::api::Mapping;
use crate::bbcode;
use crate::config::ImageMode;
use crate::protocol::{vi, vs};
use crate::timeutil;

pub const TABS: [&str; 7] = ["Overview", "Description", "Kinks", "Images", "Friends", "Guestbook", "Groups"];

pub struct ProfileState {
    pub name: String,
    pub data: Option<Result<Value, String>>,
    pub tab: usize,
    pub scroll: usize,
    pub image_sel: usize,
    pub friend_sel: usize,
    pub guestbook: Option<(u32, Result<Value, String>)>,
    pub gb_page: u32,
    pub friends: Option<Result<Vec<String>, String>>,
    pub groups: Option<Result<Vec<String>, String>>,
}

impl ProfileState {
    pub fn new(name: &str) -> ProfileState {
        ProfileState {
            name: name.to_string(),
            data: None,
            tab: 0,
            scroll: 0,
            image_sel: 0,
            friend_sel: 0,
            guestbook: None,
            gb_page: 0,
            friends: None,
            groups: None,
        }
    }

    pub fn set_data(&mut self, d: Result<Value, String>) {
        if let Ok(v) = &d {
            let n = vs(v, "name");
            if !n.is_empty() {
                self.name = n;
            }
        }
        self.data = Some(d);
    }

    fn images(&self) -> Vec<(String, String)> {
        let Some(Ok(v)) = &self.data else { return Vec::new() };
        let mut imgs: Vec<(i64, String, String)> = v
            .get("images")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|i| {
                        let id = if vs(i, "image_id").is_empty() { vs(i, "id") } else { vs(i, "image_id") };
                        (vi(i, "sort_order"), crate::images::charimage_url(&id, &vs(i, "extension")), vs(i, "description"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        imgs.sort_by_key(|i| i.0);
        imgs.into_iter().map(|(_, u, d)| (u, d)).collect()
    }

    fn friend_list(&self) -> Vec<String> {
        match &self.friends {
            Some(Ok(f)) => f.clone(),
            _ => Vec::new(),
        }
    }
}

/// Your kinks: kink id -> choice ("fave", "yes", "maybe", "no").
pub type MyKinks = std::collections::HashMap<String, String>;

/// A profile's info fields by group, in the website's order: (group, [(field, value)]).
pub fn info_groups(v: &Value, map: &Mapping) -> Vec<(String, Vec<(String, String)>)> {
    let tags: Vec<(String, String)> = v
        .get("infotags")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .map(|(k, val)| {
                    (
                        k.clone(),
                        match val {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    for (gid, gname) in &map.infotag_groups {
        let mut rows: Vec<(String, String)> = Vec::new();
        for (tid, val) in &tags {
            if let Some((name, typ, _list, group)) = map.infotags.get(tid) {
                if group != gid {
                    continue;
                }
                let shown = if typ == "list" { map.listitems.get(val).cloned().unwrap_or_else(|| val.clone()) } else { bbcode::strip(val) };
                rows.push((name.clone(), shown));
            }
        }
        if !rows.is_empty() {
            rows.sort();
            out.push((gname.clone(), rows));
        }
    }
    out
}

pub fn my_kinks_from(v: &Value) -> MyKinks {
    v.get("kinks")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, c)| c.as_str().map(|c| (k.clone(), c.to_string()))).collect())
        .unwrap_or_default()
}

fn likes(choice: &str) -> bool {
    matches!(choice, "fave" | "yes")
}

/// How a kink of theirs compares with yours: Some(true) = both like it,
/// Some(false) = one of you likes it and the other said no.
fn kink_fit(theirs: &str, mine: Option<&str>) -> Option<bool> {
    let mine = mine?;
    if likes(theirs) && likes(mine) {
        Some(true)
    } else if (likes(theirs) && mine == "no") || (theirs == "no" && likes(mine)) {
        Some(false)
    } else {
        None
    }
}

/// (shared likes, shared favourites, conflicts) between a profile and yours.
pub fn kink_match(v: &Value, mine: &MyKinks) -> (usize, usize, usize) {
    let (mut shared, mut faves, mut conflicts) = (0, 0, 0);
    if let Some(kinks) = v.get("kinks").and_then(Value::as_object) {
        for (id, c) in kinks {
            let theirs = c.as_str().unwrap_or("");
            match kink_fit(theirs, mine.get(id).map(String::as_str)) {
                Some(true) => {
                    shared += 1;
                    if theirs == "fave" && mine.get(id).map(|m| m == "fave").unwrap_or(false) {
                        faves += 1;
                    }
                }
                Some(false) => conflicts += 1,
                None => {}
            }
        }
    }
    (shared, faves, conflicts)
}

fn overview_lines(v: &Value, map: Option<&Mapping>, status: Option<(String, String)>, mine: Option<&MyKinks>) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let key = Style::default().fg(Color::Gray);
    let head = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    out.push(Line::from(Span::styled(vs(v, "name"), Style::default().add_modifier(Modifier::BOLD).fg(Color::White))));
    let title = vs(v, "custom_title");
    if !title.is_empty() && title != "null" {
        out.push(Line::from(Span::styled(title, Style::default().fg(Color::Yellow))));
    }
    if let Some((st, msg)) = status {
        out.push(Line::from(vec![Span::styled("Status: ", key), Span::raw(st)]));
        if !msg.is_empty() {
            out.push(Line::from(Span::styled(bbcode::strip(&msg), Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC))));
        }
    } else {
        out.push(Line::from(vec![Span::styled("Status: ", key), Span::styled("offline", Style::default().fg(Color::DarkGray))]));
    }
    let badges = v
        .get("badges")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|b| b.as_str().map(String::from)).collect::<Vec<_>>())
        .unwrap_or_default();
    if !badges.is_empty() {
        out.push(Line::from(vec![
            Span::styled("Badges: ", key),
            Span::styled(badges.join(", "), Style::default().fg(Color::LightMagenta)),
        ]));
    }
    let created = vi(v, "created_at");
    let updated = vi(v, "updated_at");
    if created > 0 {
        out.push(Line::from(vec![Span::styled("Created: ", key), Span::raw(timeutil::fmt_date(created))]));
    }
    if updated > 0 {
        out.push(Line::from(vec![Span::styled("Updated: ", key), Span::raw(timeutil::fmt_date(updated))]));
    }
    out.push(Line::from(vec![Span::styled("Views: ", key), Span::raw(vi(v, "views").to_string())]));
    if let Some(mine) = mine.filter(|m| !m.is_empty()) {
        let (shared, faves, conflicts) = kink_match(v, mine);
        out.push(Line::from(vec![
            Span::styled("Kink match: ", key),
            Span::styled(format!("{shared} liked by both"), Style::default().fg(Color::LightGreen)),
            Span::raw(format!(" ({faves} favorite{}), ", if faves == 1 { "" } else { "s" })),
            Span::styled(
                format!("{conflicts} conflict{}", if conflicts == 1 { "" } else { "s" }),
                Style::default().fg(if conflicts > 0 { Color::LightRed } else { Color::Gray }),
            ),
            Span::styled(" (see Kinks)", Style::default().fg(Color::DarkGray)),
        ]));
    }
    if let Some(m) = v.get("memo") {
        let memo = vs(m, "memo");
        if !memo.is_empty() {
            out.push(Line::default());
            out.push(Line::from(Span::styled("Your memo", head)));
            for l in memo.lines() {
                out.push(Line::from(Span::styled(l.to_string(), Style::default().fg(Color::LightYellow))));
            }
        }
    }
    // Info tags grouped.
    let Some(map) = map else {
        out.push(Line::default());
        out.push(Line::from(Span::styled("Loading field names…", Style::default().fg(Color::DarkGray))));
        return out;
    };
    for (gname, rows) in info_groups(v, map) {
        out.push(Line::default());
        out.push(Line::from(Span::styled(gname.clone(), head)));
        for (n, val) in rows {
            out.push(Line::from(vec![Span::styled(format!("{n}: "), key), Span::raw(val)]));
        }
    }
    out
}

fn kink_lines(v: &Value, map: Option<&Mapping>, width: u16, mine: Option<&MyKinks>, ascii: bool) -> Vec<Line<'static>> {
    let choices = ["fave", "yes", "maybe", "no"];
    let titles = ["Favorites", "Yes", "Maybe", "No"];
    let colors = [Color::LightGreen, Color::Green, Color::Yellow, Color::LightRed];
    let mut cols: Vec<Vec<(String, bool, Option<bool>)>> = vec![Vec::new(); 4];
    let fit = |id: &str, ci: usize| mine.and_then(|m| kink_fit(choices[ci], m.get(id).map(String::as_str)));
    let mut sub_kinks = std::collections::HashSet::new();
    if let Some(customs) = v.get("custom_kinks").and_then(Value::as_object) {
        for (_, c) in customs {
            let choice = vs(c, "choice");
            let Some(ci) = choices.iter().position(|x| *x == choice) else { continue };
            cols[ci].push((format!("*{}", vs(c, "name")), true, None));
            if let Some(children) = c.get("children").and_then(Value::as_array) {
                for ch in children {
                    let id = match ch {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    sub_kinks.insert(id.clone());
                    let f = fit(&id, ci);
                    let name = map.and_then(|m| m.kinks.get(&id)).map(|k| k.0.clone()).unwrap_or(id);
                    cols[ci].push((format!("  · {name}"), false, f));
                }
            }
        }
    }
    if let Some(kinks) = v.get("kinks").and_then(Value::as_object) {
        let mut std_kinks: Vec<(usize, String, Option<bool>)> = Vec::new();
        for (id, choice) in kinks {
            if sub_kinks.contains(id) {
                continue;
            }
            let choice = choice.as_str().unwrap_or("");
            let Some(ci) = choices.iter().position(|x| *x == choice) else { continue };
            let name = map.and_then(|m| m.kinks.get(id)).map(|k| k.0.clone()).unwrap_or_else(|| format!("#{id}"));
            std_kinks.push((ci, name, fit(id, ci)));
        }
        std_kinks.sort_by_key(|k| k.1.to_lowercase());
        for (ci, name, f) in std_kinks {
            cols[ci].push((name, false, f));
        }
    }
    let (yes_mark, no_mark) = if ascii { ("+", "x") } else { ("✓", "✗") };
    let mark = |f: Option<bool>| match f {
        Some(true) => Span::styled(format!("{yes_mark} "), Style::default().fg(Color::LightGreen)),
        Some(false) => Span::styled(format!("{no_mark} "), Style::default().fg(Color::LightRed)),
        None => Span::raw(if mine.is_some() { "  " } else { "" }),
    };
    let mut out = Vec::new();
    if mine.is_some() {
        out.push(Line::from(vec![
            Span::styled(format!("{yes_mark} "), Style::default().fg(Color::LightGreen)),
            Span::raw("you like it too   "),
            Span::styled(format!("{no_mark} "), Style::default().fg(Color::LightRed)),
            Span::raw("one of you said no"),
        ]));
        out.push(Line::default());
    }
    let mw = if mine.is_some() { 2 } else { 0 };
    if width >= 100 {
        // Four columns side by side.
        let cw = (width as usize / 4).saturating_sub(1);
        let rows = cols.iter().map(Vec::len).max().unwrap_or(0);
        out.push(Line::from(
            titles
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    Span::styled(
                        format!("{:<cw$} ", format!("{} ({})", t, cols[i].iter().filter(|k| !k.0.starts_with("  ")).count())),
                        Style::default().fg(colors[i]).add_modifier(Modifier::BOLD),
                    )
                })
                .collect::<Vec<_>>(),
        ));
        for r in 0..rows {
            let spans: Vec<Span<'static>> = (0..4)
                .flat_map(|c| {
                    let (txt, custom, f) = cols[c].get(r).cloned().unwrap_or_default();
                    let st = if custom { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
                    let w = cw.saturating_sub(mw);
                    [mark(f), Span::styled(format!("{:<w$} ", trunc(&txt, w)), st)]
                })
                .collect();
            out.push(Line::from(spans));
        }
    } else {
        for (i, t) in titles.iter().enumerate() {
            out.push(Line::from(Span::styled(
                format!("{t} ({})", cols[i].len()),
                Style::default().fg(colors[i]).add_modifier(Modifier::BOLD),
            )));
            for (txt, custom, f) in &cols[i] {
                let st = if *custom { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
                out.push(Line::from(vec![Span::raw("  "), mark(*f), Span::styled(txt.clone(), st)]));
            }
            out.push(Line::default());
        }
    }
    if map.is_none() {
        out.push(Line::from(Span::styled("Loading kink names…", Style::default().fg(Color::DarkGray))));
    }
    out
}

fn guestbook_lines(gb: &Value, name_style: &dyn Fn(&str) -> Style) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let posts = gb.get("posts").and_then(Value::as_array).cloned().unwrap_or_default();
    if posts.is_empty() {
        out.push(Line::from("No guestbook posts."));
    }
    for p in posts {
        let who = p.get("character").map(|c| vs(c, "name")).unwrap_or_default();
        let at = vi(&p, "postedAt");
        out.push(Line::from(vec![
            Span::styled(who.clone(), name_style(&who)),
            Span::styled(
                format!("  {}", if at > 0 { timeutil::fmt_datetime(at) } else { String::new() }),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        out.extend(bbcode::to_lines(&vs(&p, "message"), Style::default(), name_style, true));
        let reply = vs(&p, "reply");
        if !reply.is_empty() && reply != "null" {
            out.push(Line::from(Span::styled("Reply:", Style::default().fg(Color::Cyan))));
            out.extend(bbcode::to_lines(&reply, Style::default().fg(Color::Gray), name_style, true));
        }
        out.push(Line::default());
    }
    if gb.get("nextPage").map(|n| n.as_bool().unwrap_or(false) || n.as_i64().unwrap_or(0) > 0).unwrap_or(false) {
        out.push(Line::from(Span::styled("More posts: press n for the next page", Style::default().fg(Color::DarkGray))));
    }
    out
}

impl App {
    pub(super) fn draw_profile(&mut self, buf: &mut Buffer, area: Rect, idx: usize) {
        let Dialog::Profile(p) = &self.dialogs[idx] else { return };
        let ascii = self.settings.ascii_borders;
        let outer = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), area.height.saturating_sub(2));
        let inner = widgets::popup(buf, outer, &format!("Profile: {}", p.name), ascii);
        self.hit.dialog = outer;
        // Tab bar.
        let mut x = inner.x;
        for (i, t) in TABS.iter().enumerate() {
            let label = format!(" {} {} ", i + 1, t);
            let st = if i == p.tab { Style::default().fg(Color::Black).bg(Color::Cyan) } else { Style::default().fg(Color::Gray) };
            buf.set_string(x, inner.y, &label, st);
            x += label.chars().count() as u16 + 1;
        }
        let hints: &[(&str, &str)] = if p.tab == 5 {
            &[("←→", "tabs"), ("n/N", "page"), ("w", "sign guestbook"), ("p", "message"), ("t", "note"), ("Esc", "close")]
        } else {
            &[("←→", "tabs"), ("p", "message"), ("t", "note"), ("m", "memo"), ("b", "bookmark"), ("o", "browser"), ("Esc", "close")]
        };
        widgets::draw_hints(buf, inner.x, inner.bottom() - 1, hints, inner.width);
        let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(3));
        let data = match &p.data {
            None => {
                buf.set_string(body.x, body.y, "Loading…", Style::default().fg(Color::DarkGray));
                return;
            }
            Some(Err(e)) => {
                buf.set_stringn(body.x, body.y, format!("Error: {e}"), body.width as usize, Style::default().fg(Color::LightRed));
                return;
            }
            Some(Ok(v)) => v.clone(),
        };
        let (tab, scroll, name) = (p.tab, p.scroll, p.name.clone());
        let map = self.mapping.clone();
        let ns = |n: &str| self.name_style(n);
        let lines: Vec<Line<'static>> = match tab {
            0 => {
                let status = self.chat.character(&name).map(|c| (c.status.label().to_string(), c.status_msg.clone()));
                let mut body_area = body;
                if self.settings.active_image_mode() != ImageMode::Off && body.width > 50 {
                    let img_w = 24.min(body.width / 3);
                    let url = crate::images::avatar_url(&name);
                    let (mode, proto, tc) = (self.settings.active_image_mode(), self.settings.graphics_protocol, self.settings.truecolor);
                    self.images.request(&url, &self.tx, super::AppEvent::Image);
                    self.images.draw(buf, Rect::new(body.x, body.y, img_w, (img_w / 2).min(body.height)), &url, mode, proto, tc);
                    body_area = Rect::new(body.x + img_w + 2, body.y, body.width - img_w - 2, body.height);
                }
                let mine = self.my_kinks.clone().filter(|_| !name.eq_ignore_ascii_case(&self.chat.me));
                let lines = widgets::wrap_all(&overview_lines(&data, map.as_deref(), status, mine.as_deref()), body_area.width as usize, 2);
                render_scrolled(buf, body_area, &lines, scroll);
                return;
            }
            1 => widgets::wrap_all(&bbcode::to_lines(&vs(&data, "description"), Style::default(), &ns, true), body.width as usize, 0),
            2 => {
                let mine = self.my_kinks.clone().filter(|m| !m.is_empty() && !name.eq_ignore_ascii_case(&self.chat.me));
                kink_lines(&data, map.as_deref(), body.width, mine.as_deref(), self.settings.ascii_borders)
            }
            3 => {
                let Dialog::Profile(p) = &self.dialogs[idx] else { return };
                let imgs = p.images();
                if imgs.is_empty() {
                    buf.set_string(body.x, body.y, "No images.", Style::default().fg(Color::DarkGray));
                    return;
                }
                let sel = p.image_sel.min(imgs.len() - 1);
                let list_w = (body.width / 3).clamp(20, 40);
                let items: Vec<Line<'static>> = imgs
                    .iter()
                    .enumerate()
                    .map(|(i, (_, d))| {
                        Line::from(format!("{}. {}", i + 1, if d.is_empty() { "(no description)".to_string() } else { bbcode::strip(d) }))
                    })
                    .collect();
                widgets::list(buf, Rect::new(body.x, body.y, list_w, body.height), &items, sel, true);
                let url = imgs[sel].0.clone();
                let view = Rect::new(body.x + list_w + 1, body.y, body.width.saturating_sub(list_w + 1), body.height);
                if self.settings.active_image_mode() == ImageMode::Off {
                    buf.set_string(view.x, view.y, "Images are off (/images blocks to enable).", Style::default().fg(Color::DarkGray));
                } else {
                    let (mode, proto, tc) = (self.settings.active_image_mode(), self.settings.graphics_protocol, self.settings.truecolor);
                    self.images.request(&url, &self.tx, super::AppEvent::Image);
                    self.images.draw(buf, view, &url, mode, proto, tc);
                }
                return;
            }
            4 => {
                let Dialog::Profile(p) = &self.dialogs[idx] else { return };
                match &p.friends {
                    None => vec![Line::from("Loading…")],
                    Some(Err(e)) => vec![Line::from(e.clone())],
                    Some(Ok(f)) if f.is_empty() => vec![Line::from("No friends listed.")],
                    Some(Ok(f)) => {
                        let items: Vec<Line<'static>> = f
                            .iter()
                            .map(|n| {
                                let st = self.chat.status_of(n);
                                Line::from(vec![
                                    Span::styled(format!("{} ", st.symbol()), Style::default().fg(st.color())),
                                    Span::styled(n.clone(), self.name_style(n)),
                                ])
                            })
                            .collect();
                        let sel = p.friend_sel;
                        let off = widgets::list(buf, body, &items, sel, true);
                        self.hit.dialog_list = body;
                        self.hit.dialog_list_offset = off;
                        return;
                    }
                }
            }
            6 => {
                let Dialog::Profile(p) = &self.dialogs[idx] else { return };
                match &p.groups {
                    None => vec![Line::from("Loading…")],
                    Some(Err(e)) => vec![Line::from(e.clone())],
                    Some(Ok(g)) if g.is_empty() => vec![Line::from("Not in any groups.")],
                    Some(Ok(g)) => {
                        let mut l =
                            vec![Line::from(Span::styled(format!("{} groups on F-List", g.len()), Style::default().fg(Color::DarkGray)))];
                        l.extend(g.iter().map(|n| Line::from(format!("• {n}"))));
                        l
                    }
                }
            }
            _ => {
                let Dialog::Profile(p) = &self.dialogs[idx] else { return };
                match &p.guestbook {
                    None => vec![Line::from("Loading…")],
                    Some((_, Err(e))) => vec![Line::from(e.clone())],
                    Some((page, Ok(gb))) => {
                        let mut l = vec![Line::from(Span::styled(format!("Page {}", page + 1), Style::default().fg(Color::DarkGray)))];
                        l.extend(widgets::wrap_all(&guestbook_lines(gb, &ns), body.width as usize, 0));
                        l
                    }
                }
            }
        };
        render_scrolled(buf, body, &lines, scroll);
    }

    pub(super) fn profile_key(&mut self, key: KeyEvent) {
        let Some(Dialog::Profile(p)) = self.dialogs.last_mut() else { return };
        let name = p.name.clone();
        let mut load_tab = None;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.dialogs.pop();
                self.full_redraw = true;
                return;
            }
            KeyCode::Right | KeyCode::Tab => {
                p.tab = (p.tab + 1) % TABS.len();
                p.scroll = 0;
                load_tab = Some(p.tab);
            }
            KeyCode::Left | KeyCode::BackTab => {
                p.tab = (p.tab + TABS.len() - 1) % TABS.len();
                p.scroll = 0;
                load_tab = Some(p.tab);
            }
            KeyCode::Char(c @ '1'..='7') => {
                p.tab = c as usize - '1' as usize;
                p.scroll = 0;
                load_tab = Some(p.tab);
            }
            KeyCode::Up | KeyCode::Char('k') => match p.tab {
                3 => p.image_sel = p.image_sel.saturating_sub(1),
                4 => p.friend_sel = p.friend_sel.saturating_sub(1),
                _ => p.scroll = p.scroll.saturating_sub(1),
            },
            KeyCode::Down | KeyCode::Char('j') => match p.tab {
                3 => p.image_sel = (p.image_sel + 1).min(p.images().len().saturating_sub(1)),
                4 => p.friend_sel = (p.friend_sel + 1).min(p.friend_list().len().saturating_sub(1)),
                _ => p.scroll += 1,
            },
            KeyCode::PageUp => p.scroll = p.scroll.saturating_sub(10),
            KeyCode::PageDown => p.scroll += 10,
            KeyCode::Home => p.scroll = 0,
            KeyCode::Enter => match p.tab {
                3 => {
                    let imgs = p.images();
                    if let Some((url, desc)) = imgs.get(p.image_sel).cloned() {
                        let t = if desc.is_empty() { format!("{name}'s image") } else { bbcode::strip(&desc) };
                        self.show_image(&t, &url);
                    }
                }
                4 => {
                    if let Some(f) = p.friend_list().get(p.friend_sel).cloned() {
                        self.open_user_menu(&f);
                    }
                }
                _ => {}
            },
            KeyCode::Char('n') if p.tab == 5 => {
                p.gb_page += 1;
                p.guestbook = None;
                load_tab = Some(5);
            }
            KeyCode::Char('N') if p.tab == 5 && p.gb_page > 0 => {
                p.gb_page -= 1;
                p.guestbook = None;
                load_tab = Some(5);
            }
            KeyCode::Char('w') if p.tab == 5 => return self.sign_guestbook(&name),
            KeyCode::Char('t') => return self.compose_note(&name),
            KeyCode::Char('p') => self.run_action(Action::OpenPrivate(name.clone())),
            KeyCode::Char('m') => self.open_memo(&name),
            KeyCode::Char('b') => self.run_action(Action::ToggleBookmark(name.clone())),
            KeyCode::Char('o') => {
                let url =
                    format!("{}/c/{}", self.settings.api_base, url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>());
                self.run_action(Action::OpenUrl(url));
            }
            KeyCode::Char('a') => self.open_user_menu(&name),
            _ => {}
        }
        if let Some(tab) = load_tab {
            self.full_redraw = true;
            let Some(Dialog::Profile(p)) = self.dialogs.last() else { return };
            match tab {
                4 if p.friends.is_none() => {
                    let n = name.clone();
                    self.api_call(move |api| ApiReply::CharFriends(n.clone(), api.character_friends(&n)));
                }
                6 if p.groups.is_none() => self.load_profile_groups(&name),
                5 if p.guestbook.is_none() => {
                    let (n, page) = (name.clone(), p.gb_page);
                    self.api_call(move |api| ApiReply::Guestbook(n.clone(), page, api.guestbook(&n, page)));
                }
                _ => {}
            }
        }
    }
}

fn render_scrolled(buf: &mut Buffer, area: Rect, lines: &[Line<'static>], scroll: usize) {
    let max = lines.len().saturating_sub(area.height as usize);
    let s = scroll.min(max);
    super::widgets::note_text(lines, s);
    for (i, l) in lines.iter().skip(s).take(area.height as usize).enumerate() {
        buf.set_line(area.x, area.y + i as u16, l, area.width);
    }
    if lines.len() > area.height as usize {
        let info = format!(" {}/{} ", (s + area.height as usize).min(lines.len()), lines.len());
        buf.set_string(area.right().saturating_sub(info.len() as u16), area.y, info, Style::default().fg(Color::DarkGray));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kink_matching() {
        assert_eq!(kink_fit("fave", Some("yes")), Some(true));
        assert_eq!(kink_fit("yes", Some("no")), Some(false));
        assert_eq!(kink_fit("no", Some("fave")), Some(false));
        assert_eq!(kink_fit("maybe", Some("no")), None);
        assert_eq!(kink_fit("no", Some("no")), None);
        assert_eq!(kink_fit("fave", None), None);
        let theirs = serde_json::json!({"kinks": {"1": "fave", "2": "yes", "3": "no", "4": "maybe"}});
        let mine: MyKinks =
            [("1", "fave"), ("2", "no"), ("3", "yes"), ("4", "fave")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(kink_match(&theirs, &mine), (1, 1, 2));
    }
}

//! Website features: "Browse New" (new/updated characters) and groups with
//! their forums, threads and posts. The network side is in `crate::site`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::actions::{Action, PromptKind};
use super::dialogs::{Dialog, MenuItem};
use super::input::{EditResult, LineEditor};
use super::site::{Refresh, SiteAction, SitePrompt, SiteReply};
use super::widgets::{self, str_width, trunc};
use super::App;
use crate::bbcode;
use crate::site::{BrowsePage, ForumPage, Group, GroupSummary, ThreadPage, BROWSE_PER_PAGE};
use crate::state::gender_color;

/// Something loading (`None`) or loaded.
type Loading<T> = Option<Result<T, String>>;

/// Whether an opened level is loading, failed or loaded.
fn unit<T>(r: &Option<Loading<T>>) -> Loading<()> {
    match r {
        Some(Some(Ok(_))) => Some(Ok(())),
        Some(Some(Err(e))) => Some(Err(e.clone())),
        _ => None,
    }
}

/// "Loading…" or the error in a dialog.
fn status(buf: &mut Buffer, area: Rect, r: &Loading<()>) {
    match r {
        None => buf.set_string(area.x + 1, area.y, "Loading…", Style::default().fg(Color::DarkGray)),
        Some(Err(e)) => {
            buf.set_stringn(area.x + 1, area.y, format!("Error: {e}"), area.width as usize, Style::default().fg(Color::LightRed));
        }
        Some(Ok(())) => {}
    }
}

pub struct BrowseState {
    /// Recently created instead of new/updated characters.
    pub newest: bool,
    pub offset: usize,
    pub data: Loading<BrowsePage>,
    pub sel: usize,
}

impl BrowseState {
    fn selected(&self) -> Option<&crate::site::BrowseCharacter> {
        match &self.data {
            Some(Ok(p)) => p.characters.get(self.sel),
            _ => None,
        }
    }
}

/// The group list, and the group, forum and thread opened from it.
#[derive(Default)]
pub struct GroupsState {
    pub list: Loading<Vec<GroupSummary>>,
    pub filter: LineEditor,
    pub sel: usize,
    pub group: Option<Loading<Group>>,
    pub group_slug: String,
    pub group_sel: usize,
    pub forum: Option<Loading<ForumPage>>,
    pub forum_id: i64,
    pub forum_sel: usize,
    pub thread: Option<Loading<ThreadPage>>,
    pub thread_id: i64,
    pub scroll: usize,
}

impl GroupsState {
    /// 0 list, 1 group, 2 forum, 3 thread.
    pub fn level(&self) -> usize {
        if self.thread.is_some() {
            3
        } else if self.forum.is_some() {
            2
        } else if self.group.is_some() {
            1
        } else {
            0
        }
    }

    pub fn filtered(&self) -> Vec<GroupSummary> {
        let Some(Ok(all)) = &self.list else { return Vec::new() };
        let words: Vec<String> = self.filter.text().to_lowercase().split_whitespace().map(String::from).collect();
        all.iter()
            .filter(|g| {
                let hay = format!("{} {} {}", g.name, g.owner, g.description).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .cloned()
            .collect()
    }

    fn loaded_group(&self) -> Option<&Group> {
        self.group.as_ref()?.as_ref()?.as_ref().ok()
    }

    fn loaded_forum(&self) -> Option<&ForumPage> {
        self.forum.as_ref()?.as_ref()?.as_ref().ok()
    }

    fn loaded_thread(&self) -> Option<&ThreadPage> {
        self.thread.as_ref()?.as_ref()?.as_ref().ok()
    }

    /// Select a list row (mouse). Returns true when it should open.
    pub fn select_row(&mut self, row: usize) -> bool {
        match self.level() {
            0 => self.sel = row,
            1 => self.group_sel = row,
            2 => self.forum_sel = row,
            _ => return false,
        }
        true
    }
}

fn group_url(base: &str, slug: &str) -> String {
    format!("{base}/group.php?group={}", crate::site::urlenc(slug))
}

impl App {
    // ------------------------------------------------------------------
    // Opening and loading

    pub(super) fn open_browse(&mut self, newest: bool) {
        self.dialogs.retain(|d| !matches!(d, Dialog::Browse(_)));
        self.dialogs.push(Dialog::Browse(Box::new(BrowseState { newest, offset: 0, data: None, sel: 0 })));
        self.load_browse(newest, 0);
    }

    fn load_browse(&mut self, newest: bool, offset: usize) {
        self.site_call(move |api| SiteReply::Browse { newest, offset, result: api.site.browse(newest, offset) });
    }

    /// The group list, or with a name that group's page.
    pub(super) fn open_groups(&mut self, name: &str) {
        self.dialogs.retain(|d| !matches!(d, Dialog::Groups(_)));
        self.dialogs.push(Dialog::Groups(Box::default()));
        if name.trim().is_empty() {
            self.load_group_list();
        } else {
            self.open_group(name.trim());
        }
    }

    fn groups_state(&mut self) -> Option<&mut GroupsState> {
        self.dialogs.iter_mut().rev().find_map(|d| match d {
            Dialog::Groups(g) => Some(g.as_mut()),
            _ => None,
        })
    }

    fn load_group_list(&mut self) {
        if let Some(g) = self.groups_state() {
            g.list = None;
        }
        self.site_call(|api| SiteReply::GroupList(api.site.groups()));
    }

    fn open_group(&mut self, slug: &str) {
        if let Some(g) = self.groups_state() {
            g.group = Some(None);
            g.group_slug = slug.to_string();
            g.group_sel = 0;
        }
        self.reload_group(slug);
    }

    fn reload_group(&mut self, slug: &str) {
        let s = slug.to_string();
        self.site_call(move |api| {
            let r = api.site.group(&s);
            SiteReply::Group(s, r)
        });
    }

    fn open_forum(&mut self, id: i64) {
        if let Some(g) = self.groups_state() {
            g.forum = Some(None);
            g.forum_id = id;
            g.forum_sel = 0;
        }
        self.reload_forum(id);
    }

    fn reload_forum(&mut self, id: i64) {
        self.site_call(move |api| SiteReply::Forum(id, api.site.forum(id)));
    }

    /// `offset` None opens the last page.
    fn open_thread(&mut self, id: i64, offset: Option<usize>) {
        if let Some(g) = self.groups_state() {
            g.thread = Some(None);
            g.thread_id = id;
            g.scroll = if offset.is_none() { usize::MAX / 2 } else { 0 };
        }
        self.site_call(move |api| SiteReply::Thread(id, api.site.thread(id, offset)));
    }

    // ------------------------------------------------------------------
    // Replies

    pub(super) fn on_groups_reply(&mut self, r: SiteReply) {
        match r {
            SiteReply::Browse { newest, offset, result } => {
                for d in self.dialogs.iter_mut() {
                    if let Dialog::Browse(b) = d {
                        if b.newest == newest && b.offset == offset {
                            if let Ok(p) = &result {
                                b.sel = b.sel.min(p.characters.len().saturating_sub(1));
                            }
                            b.data = Some(result.clone());
                        }
                    }
                }
            }
            SiteReply::GroupList(r) => {
                if let Some(g) = self.groups_state() {
                    g.list = Some(r);
                }
            }
            SiteReply::Group(slug, r) => {
                if let Some(g) = self.groups_state() {
                    if g.group.is_some() && g.group_slug == slug {
                        if let Ok(gr) = &r {
                            g.group_sel = g.group_sel.min(gr.forums.len().saturating_sub(1));
                        }
                        g.group = Some(Some(r));
                    }
                }
            }
            SiteReply::Forum(id, r) => {
                if let Some(g) = self.groups_state() {
                    if g.forum.is_some() && g.forum_id == id {
                        if let Ok(f) = &r {
                            g.forum_sel = g.forum_sel.min(f.threads.len().saturating_sub(1));
                            // Opened directly: know the group too.
                            if g.group.is_none() || g.group_slug.is_empty() {
                                g.group_slug = f.group_slug.clone();
                            }
                        }
                        g.forum = Some(Some(r));
                    }
                }
            }
            SiteReply::Thread(id, r) => {
                if let Some(g) = self.groups_state() {
                    if g.thread.is_some() && g.thread_id == id {
                        g.thread = Some(Some(r));
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn groups_refresh(&mut self, refresh: Refresh) {
        match refresh {
            Refresh::Group(slug) => self.reload_group(&slug),
            Refresh::Forum(id) => self.reload_forum(id),
            Refresh::ThreadLast(id) => self.open_thread(id, None),
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Prompts and menu actions

    /// Write a reply to the open thread.
    fn reply_to_thread(&mut self) {
        let Some(t) = self.groups_state().and_then(|g| g.loaded_thread().cloned()) else { return };
        if !t.can_reply {
            return self.flash("Replies can't be posted to this thread.");
        }
        let me = self.chat.me.clone();
        self.prompt(&format!("Reply to \"{}\" as {me}", t.subject), "", PromptKind::Site(SitePrompt::GroupReply), true);
        self.set_prompt_extra(&format!("{} {} {}", t.group_id, t.forum_id, t.id));
    }

    fn new_thread(&mut self) {
        let Some(f) = self.groups_state().and_then(|g| g.loaded_forum().cloned()) else { return };
        if !f.can_post || f.group_id == 0 {
            return self.flash("You can't start threads in this forum.");
        }
        self.prompt(&format!("New thread in {}: subject", f.name), "", PromptKind::Site(SitePrompt::ThreadSubject), false);
        self.set_prompt_extra(&format!("{} {}", f.group_id, f.id));
    }

    pub(super) fn on_groups_prompt(&mut self, kind: SitePrompt, value: String, extra: String) {
        let v = value.trim().to_string();
        let ids: Vec<i64> = extra.split('\n').next().unwrap_or("").split(' ').filter_map(|x| x.parse().ok()).collect();
        let me = self.chat.me.clone();
        match (kind, ids.as_slice()) {
            (SitePrompt::GroupReply, &[group, forum, thread]) => {
                if v.is_empty() {
                    return self.flash("Empty reply not posted.");
                }
                self.site_do("Reply posted.", Refresh::ThreadLast(thread), move |api| {
                    let c = api.character_id(&me)?;
                    api.site.post_reply(group, forum, thread, c, &value)
                });
            }
            (SitePrompt::ThreadSubject, &[_, _]) => {
                if v.is_empty() {
                    return self.flash("A thread needs a subject.");
                }
                self.prompt(&format!("New thread \"{v}\" as {me}"), "", PromptKind::Site(SitePrompt::ThreadBody), true);
                self.set_prompt_extra(&format!("{extra}\n{v}"));
            }
            (SitePrompt::ThreadBody, &[group, forum]) => {
                if v.is_empty() {
                    return self.flash("Empty thread not posted.");
                }
                let subject = extra.split_once('\n').map(|x| x.1.to_string()).unwrap_or_default();
                self.site_do("Thread posted.", Refresh::Forum(forum), move |api| {
                    let c = api.character_id(&me)?;
                    api.site.post_thread(group, forum, c, &subject, &value)
                });
            }
            _ => {}
        }
    }

    pub(super) fn run_groups_action(&mut self, a: SiteAction) {
        if let SiteAction::JoinGroup { group_id, character_id, character, slug, join } = a {
            let what = if join { format!("{character} joined the group.") } else { format!("{character} left the group.") };
            self.site_do(&what, Refresh::Group(slug), move |api| api.site.group_join(group_id, character_id, join));
        }
    }

    /// Menu of your characters that can join or leave the open group.
    fn join_menu(&mut self, join: bool) {
        let Some(g) = self.groups_state().and_then(|g| g.loaded_group().cloned()) else { return };
        let chars = if join { &g.not_joined } else { &g.joined };
        if chars.is_empty() {
            return self.flash(if join { "All your characters are in this group." } else { "None of your characters is in this group." });
        }
        let items = chars
            .iter()
            .map(|(id, name)| {
                MenuItem::new(
                    name,
                    Action::Site(SiteAction::JoinGroup {
                        group_id: g.id,
                        character_id: *id,
                        character: name.clone(),
                        slug: g.slug.clone(),
                        join,
                    }),
                )
            })
            .collect();
        let title = if join { format!("Join {} as", g.name) } else { format!("Leave {} with", g.name) };
        self.dialogs.push(Dialog::menu(&title, items));
    }

    // ------------------------------------------------------------------
    // Keys

    pub(super) fn browse_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let base = self.settings.api_base.clone();
        let Some(Dialog::Browse(b)) = self.dialogs.last_mut() else { return };
        let (count, next) = match &b.data {
            Some(Ok(p)) => (p.characters.len(), p.next),
            _ => (0, None),
        };
        let mut load: Option<(bool, usize)> = None;
        match key.code {
            KeyCode::Esc => {
                self.dialogs.pop();
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => b.sel = b.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => b.sel = (b.sel + 1).min(count.saturating_sub(1)),
            KeyCode::Home => b.sel = 0,
            KeyCode::End => b.sel = count.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(']') => {
                if let Some(n) = next {
                    load = Some((b.newest, n));
                }
            }
            KeyCode::PageUp | KeyCode::Char('[') if b.offset > 0 => load = Some((b.newest, b.offset.saturating_sub(BROWSE_PER_PAGE))),
            KeyCode::Tab | KeyCode::BackTab => load = Some((!b.newest, 0)),
            KeyCode::Char('r') if ctrl => load = Some((b.newest, b.offset)),
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(c) = b.selected() {
                    let n = c.name.clone();
                    self.open_user_menu(&n);
                }
                return;
            }
            KeyCode::Char('p') => {
                if let Some(c) = b.selected() {
                    let n = c.name.clone();
                    self.open_profile(&n);
                }
                return;
            }
            KeyCode::Char('o') => {
                let url = format!("{base}/character_browse.php?type={}&offset={}", if b.newest { "newest" } else { "edited" }, b.offset);
                return self.run_action(Action::OpenUrl(url));
            }
            _ => {}
        }
        if let Some((newest, offset)) = load {
            if newest != b.newest || offset != b.offset {
                b.sel = 0;
            }
            b.newest = newest;
            b.offset = offset;
            b.data = None;
            self.load_browse(newest, offset);
        }
    }

    pub(super) fn groups_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let base = self.settings.api_base.clone();
        let Some(Dialog::Groups(g)) = self.dialogs.last_mut() else { return };
        let page = 10;
        match g.level() {
            0 => {
                let list = g.filtered();
                let n = list.len();
                match key.code {
                    KeyCode::Esc if !g.filter.is_empty() => g.filter.clear(),
                    KeyCode::Esc => {
                        self.dialogs.pop();
                    }
                    KeyCode::Up => g.sel = g.sel.saturating_sub(1),
                    KeyCode::Down => g.sel = (g.sel + 1).min(n.saturating_sub(1)),
                    KeyCode::PageUp => g.sel = g.sel.saturating_sub(page),
                    KeyCode::PageDown => g.sel = (g.sel + page).min(n.saturating_sub(1)),
                    KeyCode::Home if g.filter.is_empty() => g.sel = 0,
                    KeyCode::End if g.filter.is_empty() => g.sel = n.saturating_sub(1),
                    KeyCode::Char('r') if ctrl => self.load_group_list(),
                    KeyCode::Enter => {
                        if let Some(x) = list.get(g.sel) {
                            let slug = x.slug.clone();
                            self.open_group(&slug);
                        }
                    }
                    _ => {
                        if let EditResult::Changed = g.filter.handle(key) {
                            g.sel = 0;
                        }
                    }
                }
            }
            1 => {
                let n = g.loaded_group().map(|x| x.forums.len()).unwrap_or(0);
                match key.code {
                    KeyCode::Esc | KeyCode::Backspace => {
                        g.group = None;
                        if g.list.is_none() {
                            self.load_group_list();
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => g.group_sel = g.group_sel.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => g.group_sel = (g.group_sel + 1).min(n.saturating_sub(1)),
                    KeyCode::Enter => {
                        if let Some(f) = g.loaded_group().and_then(|x| x.forums.get(g.group_sel)) {
                            let id = f.id;
                            self.open_forum(id);
                        }
                    }
                    KeyCode::Char('r') if ctrl => {
                        let slug = g.group_slug.clone();
                        self.reload_group(&slug);
                    }
                    KeyCode::Char('J') | KeyCode::Char('+') => self.join_menu(true),
                    KeyCode::Char('L') | KeyCode::Char('-') => self.join_menu(false),
                    KeyCode::Char('o') => {
                        let url = group_url(&base, &g.group_slug);
                        self.run_action(Action::OpenUrl(url));
                    }
                    _ => {}
                }
            }
            2 => {
                let n = g.loaded_forum().map(|x| x.threads.len()).unwrap_or(0);
                match key.code {
                    KeyCode::Esc | KeyCode::Backspace => {
                        g.forum = None;
                        // Opened without the group: load it now.
                        if g.group.is_none() && !g.group_slug.is_empty() {
                            let slug = g.group_slug.clone();
                            self.open_group(&slug);
                        } else if g.group.is_none() && g.list.is_none() {
                            self.load_group_list();
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => g.forum_sel = g.forum_sel.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => g.forum_sel = (g.forum_sel + 1).min(n.saturating_sub(1)),
                    KeyCode::PageUp => g.forum_sel = g.forum_sel.saturating_sub(page),
                    KeyCode::PageDown => g.forum_sel = (g.forum_sel + page).min(n.saturating_sub(1)),
                    KeyCode::Home => g.forum_sel = 0,
                    KeyCode::End => g.forum_sel = n.saturating_sub(1),
                    KeyCode::Enter | KeyCode::Char('e') => {
                        if let Some(t) = g.loaded_forum().and_then(|x| x.threads.get(g.forum_sel)) {
                            let id = t.id;
                            // Enter: from the start; e: the newest posts.
                            let offset = if key.code == KeyCode::Enter { Some(0) } else { None };
                            self.open_thread(id, offset);
                        }
                    }
                    KeyCode::Char('n') => self.new_thread(),
                    KeyCode::Char('r') if ctrl => {
                        let id = g.forum_id;
                        self.reload_forum(id);
                    }
                    KeyCode::Char('o') => {
                        let url = format!("{base}/forum.php?forum={}", g.forum_id);
                        self.run_action(Action::OpenUrl(url));
                    }
                    _ => {}
                }
            }
            _ => {
                let t = g.loaded_thread().cloned();
                let id = g.thread_id;
                let pos = t.as_ref().and_then(|t| t.pages.iter().position(|p| *p == t.offset)).unwrap_or(0);
                let goto = |pages: &[usize], i: usize| pages.get(i).copied();
                let mut open: Option<Option<usize>> = None;
                match key.code {
                    KeyCode::Esc | KeyCode::Backspace => {
                        g.thread = None;
                        if g.forum.is_none() {
                            // Opened without its forum: go back to the forum.
                            match t {
                                Some(t) if t.forum_id > 0 => {
                                    if g.group_slug.is_empty() {
                                        g.group_slug = t.group_slug.clone();
                                    }
                                    self.open_forum(t.forum_id);
                                }
                                _ if g.group.is_none() && g.list.is_none() => self.load_group_list(),
                                _ => {}
                            }
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => g.scroll = g.scroll.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => g.scroll += 1,
                    KeyCode::PageUp => g.scroll = g.scroll.saturating_sub(15),
                    KeyCode::PageDown | KeyCode::Char(' ') => g.scroll += 15,
                    KeyCode::Home => g.scroll = 0,
                    KeyCode::End => g.scroll = usize::MAX / 2,
                    KeyCode::Char('[') if pos > 0 => open = t.as_ref().and_then(|t| goto(&t.pages, pos - 1)).map(Some),
                    KeyCode::Char(']') => open = t.as_ref().and_then(|t| goto(&t.pages, pos + 1)).map(Some),
                    KeyCode::Char('{') => open = t.as_ref().and_then(|t| t.pages.first().copied()).map(Some),
                    KeyCode::Char('}') => open = Some(None),
                    KeyCode::Char('r') if ctrl => open = Some(t.as_ref().map(|t| t.offset)),
                    KeyCode::Char('r') => return self.reply_to_thread(),
                    KeyCode::Char('o') => {
                        let off = t.as_ref().map(|t| t.offset).unwrap_or(0);
                        self.run_action(Action::OpenUrl(format!("{base}/thread.php?thread={id}&offset={off}")));
                    }
                    _ => {}
                }
                if let Some(offset) = open {
                    self.open_thread(id, offset);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Drawing

    /// Returns (dialog area, list area, list offset).
    pub(super) fn draw_browse(&mut self, buf: &mut Buffer, big: Rect, idx: usize, top: bool) -> (Rect, Rect, usize) {
        let ascii = self.settings.ascii_borders;
        let Dialog::Browse(b) = &self.dialogs[idx] else { return (big, Rect::default(), 0) };
        let page = b.offset / BROWSE_PER_PAGE + 1;
        let title = if b.newest {
            format!("Recently created characters · page {page}")
        } else {
            format!("New and updated characters · page {page}")
        };
        let inner = widgets::popup(buf, big, &title, ascii);
        let body = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
        let hints = [
            ("Enter", "actions"),
            ("p", "profile"),
            ("Tab", if b.newest { "new/updated" } else { "recently created" }),
            ("PgUp/PgDn", "page"),
            ("o", "browser"),
            ("Esc", "close"),
        ];
        widgets::draw_hints(buf, inner.x, inner.bottom().saturating_sub(1), &hints, inner.width);
        let page = match &b.data {
            None => {
                buf.set_string(body.x + 1, body.y, "Loading…", Style::default().fg(Color::DarkGray));
                return (big, Rect::default(), 0);
            }
            Some(Err(e)) => {
                buf.set_stringn(body.x + 1, body.y, format!("Error: {e}"), body.width as usize, Style::default().fg(Color::LightRed));
                return (big, Rect::default(), 0);
            }
            Some(Ok(p)) => p.clone(),
        };
        let sel = b.sel;
        if page.characters.is_empty() {
            buf.set_string(body.x + 1, body.y, "No characters on this page.", Style::default().fg(Color::DarkGray));
            return (big, Rect::default(), 0);
        }
        let gender = |c: &crate::site::BrowseCharacter| c.info.iter().find(|i| i.0 == "Gender").map(|i| i.1.clone()).unwrap_or_default();
        let left_w = (body.width * 2 / 5).clamp(20, 40).min(body.width);
        let left = Rect::new(body.x, body.y, left_w, body.height);
        let items: Vec<Line<'static>> = page
            .characters
            .iter()
            .map(|c| {
                let g = gender(c);
                let st = if g.is_empty() { Style::default() } else { Style::default().fg(gender_color(&g)) };
                let online = self.chat.is_online(&c.name);
                Line::from(vec![
                    Span::styled(if online { "● " } else { "  " }, Style::default().fg(Color::LightGreen)),
                    Span::styled(trunc(&c.name, left_w as usize - 4), st),
                ])
            })
            .collect();
        let off = widgets::list(buf, left, &items, sel, top);
        let right = Rect::new(body.x + left_w + 1, body.y, body.width.saturating_sub(left_w + 1), body.height);
        if let Some(c) = page.characters.get(sel.min(page.characters.len() - 1)) {
            let g = gender(c);
            self.draw_character_card(buf, right, &c.name, &g, &c.info, Vec::new());
        }
        (big, left, off)
    }

    /// A character's details: name, extra lines, avatar and info fields.
    pub(super) fn draw_character_card(
        &mut self,
        buf: &mut Buffer,
        right: Rect,
        name: &str,
        gender: &str,
        info: &[(String, String)],
        extra: Vec<Line<'static>>,
    ) {
        let mut y = right.y;
        let st = if gender.is_empty() { self.name_style(name) } else { self.name_style(name).fg(gender_color(gender)) };
        buf.set_stringn(right.x, y, name, right.width as usize, st);
        y += 1;
        for l in widgets::wrap_all(&extra, right.width as usize, 0) {
            if y >= right.bottom() {
                return;
            }
            buf.set_line(right.x, y, &l, right.width);
            y += 1;
        }
        let mode = self.settings.active_image_mode();
        if mode != crate::config::ImageMode::Off && right.height > 12 {
            let url = crate::images::avatar_url(name);
            self.request_image(&url);
            let h = (right.height / 2).clamp(4, 10).min(right.bottom().saturating_sub(y + 1));
            let (proto, tc) = (self.settings.graphics_protocol, self.settings.truecolor);
            let (_, used) = self.images.draw(buf, Rect::new(right.x, y + 1, (h * 2).min(right.width), h), &url, mode, proto, tc);
            y += used.max(1) + 2;
        } else {
            y += 1;
        }
        let kw = info.iter().map(|i| str_width(&i.0)).max().unwrap_or(0).min(16);
        for (k, v) in info {
            if y >= right.bottom() {
                break;
            }
            let line =
                Line::from(vec![Span::styled(format!("{:<kw$}  ", trunc(k, kw)), Style::default().fg(Color::Gray)), Span::raw(v.clone())]);
            buf.set_line(right.x, y, &line, right.width);
            y += 1;
        }
        if info.is_empty() && y < right.bottom() {
            buf.set_string(right.x, y, "No details listed.", Style::default().fg(Color::DarkGray));
        }
    }

    /// Profile data for a character's details, loaded once (None while loading).
    pub(super) fn char_card(&mut self, name: &str) -> Option<Result<std::sync::Arc<serde_json::Value>, String>> {
        let key = name.to_lowercase();
        if let Some(c) = self.char_cards.get(&key) {
            return c.clone();
        }
        self.char_cards.insert(key, None);
        self.ensure_mapping();
        let n = name.to_string();
        self.api_call(move |api| super::ApiReply::CharCard(n.clone(), api.character_data(&n)));
        None
    }

    /// Returns (dialog area, list area, list offset).
    pub(super) fn draw_groups(&mut self, buf: &mut Buffer, big: Rect, idx: usize, top: bool) -> (Rect, Rect, usize) {
        let ascii = self.settings.ascii_borders;
        let ns = |n: &str| self.name_style(n);
        let eicons = self.settings.show_eicon_names;
        let Dialog::Groups(g) = &self.dialogs[idx] else { return (big, Rect::default(), 0) };
        let level = g.level();
        let gray = Style::default().fg(Color::Gray);
        let dim = Style::default().fg(Color::DarkGray);
        match level {
            0 => {
                let list = g.filtered();
                let total = match &g.list {
                    Some(Ok(l)) => l.len(),
                    _ => 0,
                };
                let inner = widgets::popup(buf, big, &format!("Groups · {} of {total}", list.len()), ascii);
                buf.set_string(inner.x, inner.y, "Filter: ", gray);
                buf.set_string(inner.x + 8, inner.y, g.filter.text(), Style::default().fg(Color::Yellow));
                let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(3));
                let hints = [("type", "filter"), ("Enter", "open"), ("Ctrl+R", "refresh"), ("Esc", "close")];
                widgets::draw_hints(buf, inner.x, inner.bottom().saturating_sub(1), &hints, inner.width);
                let state: Loading<()> = match &g.list {
                    None => None,
                    Some(Err(e)) => Some(Err(e.clone())),
                    Some(Ok(_)) => Some(Ok(())),
                };
                if state != Some(Ok(())) {
                    status(buf, body, &state);
                    return (big, Rect::default(), 0);
                }
                let list_h = (body.height / 2).max(3);
                let list_area = Rect::new(body.x, body.y, body.width, list_h);
                let items: Vec<Line<'static>> = list
                    .iter()
                    .map(|x| {
                        Line::from(vec![
                            Span::styled(if x.joined { "★ " } else { "  " }, Style::default().fg(Color::Yellow)),
                            Span::styled(trunc(&x.name, 40), Style::default().add_modifier(Modifier::BOLD)),
                            Span::styled(format!("  {} posts · {} members", x.posts, x.members), dim),
                        ])
                    })
                    .collect();
                let off = widgets::list(buf, list_area, &items, g.sel, top);
                if items.is_empty() {
                    buf.set_string(body.x + 1, body.y, "Nothing matches.", dim);
                }
                if let Some(x) = list.get(g.sel.min(list.len().saturating_sub(1))) {
                    let view =
                        Rect::new(body.x + 1, body.y + list_h + 1, body.width.saturating_sub(1), body.height.saturating_sub(list_h + 1));
                    let mut lines = vec![Line::from(vec![
                        Span::styled(x.name.clone(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                        Span::styled(
                            format!(
                                "{}{}",
                                if x.owner.is_empty() { String::new() } else { format!("  owner {}", x.owner) },
                                if x.last_post.is_empty() { String::new() } else { format!(" · last post {}", x.last_post) }
                            ),
                            dim,
                        ),
                    ])];
                    lines.extend(bbcode::to_lines(&x.description, Style::default(), &ns, eicons));
                    for (i, l) in widgets::wrap_all(&lines, view.width as usize, 0).iter().take(view.height as usize).enumerate() {
                        buf.set_line(view.x, view.y + i as u16, l, view.width);
                    }
                }
                (big, list_area, off)
            }
            1 => {
                let title = match g.loaded_group() {
                    Some(gr) => format!("Group: {}", gr.name),
                    None => format!("Group: {}", g.group_slug),
                };
                let inner = widgets::popup(buf, big, &title, ascii);
                let body = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
                let hints = [("Enter", "open forum"), ("J", "join"), ("L", "leave"), ("o", "browser"), ("Esc", "back")];
                widgets::draw_hints(buf, inner.x, inner.bottom().saturating_sub(1), &hints, inner.width);
                let Some(gr) = g.loaded_group() else {
                    status(buf, body, &unit(&g.group));
                    return (big, Rect::default(), 0);
                };
                // Description (a few lines), who of you is a member, then the forums.
                let mut head = widgets::wrap_all(&bbcode::to_lines(&gr.description, gray, &ns, eicons), body.width as usize - 1, 0);
                head.truncate((body.height / 4).max(1) as usize);
                let members: Vec<&str> = gr.joined.iter().map(|x| x.1.as_str()).collect();
                head.push(Line::from(Span::styled(
                    if members.is_empty() {
                        "None of your characters is a member.".to_string()
                    } else {
                        format!("Members: {}", members.join(", "))
                    },
                    Style::default().fg(Color::Yellow),
                )));
                for (i, l) in head.iter().enumerate() {
                    buf.set_line(body.x + 1, body.y + i as u16, l, body.width.saturating_sub(1));
                }
                let top_h = head.len() as u16 + 1;
                let rest = Rect::new(body.x, body.y + top_h, body.width, body.height.saturating_sub(top_h));
                let list_h = (rest.height / 2).max(3).min(gr.forums.len().max(1) as u16 + 1).min(rest.height);
                let list_area = Rect::new(rest.x, rest.y, rest.width, list_h);
                let items: Vec<Line<'static>> = gr
                    .forums
                    .iter()
                    .map(|f| {
                        Line::from(vec![
                            Span::styled(f.name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                            Span::styled(format!("  {}", f.stats), dim),
                        ])
                    })
                    .collect();
                let off = widgets::list(buf, list_area, &items, g.group_sel, top);
                if items.is_empty() {
                    buf.set_string(rest.x + 1, rest.y, "No forums you can see.", dim);
                }
                if let Some(f) = gr.forums.get(g.group_sel) {
                    let view =
                        Rect::new(rest.x + 1, rest.y + list_h + 1, rest.width.saturating_sub(1), rest.height.saturating_sub(list_h + 1));
                    let mut lines = bbcode::to_lines(&f.description, Style::default(), &ns, eicons);
                    if !f.last.is_empty() {
                        lines.push(Line::from(Span::styled(f.last.clone(), dim)));
                    }
                    for (i, l) in widgets::wrap_all(&lines, view.width as usize, 0).iter().take(view.height as usize).enumerate() {
                        buf.set_line(view.x, view.y + i as u16, l, view.width);
                    }
                }
                (big, list_area, off)
            }
            2 => {
                let title = match g.loaded_forum() {
                    Some(f) if !f.group_name.is_empty() => format!("{} › {}", f.group_name, f.name),
                    Some(f) => f.name.clone(),
                    None => "Forum".to_string(),
                };
                let inner = widgets::popup(buf, big, &title, ascii);
                let body = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
                let hints = [("Enter", "read"), ("e", "newest posts"), ("n", "new thread"), ("o", "browser"), ("Esc", "back")];
                widgets::draw_hints(buf, inner.x, inner.bottom().saturating_sub(1), &hints, inner.width);
                let Some(f) = g.loaded_forum() else {
                    status(buf, body, &unit(&g.forum));
                    return (big, Rect::default(), 0);
                };
                let pw = f.threads.iter().map(|t| t.posts.to_string().len()).max().unwrap_or(1);
                let items: Vec<Line<'static>> = f
                    .threads
                    .iter()
                    .map(|t| {
                        let mut s = vec![
                            Span::styled(format!("{:>pw$} ", t.posts), dim),
                            Span::styled(t.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
                        ];
                        if t.locked {
                            s.push(Span::styled(" [locked]", Style::default().fg(Color::LightRed)));
                        }
                        if t.poll {
                            s.push(Span::styled(" [poll]", Style::default().fg(Color::LightCyan)));
                        }
                        if !t.author.is_empty() {
                            s.push(Span::styled(" by ", dim));
                            s.push(Span::styled(t.author.clone(), self.name_style(&t.author).remove_modifier(Modifier::BOLD)));
                        }
                        s.push(Span::styled(format!(" · last post {}", t.last_post), dim));
                        Line::from(s)
                    })
                    .collect();
                let off = widgets::list(buf, body, &items, g.forum_sel, top);
                if items.is_empty() {
                    buf.set_string(body.x + 1, body.y, "No threads yet.", dim);
                }
                (big, body, off)
            }
            _ => {
                let title = match g.loaded_thread() {
                    Some(t) => {
                        let pos = t.pages.iter().position(|p| *p == t.offset).unwrap_or(0) + 1;
                        format!("{} · page {pos}/{}", t.subject, t.pages.len().max(1))
                    }
                    None => "Thread".to_string(),
                };
                let inner = widgets::popup(buf, big, &title, ascii);
                let body = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(1), inner.height.saturating_sub(1));
                let hints =
                    [("↑↓ PgUp PgDn", "scroll"), ("[ ]", "page"), ("{ }", "first/last"), ("r", "reply"), ("o", "browser"), ("Esc", "back")];
                widgets::draw_hints(buf, inner.x, inner.bottom().saturating_sub(1), &hints, inner.width);
                let Some(t) = g.loaded_thread() else {
                    status(buf, body, &unit(&g.thread));
                    return (big, Rect::default(), 0);
                };
                let rule = if ascii { "-" } else { "─" };
                let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(format!("{} › {}", t.group_name, t.forum_name), dim))];
                for p in &t.posts {
                    lines.push(Line::from(Span::styled(rule.repeat(body.width as usize), dim)));
                    lines.push(Line::from(vec![
                        Span::styled(p.author.clone(), self.name_style(&p.author)),
                        Span::styled(format!(" · {}", p.posted), dim),
                    ]));
                    lines.extend(bbcode::to_lines(&p.body, Style::default(), &ns, eicons));
                }
                if t.posts.is_empty() {
                    lines.push(Line::from(Span::styled("No posts.", dim)));
                }
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(
                    if t.can_reply { "r: write a reply" } else { "This thread takes no replies (locked or not allowed)." },
                    dim,
                )));
                let lines = widgets::wrap_all(&lines, body.width as usize, 0);
                let max = lines.len().saturating_sub(body.height as usize);
                let s = g.scroll.min(max);
                widgets::note_text(&lines, s);
                for (i, l) in lines.iter().skip(s).take(body.height as usize).enumerate() {
                    buf.set_line(body.x, body.y + i as u16, l, body.width);
                }
                if let Some(Dialog::Groups(g)) = self.dialogs.get_mut(idx) {
                    g.scroll = s;
                }
                (big, Rect::default(), 0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_filter() {
        let mut g = GroupsState::default();
        assert_eq!(g.level(), 0);
        g.list = Some(Ok(vec![
            GroupSummary { slug: "a".into(), name: "Alpha Wolves".into(), ..Default::default() },
            GroupSummary { slug: "b".into(), name: "Beta".into(), description: "wolves too".into(), ..Default::default() },
            GroupSummary { slug: "c".into(), name: "Gamma".into(), ..Default::default() },
        ]));
        g.filter.set("wolves");
        assert_eq!(g.filtered().iter().map(|x| x.slug.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        g.group = Some(None);
        assert_eq!(g.level(), 1);
        g.thread = Some(None);
        assert_eq!(g.level(), 3);
        assert!(!g.select_row(2));
    }
}

//! Website features in the UI: notes, the message center, news, roleplay
//! ads, profile groups, guestbook posts and the unread counters in the
//! status bar. The network side is in `crate::site`.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::actions::{Action, PromptKind};
use super::dialogs::{Dialog, MenuItem};
use super::input::{EditResult, LineEditor};
use super::widgets::{self, trunc};
use super::{ApiReply, App};
use crate::bbcode;
use crate::site::{Counters, MessageCenter, NewsPost, Note, NoteFolder, NoteList, NoteSummary, Personal};

/// Notes per page.
const PAGE: usize = 20;
/// News posts per page (the website's own step).
const NEWS_PAGE: usize = 2;

pub enum SiteReply {
    Folders(Result<Vec<NoteFolder>, String>),
    Notes { list: NoteList, page: usize, result: Result<(Vec<NoteSummary>, i64), String> },
    Note(Result<Note, String>),
    Counters(Result<Counters, String>),
    Messages(Result<MessageCenter, String>),
    News(usize, Result<(Vec<NewsPost>, i64), String>),
    Personals(Result<Vec<Personal>, String>),
    Groups(String, Result<Vec<String>, String>),
    Done { what: String, result: Result<(), String>, refresh: Refresh },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refresh {
    Nothing,
    Notes,
    /// Back from the note view to the list, then refresh.
    NotesList,
    Messages,
    Guestbook(String),
}

/// Prompts used by the website features. The prompt's `extra` holds the context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitePrompt {
    /// Who to send a note to.
    NoteTo,
    /// extra: receiver
    NoteTitle,
    /// extra: "receiver\ntitle"
    NoteBody,
    NoteSearch,
    /// extra: note id
    NoteReport,
    FolderNew,
    /// extra: folder id
    FolderRename,
    /// extra: character
    Guestbook,
}

/// Menu actions of the website features.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteAction {
    MoveNotes(Vec<i64>, i64),
    NewFolder,
    RenameFolder(i64, String),
    DeleteFolder(i64),
    EmptyTrash,
    DeleteMessage(String),
    TrashMessages(String),
    GuestbookPost { character: String, private: bool, text: String },
    ComposeNote(String),
}

pub struct NotesState {
    pub folders: Vec<NoteFolder>,
    /// The search shown as an extra entry in the folder column.
    pub search: Option<NoteList>,
    pub list_sel: usize,
    pub page: usize,
    pub notes: Option<Result<Vec<NoteSummary>, String>>,
    pub total: i64,
    pub sel: usize,
    pub focus_notes: bool,
    /// The note being read.
    pub view: Option<Option<Result<Note, String>>>,
    pub scroll: usize,
}

impl NotesState {
    fn new(start: NoteList) -> NotesState {
        let mut s = NotesState {
            folders: Vec::new(),
            search: None,
            list_sel: 0,
            page: 0,
            notes: None,
            total: 0,
            sel: 0,
            focus_notes: true,
            view: None,
            scroll: 0,
        };
        match start {
            NoteList::Folder(1) => {}
            l @ (NoteList::With(_) | NoteList::Containing(_)) => s.search = Some(l),
            l => s.pending_select(l),
        }
        s
    }

    /// Entries of the folder column: (label, list, unread).
    pub fn entries(&self) -> Vec<(String, NoteList, i64)> {
        let mut v: Vec<(String, NoteList, i64)> = if self.folders.is_empty() {
            vec![("Inbox".into(), NoteList::Folder(1), 0)]
        } else {
            self.folders.iter().map(|f| (f.name.clone(), NoteList::Folder(f.id), f.unread)).collect()
        };
        v.push(("Unread".into(), NoteList::Unread, 0));
        v.push(("Starred".into(), NoteList::Starred, 0));
        if let Some(s) = &self.search {
            let label = match s {
                NoteList::With(n) => format!("With {n}"),
                NoteList::Containing(t) => format!("\"{}\"", trunc(t, 16)),
                _ => "Search".into(),
            };
            v.push((label, s.clone(), 0));
        }
        v
    }

    pub fn list(&self) -> NoteList {
        let e = self.entries();
        e.get(self.list_sel.min(e.len() - 1)).map(|x| x.1.clone()).unwrap_or(NoteList::Folder(1))
    }

    /// Select a list once the folders are known (or now, if it's a fixed one).
    fn pending_select(&mut self, l: NoteList) {
        if let Some(i) = self.entries().iter().position(|e| e.1 == l) {
            self.list_sel = i;
        }
    }

    fn selected(&self) -> Option<&NoteSummary> {
        match &self.notes {
            Some(Ok(n)) => n.get(self.sel),
            _ => None,
        }
    }

    fn folder_name(&self, id: i64) -> String {
        self.folders.iter().find(|f| f.id == id).map(|f| f.name.clone()).unwrap_or_else(|| format!("folder {id}"))
    }
}

pub struct MessagesState {
    pub data: Option<Result<MessageCenter, String>>,
    pub cat_sel: usize,
    pub sel: usize,
    pub focus_items: bool,
}

pub struct PersonalsState {
    pub data: Option<Result<Vec<Personal>, String>>,
    pub filter: LineEditor,
    pub sel: usize,
}

impl PersonalsState {
    pub fn filtered(&self) -> Vec<Personal> {
        let Some(Ok(all)) = &self.data else { return Vec::new() };
        let words: Vec<String> = self.filter.text().to_lowercase().split_whitespace().map(String::from).collect();
        all.iter()
            .filter(|p| {
                let hay = format!("{} {} {} {} {} {} {}", p.name, p.gender, p.orientation, p.role, p.pref, p.age, p.message).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .cloned()
            .collect()
    }
}

/// `[user]Name[/user]` names in a text, in order, without duplicates.
fn user_tags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in text.split("[user]").skip(1) {
        if let Some(n) = part.split("[/user]").next() {
            if !n.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
                out.push(n.to_string());
            }
        }
    }
    out
}

impl App {
    /// Run a website call on a worker thread.
    pub(super) fn site_call<F>(&mut self, f: F)
    where
        F: FnOnce(&crate::api::Api) -> SiteReply + Send + 'static,
    {
        self.api_call(move |api| ApiReply::Site(f(api)));
    }

    fn site_do<F>(&mut self, what: &str, refresh: Refresh, f: F)
    where
        F: FnOnce(&crate::api::Api) -> Result<(), String> + Send + 'static,
    {
        let what = what.to_string();
        self.site_call(move |api| SiteReply::Done { what, result: f(api), refresh });
    }

    // ------------------------------------------------------------------
    // Opening things

    pub(super) fn open_notes(&mut self, start: NoteList) {
        self.dialogs.retain(|d| !matches!(d, Dialog::Notes(_)));
        let st = NotesState::new(start);
        let list = st.list();
        self.dialogs.push(Dialog::Notes(Box::new(st)));
        self.site_call(|api| SiteReply::Folders(api.site.note_folders()));
        self.load_notes(list, 0);
    }

    fn load_notes(&mut self, list: NoteList, page: usize) {
        self.site_call(move |api| {
            let result = api.site.notes(&list, page * PAGE, PAGE);
            SiteReply::Notes { list, page, result }
        });
    }

    /// A new note arrived: refresh the notes window if it is open.
    pub(super) fn refresh_open_notes(&mut self) {
        if self.dialogs.iter().any(|d| matches!(d, Dialog::Notes(_))) {
            self.reload_notes();
        }
    }

    fn reload_notes(&mut self) {
        let Some(Dialog::Notes(n)) = self.dialogs.iter().rev().find(|d| matches!(d, Dialog::Notes(_))) else { return };
        let (list, page) = (n.list(), n.page);
        self.site_call(|api| SiteReply::Folders(api.site.note_folders()));
        self.load_notes(list, page);
        self.poll_site_soon();
    }

    fn open_note(&mut self, id: i64) {
        if let Some(Dialog::Notes(n)) = self.dialogs.last_mut() {
            n.view = Some(None);
            n.scroll = 0;
        }
        self.site_call(move |api| SiteReply::Note(api.site.note(id)));
    }

    pub(super) fn compose_note(&mut self, to: &str) {
        if to.trim().is_empty() {
            self.prompt("Send a note to (character)", "", PromptKind::Site(SitePrompt::NoteTo), false);
        } else {
            self.prompt(&format!("Note to {}: title", to.trim()), "", PromptKind::Site(SitePrompt::NoteTitle), false);
            self.set_prompt_extra(to.trim());
        }
    }

    fn set_prompt_extra(&mut self, extra: &str) {
        if let Some(Dialog::Prompt(p)) = self.dialogs.last_mut() {
            p.extra = extra.to_string();
        }
    }

    pub(super) fn open_messages(&mut self) {
        self.dialogs.retain(|d| !matches!(d, Dialog::Messages(_)));
        self.dialogs.push(Dialog::Messages(Box::new(MessagesState { data: None, cat_sel: 0, sel: 0, focus_items: false })));
        self.load_messages("", 0);
    }

    fn load_messages(&mut self, category: &str, offset: usize) {
        let c = category.to_string();
        self.site_call(move |api| SiteReply::Messages(api.site.message_center(&c, offset)));
    }

    pub(super) fn open_news(&mut self, page: usize) {
        self.flash("Loading news…");
        self.site_call(move |api| SiteReply::News(page, api.site.news(page * NEWS_PAGE)));
    }

    pub(super) fn open_personals(&mut self, filter: &str) {
        self.dialogs.retain(|d| !matches!(d, Dialog::Personals(_)));
        self.dialogs.push(Dialog::Personals(Box::new(PersonalsState { data: None, filter: LineEditor::with_text(filter), sel: 0 })));
        self.site_call(|api| SiteReply::Personals(api.site.personals()));
    }

    pub(super) fn load_profile_groups(&mut self, name: &str) {
        let n = name.to_string();
        self.site_call(move |api| {
            let r = api.character_id(&n).and_then(|id| api.site.profile_groups(id));
            SiteReply::Groups(n, r)
        });
    }

    pub(super) fn sign_guestbook(&mut self, name: &str) {
        self.prompt(&format!("Sign {name}'s guestbook"), "", PromptKind::Site(SitePrompt::Guestbook), true);
        self.set_prompt_extra(name);
    }

    // ------------------------------------------------------------------
    // Counters

    /// Check the unread counters in a few seconds (after a site notification).
    pub(super) fn poll_site_soon(&mut self) {
        if self.settings.site_poll_minutes > 0 {
            self.site_poll_at = Some(Instant::now() + Duration::from_secs(3));
        }
    }

    /// Called from the periodic tick.
    pub(super) fn site_tick(&mut self) {
        let Some(at) = self.site_poll_at else { return };
        if Instant::now() < at || self.api.is_none() || self.settings.site_poll_minutes == 0 {
            return;
        }
        self.site_poll_at = Some(Instant::now() + Duration::from_secs(self.settings.site_poll_minutes as u64 * 60));
        self.site_call(|api| SiteReply::Counters(api.site.counters()));
    }

    /// Status bar part for the counters: (text, key to open it).
    pub(super) fn site_counter_spans(&self) -> Vec<(String, KeyEvent)> {
        let c = self.site_counters;
        let mut v = Vec::new();
        if c.notes > 0 {
            v.push((
                format!("✉ {} note{} (Alt+N)", c.notes, if c.notes == 1 { "" } else { "s" }),
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT),
            ));
        }
        if c.messages > 0 {
            v.push((
                format!("★ {} site message{} (Alt+M)", c.messages, if c.messages == 1 { "" } else { "s" }),
                KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT),
            ));
        }
        v
    }

    // ------------------------------------------------------------------
    // Replies

    pub(super) fn on_site(&mut self, r: SiteReply) {
        match r {
            SiteReply::Folders(Ok(f)) => {
                for d in self.dialogs.iter_mut() {
                    if let Dialog::Notes(n) = d {
                        let cur = n.list();
                        n.folders = f.clone();
                        n.pending_select(cur);
                    }
                }
                if let Some(inbox) = f.iter().find(|x| x.id == 1) {
                    self.site_counters.notes = inbox.unread;
                }
            }
            SiteReply::Folders(Err(e)) => self.site_error(&e),
            SiteReply::Notes { list, page, result } => {
                for d in self.dialogs.iter_mut() {
                    if let Dialog::Notes(n) = d {
                        if n.list() == list {
                            n.page = page;
                            match result {
                                Ok((notes, total)) => {
                                    n.total = total;
                                    n.sel = n.sel.min(notes.len().saturating_sub(1));
                                    n.notes = Some(Ok(notes));
                                }
                                Err(e) => n.notes = Some(Err(e)),
                            }
                            break;
                        }
                    }
                }
            }
            SiteReply::Note(r) => {
                if let Some(Dialog::Notes(n)) = self.dialogs.iter_mut().rev().find(|d| matches!(d, Dialog::Notes(_))) {
                    if n.view.is_some() {
                        n.view = Some(Some(r));
                    }
                }
                // Reading marks it read.
                self.reload_notes();
            }
            SiteReply::Counters(Ok(c)) => self.site_counters = c,
            SiteReply::Counters(Err(e)) => self.site_error(&e),
            SiteReply::Messages(r) => {
                if let Ok(mc) = &r {
                    self.site_counters.messages = mc.total;
                }
                if let Some(Dialog::Messages(m)) = self.dialogs.iter_mut().rev().find(|d| matches!(d, Dialog::Messages(_))) {
                    if let Ok(mc) = &r {
                        if let Some(i) = mc.categories.iter().position(|c| c.0 == mc.shown) {
                            m.cat_sel = i;
                        }
                        m.sel = m.sel.min(mc.items.len().saturating_sub(1));
                    }
                    m.data = Some(r);
                }
            }
            SiteReply::News(page, r) => match r {
                Ok((posts, count)) => {
                    let pages = (count as usize).div_ceil(NEWS_PAGE).max(1);
                    let mut text = String::new();
                    for p in &posts {
                        text.push_str(&format!(
                            "[b][color=cyan]{}[/color][/b]\nby [user]{}[/user], {} · {} comment{}\n\n{}\n\n[hr]\n",
                            p.title,
                            p.author,
                            if p.posted > 0 { crate::timeutil::fmt_datetime(p.posted) } else { p.ago.clone() },
                            p.comments,
                            if p.comments == 1 { "" } else { "s" },
                            p.content.trim()
                        ));
                    }
                    if posts.is_empty() {
                        text.push_str("No news posts.\n");
                    }
                    text.push_str(&format!("Page {} of {pages}.", page + 1));
                    if page + 1 < pages {
                        text.push_str(&format!(" Older posts: /news {}", page + 2));
                    }
                    let title = format!("F-List news, page {}", page + 1);
                    self.dialogs.push(Dialog::text_bbcode(&title, &text));
                }
                Err(e) => self.info(&format!("Could not load the news: {e}")),
            },
            SiteReply::Personals(r) => {
                if let Some(Dialog::Personals(p)) = self.dialogs.iter_mut().rev().find(|d| matches!(d, Dialog::Personals(_))) {
                    p.data = Some(r);
                }
            }
            SiteReply::Groups(name, r) => {
                for d in self.dialogs.iter_mut() {
                    if let Dialog::Profile(p) = d {
                        if p.name.eq_ignore_ascii_case(&name) {
                            p.groups = Some(r.clone());
                        }
                    }
                }
            }
            SiteReply::Done { what, result, refresh } => match result {
                Ok(()) => {
                    self.flash(&what);
                    match refresh {
                        Refresh::Nothing => {}
                        Refresh::Notes => self.reload_notes(),
                        Refresh::NotesList => {
                            if let Some(Dialog::Notes(n)) = self.dialogs.iter_mut().rev().find(|d| matches!(d, Dialog::Notes(_))) {
                                n.view = None;
                            }
                            self.reload_notes();
                        }
                        Refresh::Messages => {
                            let (cat, offset) = match self.dialogs.iter().rev().find(|d| matches!(d, Dialog::Messages(_))) {
                                Some(Dialog::Messages(m)) => match &m.data {
                                    Some(Ok(mc)) => (mc.shown.clone(), mc.offset),
                                    _ => (String::new(), 0),
                                },
                                _ => (String::new(), 0),
                            };
                            self.load_messages(&cat, offset);
                        }
                        Refresh::Guestbook(name) => {
                            let mut page = None;
                            for d in self.dialogs.iter_mut() {
                                if let Dialog::Profile(p) = d {
                                    if p.name.eq_ignore_ascii_case(&name) {
                                        p.guestbook = None;
                                        page = Some(p.gb_page);
                                    }
                                }
                            }
                            if let Some(page) = page {
                                self.api_call(move |api| ApiReply::Guestbook(name.clone(), page, api.guestbook(&name, page)));
                            }
                        }
                    }
                }
                Err(e) => {
                    self.flash(&format!("Failed: {e}"));
                    self.info(&format!("{what} failed: {e}"));
                }
            },
        }
    }

    /// Show a website error once per session in the console.
    fn site_error(&mut self, e: &str) {
        crate::net::log::debug(&format!("site: {e}"));
        if !self.site_error_shown {
            self.site_error_shown = true;
            self.sys_err(&format!("F-List website: {e}"));
        }
    }

    // ------------------------------------------------------------------
    // Prompts and menu actions

    pub(super) fn on_site_prompt(&mut self, kind: SitePrompt, value: String, extra: String) {
        let v = value.trim().to_string();
        match kind {
            SitePrompt::NoteTo => {
                if !v.is_empty() {
                    self.compose_note(&v);
                }
            }
            SitePrompt::NoteTitle => {
                let title = if v.is_empty() { "(no title)".to_string() } else { v };
                self.prompt(&format!("Note to {extra}: {title}"), "", PromptKind::Site(SitePrompt::NoteBody), true);
                self.set_prompt_extra(&format!("{extra}\n{title}"));
            }
            SitePrompt::NoteBody => {
                if v.is_empty() {
                    return self.flash("Empty note not sent.");
                }
                let (to, title) = extra.split_once('\n').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((extra, String::new()));
                let me = self.chat.me.clone();
                let what = format!("Note sent to {to}.");
                self.site_do(&what, Refresh::Notes, move |api| {
                    let id = api.character_id(&me)?;
                    api.site.note_send(id, &to, &title, &value)
                });
            }
            SitePrompt::NoteSearch => {
                if v.is_empty() {
                    return;
                }
                let list = match v.strip_prefix('@') {
                    Some(name) => NoteList::With(name.trim().to_string()),
                    None => NoteList::Containing(v),
                };
                if let Some(Dialog::Notes(n)) = self.dialogs.last_mut() {
                    n.search = Some(list.clone());
                    n.list_sel = n.entries().len() - 1;
                    n.notes = None;
                    n.sel = 0;
                    n.page = 0;
                    n.focus_notes = true;
                }
                self.load_notes(list, 0);
            }
            SitePrompt::NoteReport => {
                let Ok(id) = extra.parse::<i64>() else { return };
                if v.is_empty() {
                    return self.flash("A report needs a reason.");
                }
                self.site_do("Note reported to the moderators.", Refresh::Nothing, move |api| api.site.note_report(id, &v));
            }
            SitePrompt::FolderNew => {
                if !v.is_empty() {
                    self.site_do("Folder created.", Refresh::Notes, move |api| api.site.note_folder_create(&v));
                }
            }
            SitePrompt::FolderRename => {
                let Ok(id) = extra.parse::<i64>() else { return };
                if !v.is_empty() {
                    self.site_do("Folder renamed.", Refresh::Notes, move |api| api.site.note_folder_rename(id, &v));
                }
            }
            SitePrompt::Guestbook => {
                if v.is_empty() {
                    return;
                }
                let items = vec![
                    MenuItem::new(
                        "Post publicly",
                        Action::Site(SiteAction::GuestbookPost { character: extra.clone(), private: false, text: value.clone() }),
                    ),
                    MenuItem::new(
                        &format!("Post privately (only {extra} sees it)"),
                        Action::Site(SiteAction::GuestbookPost { character: extra.clone(), private: true, text: value }),
                    ),
                ];
                self.dialogs.push(Dialog::menu("Guestbook post", items));
            }
        }
    }

    pub(super) fn run_site_action(&mut self, a: SiteAction) {
        match a {
            SiteAction::MoveNotes(ids, folder) => {
                self.site_do("Moved.", Refresh::NotesList, move |api| api.site.notes_move(&ids, folder));
            }
            SiteAction::NewFolder => self.prompt("New folder name", "", PromptKind::Site(SitePrompt::FolderNew), false),
            SiteAction::RenameFolder(id, name) => {
                self.prompt("Rename folder", &name, PromptKind::Site(SitePrompt::FolderRename), false);
                self.set_prompt_extra(&id.to_string());
            }
            SiteAction::DeleteFolder(id) => {
                if let Some(Dialog::Notes(n)) = self.dialogs.last_mut() {
                    n.list_sel = 0;
                }
                self.site_do("Folder deleted; its notes are in the inbox.", Refresh::Notes, move |api| api.site.note_folder_delete(id));
            }
            SiteAction::EmptyTrash => self.site_do("Trash emptied.", Refresh::Notes, |api| api.site.notes_empty_trash()),
            SiteAction::DeleteMessage(id) => self.site_do("Message deleted.", Refresh::Messages, move |api| api.site.delete_message(&id)),
            SiteAction::TrashMessages(cat) => {
                self.site_do("Messages deleted.", Refresh::Messages, move |api| api.site.trash_messages(&cat))
            }
            SiteAction::GuestbookPost { character, private, text } => {
                let me = self.chat.me.clone();
                let what = format!("Posted in {character}'s guestbook.");
                let c = character.clone();
                self.site_do(&what, Refresh::Guestbook(character), move |api| {
                    let target = api.character_id(&c)?;
                    let source = api.character_id(&me)?;
                    api.site.guestbook_post(target, source, private, &text)
                });
            }
            SiteAction::ComposeNote(to) => self.compose_note(&to),
        }
    }

    // ------------------------------------------------------------------
    // Keys

    pub(super) fn notes_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Dialog::Notes(n)) = self.dialogs.last_mut() else { return };
        if let Some(view) = &n.view {
            let note = view.as_ref().and_then(|r| r.as_ref().ok()).cloned();
            let folder = n.list();
            match key.code {
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => {
                    n.view = None;
                }
                KeyCode::Up | KeyCode::Char('k') => n.scroll = n.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => n.scroll += 1,
                KeyCode::PageUp => n.scroll = n.scroll.saturating_sub(10),
                KeyCode::PageDown | KeyCode::Char(' ') => n.scroll += 10,
                KeyCode::Char('r') => {
                    if let Some(note) = note {
                        let me = self.chat.me.clone();
                        // Reply to the other side of the conversation.
                        let to = if note.from.eq_ignore_ascii_case(&me) { note.to.clone() } else { note.from.clone() };
                        let title =
                            if note.title.to_lowercase().starts_with("re:") { note.title.clone() } else { format!("Re: {}", note.title) };
                        self.prompt(&format!("Note to {to}: {title}"), "", PromptKind::Site(SitePrompt::NoteBody), true);
                        self.set_prompt_extra(&format!("{to}\n{title}"));
                    }
                }
                KeyCode::Char('p') => {
                    if let Some(note) = note {
                        self.open_profile(&note.from);
                    }
                }
                KeyCode::Char('x') => {
                    if let Some(note) = note {
                        self.prompt(
                            &format!("Report the note \"{}\" from {}: reason", note.title, note.from),
                            "",
                            PromptKind::Site(SitePrompt::NoteReport),
                            false,
                        );
                        self.set_prompt_extra(&note.id.to_string());
                    }
                }
                KeyCode::Delete | KeyCode::Char('d') => {
                    if let Some(note) = note {
                        let id = note.id;
                        if folder == NoteList::Folder(3) {
                            self.site_do("Moved back to the inbox.", Refresh::NotesList, move |api| api.site.notes_move(&[id], 1));
                        } else {
                            self.site_do("Moved to the trash.", Refresh::NotesList, move |api| api.site.notes_trash(&[id]));
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        let entries = n.entries();
        let count = n.notes.as_ref().and_then(|r| r.as_ref().ok()).map(|v| v.len()).unwrap_or(0);
        let pages = ((n.total.max(0) as usize).div_ceil(PAGE)).max(1);
        let mut load: Option<(NoteList, usize)> = None;
        match key.code {
            KeyCode::Esc => {
                self.dialogs.pop();
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => n.focus_notes = !n.focus_notes,
            KeyCode::Left => n.focus_notes = false,
            KeyCode::Right => n.focus_notes = true,
            KeyCode::Up | KeyCode::Char('k') if !n.focus_notes => {
                n.list_sel = n.list_sel.saturating_sub(1);
                load = Some((n.list(), 0));
            }
            KeyCode::Down | KeyCode::Char('j') if !n.focus_notes => {
                n.list_sel = (n.list_sel + 1).min(entries.len() - 1);
                load = Some((n.list(), 0));
            }
            KeyCode::Up | KeyCode::Char('k') => n.sel = n.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => n.sel = (n.sel + 1).min(count.saturating_sub(1)),
            KeyCode::PageDown | KeyCode::Char(']') if n.page + 1 < pages => load = Some((n.list(), n.page + 1)),
            KeyCode::PageUp | KeyCode::Char('[') if n.page > 0 => load = Some((n.list(), n.page - 1)),
            KeyCode::Char('r') if ctrl => load = Some((n.list(), n.page)),
            KeyCode::Enter if !n.focus_notes => n.focus_notes = true,
            KeyCode::Enter => {
                if let Some(id) = n.selected().map(|s| s.id) {
                    self.open_note(id);
                }
                return;
            }
            KeyCode::Char('n') => return self.compose_note(""),
            KeyCode::Char('/') => {
                return self.prompt(
                    "Search notes: text, or @name for notes with a character",
                    "",
                    PromptKind::Site(SitePrompt::NoteSearch),
                    false,
                )
            }
            KeyCode::Char('s') => {
                if let Some(s) = n.selected().cloned() {
                    let what = if s.starred { "Star removed." } else { "Starred." };
                    self.site_do(what, Refresh::Notes, move |api| api.site.notes_set_starred(&[s.id], !s.starred));
                }
                return;
            }
            KeyCode::Char('u') => {
                if let Some(s) = n.selected().cloned() {
                    let what = if s.read { "Marked as unread." } else { "Marked as read." };
                    let folder = if s.folder > 0 { s.folder } else { 1 };
                    self.site_do(what, Refresh::Notes, move |api| api.site.notes_set_read(&[s.id], !s.read, folder));
                }
                return;
            }
            KeyCode::Delete | KeyCode::Char('d') => {
                if let Some(s) = n.selected().cloned() {
                    if n.list() == NoteList::Folder(3) {
                        self.site_do("Moved back to the inbox.", Refresh::Notes, move |api| api.site.notes_move(&[s.id], 1));
                    } else {
                        self.site_do("Moved to the trash.", Refresh::Notes, move |api| api.site.notes_trash(&[s.id]));
                    }
                }
                return;
            }
            KeyCode::Char('m') => {
                if let Some(s) = n.selected().cloned() {
                    let items: Vec<MenuItem> = n
                        .folders
                        .iter()
                        .filter(|f| f.id != s.folder && f.id != 2)
                        .map(|f| MenuItem::new(&f.name, Action::Site(SiteAction::MoveNotes(vec![s.id], f.id))))
                        .collect();
                    self.dialogs.push(Dialog::menu("Move note to", items));
                }
                return;
            }
            KeyCode::Char('f') => {
                let mut items = vec![MenuItem::new("New folder…", Action::Site(SiteAction::NewFolder))];
                if let NoteList::Folder(id) = n.list() {
                    if id > 3 {
                        let name = n.folder_name(id);
                        items.push(MenuItem::new(&format!("Rename \"{name}\"…"), Action::Site(SiteAction::RenameFolder(id, name.clone()))));
                        items.push(MenuItem::new(
                            &format!("Delete \"{name}\" (notes go to the inbox)"),
                            Action::Site(SiteAction::DeleteFolder(id)),
                        ));
                    }
                    if id == 3 {
                        items.push(MenuItem::new("Empty the trash", Action::Site(SiteAction::EmptyTrash)));
                    }
                }
                self.dialogs.push(Dialog::menu("Folders", items));
                return;
            }
            KeyCode::Char('p') if n.focus_notes => {
                if let Some(s) = n.selected().cloned() {
                    let who = if s.from.eq_ignore_ascii_case(&self.chat.me) { s.to } else { s.from };
                    self.open_profile(&who);
                }
                return;
            }
            _ => {}
        }
        if let Some((list, page)) = load {
            if !(ctrl && key.code == KeyCode::Char('r')) {
                n.sel = 0;
            }
            n.notes = None;
            n.page = page;
            self.load_notes(list, page);
        }
    }

    pub(super) fn messages_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Dialog::Messages(m)) = self.dialogs.last_mut() else { return };
        let (cats, items, shown, offset, next) = match &m.data {
            Some(Ok(mc)) => (mc.categories.clone(), mc.items.clone(), mc.shown.clone(), mc.offset, mc.next),
            _ => (
                crate::site::MESSAGE_CATEGORIES.iter().map(|(k, l)| (k.to_string(), l.to_string(), 0)).collect(),
                Vec::new(),
                String::new(),
                0,
                None,
            ),
        };
        let mut load: Option<(String, usize)> = None;
        match key.code {
            KeyCode::Esc => {
                self.dialogs.pop();
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => m.focus_items = !m.focus_items,
            KeyCode::Left => m.focus_items = false,
            KeyCode::Right => m.focus_items = true,
            KeyCode::Up | KeyCode::Char('k') if !m.focus_items => {
                m.cat_sel = m.cat_sel.saturating_sub(1);
                load = cats.get(m.cat_sel).map(|c| (c.0.clone(), 0));
            }
            KeyCode::Down | KeyCode::Char('j') if !m.focus_items => {
                m.cat_sel = (m.cat_sel + 1).min(cats.len().saturating_sub(1));
                load = cats.get(m.cat_sel).map(|c| (c.0.clone(), 0));
            }
            KeyCode::Up | KeyCode::Char('k') => m.sel = m.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => m.sel = (m.sel + 1).min(items.len().saturating_sub(1)),
            KeyCode::Char('r') if ctrl => load = Some((shown, offset)),
            KeyCode::PageDown | KeyCode::Char('n') if !ctrl => {
                if let Some(n) = next {
                    load = Some((shown, n));
                }
            }
            KeyCode::PageUp | KeyCode::Char('p') if !ctrl && offset > 0 => {
                load = Some((shown, offset.saturating_sub(crate::site::MESSAGES_PER_PAGE)));
            }
            KeyCode::Enter if !m.focus_items => m.focus_items = true,
            KeyCode::Enter => {
                if let Some((id, text)) = items.get(m.sel).cloned() {
                    let mut menu: Vec<MenuItem> =
                        user_tags(&text).into_iter().map(|n| MenuItem::new(&format!("{n}…"), Action::Profile(n))).collect();
                    if !id.is_empty() {
                        menu.push(MenuItem::new("Delete this message", Action::Site(SiteAction::DeleteMessage(id))));
                    }
                    if menu.is_empty() {
                        return;
                    }
                    self.dialogs.push(Dialog::menu("Site message", menu));
                }
                return;
            }
            KeyCode::Delete | KeyCode::Char('d') if m.focus_items => {
                if let Some((id, _)) = items.get(m.sel).cloned().filter(|x| !x.0.is_empty()) {
                    self.run_site_action(SiteAction::DeleteMessage(id));
                }
                return;
            }
            KeyCode::Char('t') => {
                if let Some((key, label, _)) = cats.iter().find(|c| c.0 == shown).cloned() {
                    self.dialogs.push(Dialog::confirm(
                        &format!("Delete all messages in \"{label}\"?"),
                        Action::Site(SiteAction::TrashMessages(key)),
                    ));
                }
                return;
            }
            KeyCode::Char('o') => {
                let url = format!("{}/messages.php?show={}", self.settings.api_base, shown);
                return self.run_action(Action::OpenUrl(url));
            }
            _ => {}
        }
        if let Some((c, off)) = load {
            m.sel = 0;
            m.data = match m.data.take() {
                Some(Ok(mut mc)) => {
                    mc.items.clear();
                    mc.shown = c.clone();
                    Some(Ok(mc))
                }
                _ => None,
            };
            self.load_messages(&c, off);
        }
    }

    pub(super) fn personals_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Dialog::Personals(p)) = self.dialogs.last_mut() else { return };
        let list = p.filtered();
        match key.code {
            KeyCode::Esc if !p.filter.is_empty() => p.filter.clear(),
            KeyCode::Esc => {
                self.dialogs.pop();
            }
            KeyCode::Up => p.sel = p.sel.saturating_sub(1),
            KeyCode::Down => p.sel = (p.sel + 1).min(list.len().saturating_sub(1)),
            KeyCode::PageUp => p.sel = p.sel.saturating_sub(10),
            KeyCode::PageDown => p.sel = (p.sel + 10).min(list.len().saturating_sub(1)),
            KeyCode::Char('r') if ctrl => {
                p.data = None;
                self.site_call(|api| SiteReply::Personals(api.site.personals()));
            }
            KeyCode::Enter => {
                if let Some(x) = list.get(p.sel) {
                    let name = x.name.clone();
                    self.open_user_menu(&name);
                }
            }
            _ => {
                if let EditResult::Changed = p.filter.handle(key) {
                    p.sel = 0;
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Drawing

    /// Draw one of the website dialogs. Returns (dialog area, list area, list offset).
    pub(super) fn draw_site_dialog(&mut self, buf: &mut Buffer, big: Rect, idx: usize, top: bool) -> (Rect, Rect, usize) {
        let ascii = self.settings.ascii_borders;
        let ns = |n: &str| self.name_style(n);
        match &self.dialogs[idx] {
            Dialog::Notes(n) => {
                let title = match &n.view {
                    Some(_) => "Note".to_string(),
                    None => {
                        let pages = ((n.total.max(0) as usize).div_ceil(PAGE)).max(1);
                        format!(
                            "Notes · {} · {} note{} · page {}/{pages}",
                            n.entries()[n.list_sel.min(n.entries().len() - 1)].0,
                            n.total,
                            if n.total == 1 { "" } else { "s" },
                            n.page + 1
                        )
                    }
                };
                let inner = widgets::popup(buf, big, &title, ascii);
                let body = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
                if let Some(view) = &n.view {
                    let lines: Vec<Line<'static>> = match view {
                        None => vec![Line::from(Span::styled("Loading…", Style::default().fg(Color::DarkGray)))],
                        Some(Err(e)) => vec![Line::from(Span::styled(format!("Error: {e}"), Style::default().fg(Color::LightRed)))],
                        Some(Ok(note)) => {
                            let key = Style::default().fg(Color::Gray);
                            let mut l = vec![
                                Line::from(Span::styled(
                                    note.title.clone(),
                                    Style::default().add_modifier(Modifier::BOLD).fg(Color::White),
                                )),
                                Line::from(vec![
                                    Span::styled("From ", key),
                                    Span::styled(note.from.clone(), self.name_style(&note.from)),
                                    Span::styled(" to ", key),
                                    Span::styled(note.to.clone(), self.name_style(&note.to)),
                                    Span::styled(format!(" · {}", note.sent), key),
                                ]),
                                Line::default(),
                            ];
                            l.extend(bbcode::to_lines(&note.body, Style::default(), &ns, self.settings.show_eicon_names));
                            l
                        }
                    };
                    let lines = widgets::wrap_all(&lines, body.width as usize, 0);
                    let s = n.scroll.min(lines.len().saturating_sub(body.height as usize));
                    for (i, l) in lines.iter().skip(s).take(body.height as usize).enumerate() {
                        buf.set_line(body.x, body.y + i as u16, l, body.width);
                    }
                    let hints = [("r", "reply"), ("p", "profile"), ("d", "trash"), ("x", "report"), ("Esc", "back")];
                    widgets::draw_hints(buf, inner.x, inner.bottom() - 1, &hints, inner.width);
                    return (big, Rect::default(), 0);
                }
                let left_w = (body.width / 4).clamp(14, 24);
                let left = Rect::new(body.x, body.y, left_w, body.height);
                let right = Rect::new(body.x + left_w + 1, body.y, body.width.saturating_sub(left_w + 1), body.height);
                let folders: Vec<Line<'static>> = n
                    .entries()
                    .into_iter()
                    .map(|(label, _, unread)| {
                        let mut s = vec![Span::raw(trunc(&label, left_w as usize - 6))];
                        if unread > 0 {
                            s.push(Span::styled(format!(" {unread}"), Style::default().fg(Color::Yellow)));
                        }
                        Line::from(s)
                    })
                    .collect();
                let loff = widgets::list(buf, left, &folders, n.list_sel, !n.focus_notes);
                let me = self.chat.me.clone();
                let mut roff = 0;
                match &n.notes {
                    None => buf.set_string(right.x + 1, right.y, "Loading…", Style::default().fg(Color::DarkGray)),
                    Some(Err(e)) => {
                        buf.set_stringn(
                            right.x + 1,
                            right.y,
                            format!("Error: {e}"),
                            right.width as usize,
                            Style::default().fg(Color::LightRed),
                        );
                    }
                    Some(Ok(notes)) if notes.is_empty() => {
                        buf.set_string(right.x + 1, right.y, "No notes here.", Style::default().fg(Color::DarkGray));
                    }
                    Some(Ok(notes)) => {
                        let items: Vec<Line<'static>> = notes
                            .iter()
                            .map(|s| {
                                let sent = s.from.eq_ignore_ascii_case(&me);
                                let who = if sent { format!("→ {}", s.to) } else { s.from.clone() };
                                let st = if s.read { Style::default() } else { Style::default().add_modifier(Modifier::BOLD) };
                                Line::from(vec![
                                    Span::styled(if s.read { "  " } else { "● " }, Style::default().fg(Color::Yellow)),
                                    Span::styled(if s.starred { "★ " } else { "  " }, Style::default().fg(Color::Yellow)),
                                    Span::styled(format!("{:<20} ", trunc(&who, 20)), self.name_style(if sent { &s.to } else { &s.from })),
                                    Span::styled(format!("{:<12} ", trunc(&s.sent, 12)), Style::default().fg(Color::DarkGray)),
                                    Span::styled(s.title.clone(), st),
                                ])
                            })
                            .collect();
                        roff = widgets::list(buf, right, &items, n.sel, n.focus_notes);
                    }
                }
                let hints = [
                    ("Enter", "read"),
                    ("n", "new"),
                    ("s", "star"),
                    ("u", "read/unread"),
                    ("d", "trash"),
                    ("m", "move"),
                    ("f", "folders"),
                    ("/", "search"),
                    ("[ ]", "page"),
                    ("Esc", "close"),
                ];
                widgets::draw_hints(buf, inner.x, inner.bottom() - 1, &hints, inner.width);
                if top && !n.focus_notes {
                    (big, left, loff)
                } else {
                    (big, right, roff)
                }
            }
            Dialog::Messages(m) => {
                let title = match &m.data {
                    Some(Ok(mc)) if !mc.range.is_empty() => {
                        format!("Site messages: {}", mc.range.to_lowercase().replacen("showing ", "", 1))
                    }
                    _ => "Site messages".to_string(),
                };
                let inner = widgets::popup(buf, big, &title, ascii);
                let body = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
                let left_w = (body.width / 3).clamp(18, 28);
                let left = Rect::new(body.x, body.y, left_w, body.height);
                let right = Rect::new(body.x + left_w + 1, body.y, body.width.saturating_sub(left_w + 1), body.height);
                let (cats, items) = match &m.data {
                    Some(Ok(mc)) => (mc.categories.clone(), Some(Ok(mc.items.clone()))),
                    Some(Err(e)) => (Vec::new(), Some(Err(e.clone()))),
                    None => (Vec::new(), None),
                };
                let cats = if cats.is_empty() {
                    crate::site::MESSAGE_CATEGORIES.iter().map(|(k, l)| (k.to_string(), l.to_string(), 0)).collect()
                } else {
                    cats
                };
                let cat_lines: Vec<Line<'static>> = cats
                    .iter()
                    .map(|(_, l, c)| {
                        let st = if *c > 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                        Line::from(vec![Span::raw(trunc(l, left_w as usize - 6)), Span::styled(format!(" {c}"), st)])
                    })
                    .collect();
                let loff = widgets::list(buf, left, &cat_lines, m.cat_sel, !m.focus_items);
                let mut roff = 0;
                match items {
                    None => buf.set_string(right.x + 1, right.y, "Loading…", Style::default().fg(Color::DarkGray)),
                    Some(Err(e)) => {
                        buf.set_stringn(
                            right.x + 1,
                            right.y,
                            format!("Error: {e}"),
                            right.width as usize,
                            Style::default().fg(Color::LightRed),
                        );
                    }
                    Some(Ok(items)) if items.is_empty() => {
                        buf.set_string(right.x + 1, right.y, "No messages here.", Style::default().fg(Color::DarkGray));
                    }
                    Some(Ok(items)) => {
                        // List of first lines on top, the selected message in full below.
                        let list_h = (right.height / 2).max(3).min(items.len() as u16 + 1);
                        let list_area = Rect::new(right.x, right.y, right.width, list_h);
                        let lines: Vec<Line<'static>> =
                            items.iter().map(|(_, t)| Line::from(bbcode::strip(t.lines().next().unwrap_or("")))).collect();
                        roff = widgets::list(buf, list_area, &lines, m.sel, m.focus_items);
                        if let Some((_, t)) = items.get(m.sel) {
                            let view = Rect::new(
                                right.x + 1,
                                right.y + list_h + 1,
                                right.width.saturating_sub(1),
                                right.height.saturating_sub(list_h + 1),
                            );
                            let full = widgets::wrap_all(&bbcode::to_lines(t, Style::default(), &ns, true), view.width as usize, 0);
                            for (i, l) in full.iter().take(view.height as usize).enumerate() {
                                buf.set_line(view.x, view.y + i as u16, l, view.width);
                            }
                        }
                    }
                }
                let hints = [
                    ("Tab", "switch"),
                    ("Enter", "actions"),
                    ("PgUp/PgDn", "page"),
                    ("d", "delete"),
                    ("t", "delete all"),
                    ("o", "browser"),
                    ("Ctrl+R", "refresh"),
                    ("Esc", "close"),
                ];
                widgets::draw_hints(buf, inner.x, inner.bottom() - 1, &hints, inner.width);
                if m.focus_items {
                    (big, Rect::new(right.x, right.y, right.width, (right.height / 2).max(3)), roff)
                } else {
                    (big, left, loff)
                }
            }
            Dialog::Personals(p) => {
                let list = p.filtered();
                let total = match &p.data {
                    Some(Ok(a)) => a.len(),
                    _ => 0,
                };
                let inner =
                    widgets::popup(buf, big, &format!("Roleplay ads from the F-List front page · {} of {total}", list.len()), ascii);
                buf.set_string(inner.x, inner.y, "Filter: ", Style::default().fg(Color::Gray));
                buf.set_string(inner.x + 8, inner.y, p.filter.text(), Style::default().fg(Color::Yellow));
                let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(3));
                let mut off = 0;
                let list_h = (body.height / 2).max(3);
                let list_area = Rect::new(body.x, body.y, body.width, list_h);
                match &p.data {
                    None => buf.set_string(body.x + 1, body.y, "Loading…", Style::default().fg(Color::DarkGray)),
                    Some(Err(e)) => {
                        buf.set_stringn(
                            body.x + 1,
                            body.y,
                            format!("Error: {e}"),
                            body.width as usize,
                            Style::default().fg(Color::LightRed),
                        );
                    }
                    Some(Ok(_)) => {
                        let items: Vec<Line<'static>> = list
                            .iter()
                            .map(|x| {
                                let info: Vec<&str> =
                                    [x.gender.as_str(), x.orientation.as_str(), x.role.as_str(), x.pref.as_str(), x.age.as_str()]
                                        .into_iter()
                                        .filter(|s| !s.is_empty())
                                        .collect();
                                Line::from(vec![
                                    Span::styled(format!("{:<24} ", trunc(&x.name, 24)), self.name_style(&x.name)),
                                    Span::styled(info.join(" · "), Style::default().fg(Color::Gray)),
                                ])
                            })
                            .collect();
                        off = widgets::list(buf, list_area, &items, p.sel, true);
                        if items.is_empty() {
                            buf.set_string(body.x + 1, body.y, "Nothing matches.", Style::default().fg(Color::DarkGray));
                        }
                        if let Some(x) = list.get(p.sel) {
                            let view = Rect::new(
                                body.x + 1,
                                body.y + list_h + 1,
                                body.width.saturating_sub(1),
                                body.height.saturating_sub(list_h + 1),
                            );
                            let lines = widgets::wrap_all(
                                &bbcode::to_lines(&x.message, Style::default(), &ns, self.settings.show_eicon_names),
                                view.width as usize,
                                0,
                            );
                            for (i, l) in lines.iter().take(view.height as usize).enumerate() {
                                buf.set_line(view.x, view.y + i as u16, l, view.width);
                            }
                        }
                    }
                }
                let hints = [("type", "filter"), ("Enter", "actions"), ("Ctrl+R", "refresh"), ("Esc", "close")];
                widgets::draw_hints(buf, inner.x, inner.bottom() - 1, &hints, inner.width);
                (big, list_area, off)
            }
            _ => (big, Rect::default(), 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_entries() {
        let mut n = NotesState::new(NoteList::Starred);
        assert_eq!(n.list(), NoteList::Starred);
        n.folders = vec![
            NoteFolder { id: 1, name: "Inbox".into(), total: 2, unread: 1 },
            NoteFolder { id: 2, name: "Outbox".into(), total: 0, unread: 0 },
            NoteFolder { id: 3, name: "Trash".into(), total: 0, unread: 0 },
        ];
        n.pending_select(NoteList::Starred);
        assert_eq!(n.list(), NoteList::Starred);
        assert_eq!(n.entries().len(), 5);
        let s = NotesState::new(NoteList::With("Bob".into()));
        assert_eq!(s.list(), NoteList::Folder(1));
        assert_eq!(s.entries().last().unwrap().0, "With Bob");
    }

    #[test]
    fn user_tag_names() {
        assert_eq!(user_tags("[user]A[/user] and [user]B[/user] and [user]a[/user]"), vec!["A".to_string(), "B".to_string()]);
    }
}

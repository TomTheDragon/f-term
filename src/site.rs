//! The F-List website itself (not the official JSON API): notes, the
//! message center, news posts, roleplay ads, profile groups and guestbook
//! posts.
//!
//! These are not in the official API, so the client logs in to the website
//! like a browser does (`/action/script_login.php` with the login form's
//! csrf token). The session cookies are kept in memory only.
//!
//! All calls are blocking; the UI runs them on worker threads.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::protocol::{vi, vs};

#[derive(Default)]
struct Session {
    cookies: Vec<(String, String)>,
    /// Token for POST requests, from a page's `<meta name="csrf-token">`.
    csrf: String,
    logged_in: bool,
}

#[derive(Clone)]
pub struct Site {
    agent: ureq::Agent,
    base: String,
    account: String,
    password: Arc<Mutex<String>>,
    session: Arc<Mutex<Session>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NoteFolder {
    pub id: i64,
    pub name: String,
    pub total: i64,
    pub unread: i64,
}

/// Which notes to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteList {
    Folder(i64),
    Unread,
    Starred,
    /// Notes to or from a character.
    With(String),
    /// Notes containing a text.
    Containing(String),
}

#[derive(Debug, Clone, Default)]
pub struct NoteSummary {
    pub id: i64,
    pub title: String,
    pub from: String,
    pub to: String,
    pub sent: String,
    pub folder: i64,
    pub read: bool,
    pub starred: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Note {
    pub id: i64,
    pub title: String,
    pub from: String,
    pub to: String,
    pub sent: String,
    /// The message as BBCode.
    pub body: String,
}

#[derive(Debug, Clone, Default)]
pub struct MessageCenter {
    /// The "N Messages" counter in the site header.
    pub total: i64,
    /// (key, label, count)
    pub categories: Vec<(String, String, i64)>,
    /// The category shown.
    pub shown: String,
    /// (message id, text as BBCode)
    pub items: Vec<(String, String)>,
    /// Where this page starts, and the start of the next one.
    pub offset: usize,
    pub next: Option<usize>,
    /// "Showing message 1 to 20" from the page heading.
    pub range: String,
}

#[derive(Debug, Clone, Default)]
pub struct NewsPost {
    pub title: String,
    pub author: String,
    pub posted: i64,
    pub ago: String,
    pub content: String,
    pub comments: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Personal {
    pub name: String,
    pub gender: String,
    pub orientation: String,
    pub role: String,
    pub pref: String,
    pub age: String,
    pub message: String,
}

/// A character from the "Browse New" pages.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrowseCharacter {
    pub name: String,
    /// (field, value), e.g. ("Gender", "Male").
    pub info: Vec<(String, String)>,
}

/// One page of new/updated or recently created characters.
#[derive(Debug, Clone, Default)]
pub struct BrowsePage {
    pub characters: Vec<BrowseCharacter>,
    pub offset: usize,
    pub next: Option<usize>,
}

/// Characters per page in "Browse New".
pub const BROWSE_PER_PAGE: usize = 20;

/// A group in the group list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupSummary {
    /// The `group=` value of its address.
    pub slug: String,
    pub name: String,
    /// As BBCode.
    pub description: String,
    pub owner: String,
    pub posts: i64,
    pub members: i64,
    pub last_post: String,
    /// One of your characters is a member.
    pub joined: bool,
}

/// A forum of a group.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Forum {
    pub id: i64,
    pub name: String,
    pub description: String,
    /// "715 posts in 285 threads"
    pub stats: String,
    /// "Last post by X in Y, 2w ago"
    pub last: String,
}

/// A group's own page.
#[derive(Debug, Clone, Default)]
pub struct Group {
    pub slug: String,
    pub id: i64,
    pub name: String,
    pub description: String,
    pub forums: Vec<Forum>,
    /// Your characters in the group and not in it: (character id, name).
    pub joined: Vec<(i64, String)>,
    pub not_joined: Vec<(i64, String)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThreadSummary {
    pub id: i64,
    pub title: String,
    pub author: String,
    pub posts: i64,
    pub created: String,
    pub last_post: String,
    pub locked: bool,
    pub poll: bool,
}

/// A forum with its threads.
#[derive(Debug, Clone, Default)]
pub struct ForumPage {
    pub id: i64,
    pub name: String,
    pub group_id: i64,
    pub group_slug: String,
    pub group_name: String,
    pub threads: Vec<ThreadSummary>,
    /// New threads can be posted here.
    pub can_post: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ForumPost {
    pub id: i64,
    pub author: String,
    pub posted: String,
    /// As BBCode.
    pub body: String,
}

/// One page of a thread.
#[derive(Debug, Clone, Default)]
pub struct ThreadPage {
    pub id: i64,
    pub subject: String,
    pub group_id: i64,
    pub group_slug: String,
    pub group_name: String,
    pub forum_id: i64,
    pub forum_name: String,
    pub posts: Vec<ForumPost>,
    /// Offsets of all pages, and the one shown.
    pub pages: Vec<usize>,
    pub offset: usize,
    /// Replies can be posted (not locked, allowed).
    pub can_reply: bool,
}

/// Unread notes and site messages, for the status bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub notes: i64,
    pub messages: i64,
}

/// Messages per page in the message center.
pub const MESSAGES_PER_PAGE: usize = 20;

pub const MESSAGE_CATEGORIES: &[(&str, &str)] = &[
    ("friends", "Friend requests"),
    ("threadpost", "Thread updates"),
    ("characterupdate", "Character updates"),
    ("guestbookpost", "Guestbook posts"),
    ("grouprequest", "Group requests"),
    ("ticketupdate", "Ticket replies"),
    ("newspostcomment", "Comments & replies"),
];

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .tls_connector(Arc::new(crate::tls::UreqTls))
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        // Redirects are handled here, so the login's cookies can be read.
        .redirects(0)
        .user_agent(&format!("Mozilla/5.0 (compatible; {}/{})", crate::protocol::CLIENT_NAME, crate::protocol::CLIENT_VERSION))
        .build()
}

impl Site {
    pub fn new(base: &str, account: &str, password: &str) -> Site {
        Site {
            agent: agent(),
            base: base.trim_end_matches('/').to_string(),
            account: account.to_string(),
            password: Arc::new(Mutex::new(password.to_string())),
            session: Arc::new(Mutex::new(Session::default())),
        }
    }

    fn cookie_header(&self) -> String {
        self.session.lock().unwrap().cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
    }

    fn keep_cookies(&self, resp: &ureq::Response) {
        let mut s = self.session.lock().unwrap();
        for c in resp.all("set-cookie") {
            let first = c.split(';').next().unwrap_or("");
            let Some((k, v)) = first.split_once('=') else { continue };
            let (k, v) = (k.trim().to_string(), v.trim().to_string());
            s.cookies.retain(|(n, _)| *n != k);
            if !v.is_empty() && v != "deleted" {
                s.cookies.push((k, v));
            }
        }
    }

    /// One HTTP request: (status, location header, body).
    fn request(&self, path: &str, form: Option<&[(&str, &str)]>) -> Result<(u16, String, String), String> {
        let url = if path.starts_with("http") { path.to_string() } else { format!("{}{}", self.base, path) };
        let req = if form.is_some() { self.agent.post(&url) } else { self.agent.get(&url) };
        let req = req.set("Cookie", &self.cookie_header());
        let r = match form {
            Some(f) => req.send_form(f),
            None => req.call(),
        };
        let resp = match r {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => {
                crate::net::log::debug(&format!("site: {t:?}"));
                return Err(format!("Network error: {t}"));
            }
        };
        self.keep_cookies(&resp);
        let status = resp.status();
        let location = resp.header("location").unwrap_or("").to_string();
        let body = resp.into_string().map_err(|e| e.to_string())?;
        Ok((status, location, body))
    }

    pub fn forget(&self) {
        *self.session.lock().unwrap() = Session::default();
    }

    fn login(&self) -> Result<(), String> {
        self.forget();
        let (_, _, front) = self.request("/", None)?;
        let token = between(&front, "name=\"csrf_token\" value=\"", "\"")
            .ok_or("The website's login form has changed; notes and site features are unavailable.")?;
        let pw = self.password.lock().unwrap().clone();
        let form = [("csrf_token", token.as_str()), ("username", &self.account), ("password", &pw), ("remember", "on"), ("redirect", "/")];
        let (status, _, body) = self.request("/action/script_login.php", Some(&form))?;
        if !(300..400).contains(&status) {
            // A failed login shows a "System message" page.
            let msg = between(&body, "id='DisplayedMessage'>", ". ")
                .map(|m| html_to_text(&m))
                .unwrap_or_else(|| "the website did not accept the login".into());
            return Err(format!("Website login failed: {}", msg.trim()));
        }
        self.session.lock().unwrap().logged_in = true;
        Ok(())
    }

    fn ensure_login(&self) -> Result<(), String> {
        if self.session.lock().unwrap().logged_in {
            return Ok(());
        }
        self.login()
    }

    /// A website page as HTML (logs in when needed).
    pub fn page(&self, path: &str) -> Result<String, String> {
        for attempt in 0..2 {
            self.ensure_login()?;
            let (status, _, body) = self.request(path, None)?;
            let logged_out = (300..400).contains(&status) || !body.contains("script_logout.php");
            if logged_out && attempt == 0 {
                self.forget();
                continue;
            }
            if status >= 400 {
                return Err(format!("HTTP {status} for {path}"));
            }
            if let Some(t) = between(&body, "name=\"csrf-token\" content=\"", "\"") {
                self.session.lock().unwrap().csrf = t;
            }
            return Ok(body);
        }
        Err("Not logged in on the website.".into())
    }

    fn csrf(&self) -> Result<String, String> {
        let t = self.session.lock().unwrap().csrf.clone();
        if !t.is_empty() {
            return Ok(t);
        }
        self.page("/messages.php")?;
        let t = self.session.lock().unwrap().csrf.clone();
        if t.is_empty() {
            return Err("The website did not send a csrf token.".into());
        }
        Ok(t)
    }

    /// GET (or POST, with the csrf token) a website JSON endpoint.
    pub fn json(&self, path: &str, params: &[(&str, &str)], post: bool) -> Result<Value, String> {
        let mut last = String::new();
        for _ in 0..2 {
            self.ensure_login()?;
            let r = if post {
                let token = self.csrf()?;
                let mut form: Vec<(&str, &str)> = params.to_vec();
                form.push(("csrf_token", &token));
                self.request(path, Some(&form))?
            } else {
                let q: Vec<String> = params.iter().map(|(k, v)| format!("{}={}", urlenc(k), urlenc(v))).collect();
                let p = if q.is_empty() { path.to_string() } else { format!("{path}?{}", q.join("&")) };
                self.request(&p, None)?
            };
            let (status, _, body) = r;
            let v: Option<Value> = serde_json::from_str(&body).ok().filter(|v: &Value| v.is_object());
            let Some(v) = v else {
                // Probably sent to the login page.
                last = format!("Unexpected answer from the website (HTTP {status}).");
                self.forget();
                continue;
            };
            let err = vs(&v, "error");
            if err.is_empty() {
                return Ok(v);
            }
            let low = err.to_lowercase();
            if low.contains("logged in") || low.contains("token") {
                last = err;
                self.forget();
                continue;
            }
            return Err(err);
        }
        Err(last)
    }

    // ------------------------------------------------------------------
    // Notes

    pub fn note_folders(&self) -> Result<Vec<NoteFolder>, String> {
        let v = self.json("/json/notes-getfolders.json", &[], false)?;
        Ok(v.get("folders")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|f| NoteFolder {
                        id: vi(f, "folder_id"),
                        name: vs(f, "name"),
                        total: vi(f, "notecount"),
                        unread: vi(f, "unreadcount"),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// One page of notes and the total number.
    pub fn notes(&self, list: &NoteList, offset: usize, amount: usize) -> Result<(Vec<NoteSummary>, i64), String> {
        let (off, amt) = (offset.to_string(), amount.to_string());
        let folder;
        let mut params: Vec<(&str, &str)> = vec![("offset", &off), ("amount", &amt)];
        let path = match list {
            NoteList::Folder(f) => {
                folder = f.to_string();
                params.push(("folder", &folder));
                "/json/notes-get.json"
            }
            NoteList::Unread => "/json/notes-getunread.json",
            NoteList::Starred => "/json/notes-getstarred.php",
            NoteList::With(name) => {
                params.push(("name", name));
                "/json/notes-searchuser.json"
            }
            NoteList::Containing(text) => {
                params.push(("query", text));
                "/json/notes-searchcontent.json"
            }
        };
        let v = self.json(path, &params, false)?;
        let notes = v
            .get("notes")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|n| NoteSummary {
                        id: vi(n, "note_id"),
                        title: html_to_text(&vs(n, "title")),
                        from: vs(n, "source_name"),
                        to: vs(n, "dest_name"),
                        sent: vs(n, "datetime_sent"),
                        folder: vi(n, "folder_id"),
                        read: vi(n, "read") != 0,
                        starred: vi(n, "starred") != 0,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok((notes, vi(&v, "total")))
    }

    /// Read a note (this marks it as read on the website).
    pub fn note(&self, id: i64) -> Result<Note, String> {
        let html = self.page(&format!("/view_note.php?note_id={id}"))?;
        let head = between(&html, "id='Note_PageHeader'", "</h2>").ok_or("That note could not be found.")?;
        let title = html_to_text(head.split_once('>').map(|x| x.1).unwrap_or(""));
        let meta = between(&html, "Sent ", "<br/>").unwrap_or_default();
        let mut names = meta.split("/c/").skip(1).map(|s| url_dec(s.split('/').next().unwrap_or("")));
        let from = names.next().unwrap_or_default();
        let to = names.next().unwrap_or_default();
        let sent = html_to_text(meta.split(", by").next().unwrap_or("")).trim().to_string();
        let body = between(&html, "class='FormattedBlock'", "<div class=\"panel StyledForm\"").unwrap_or_default();
        let body = body.split_once('>').map(|x| x.1).unwrap_or("");
        let body = body.trim_end().trim_end_matches("</div>").trim_end().trim_end_matches("</div>");
        // The body is the raw BBCode, HTML-escaped (the site formats it in JavaScript).
        let body = body.replace("\r", "").replace("<br/>", "\n").replace("<br />", "\n").replace("<br>", "\n");
        Ok(Note { id, title, from, to, sent, body: unescape(body.trim()) })
    }

    /// `source_id` is the id of the sending character.
    pub fn note_send(&self, source_id: i64, to: &str, title: &str, message: &str) -> Result<(), String> {
        let src = source_id.to_string();
        let title = if title.trim().is_empty() { "(no title)" } else { title };
        self.json("/json/notes-send.json", &[("title", title), ("message", message), ("dest", to), ("source", &src)], true).map(|_| ())
    }

    fn notes_post(&self, endpoint: &str, ids: &[i64], extra: &[(&str, &str)]) -> Result<(), String> {
        let ids: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        let mut form: Vec<(&str, &str)> = ids.iter().map(|i| ("notes[]", i.as_str())).collect();
        form.extend_from_slice(extra);
        self.json(&format!("/json/{endpoint}"), &form, true).map(|_| ())
    }

    pub fn notes_set_read(&self, ids: &[i64], read: bool, folder: i64) -> Result<(), String> {
        let f = folder.to_string();
        self.notes_post("notes-setread.json", ids, &[("state", if read { "1" } else { "0" }), ("folder_id", &f)])
    }

    pub fn notes_set_starred(&self, ids: &[i64], starred: bool) -> Result<(), String> {
        self.notes_post("notes-setstarred.json", ids, &[("state", if starred { "1" } else { "0" })])
    }

    pub fn notes_trash(&self, ids: &[i64]) -> Result<(), String> {
        self.notes_post("notes-trash.json", ids, &[])
    }

    pub fn notes_move(&self, ids: &[i64], folder: i64) -> Result<(), String> {
        let f = folder.to_string();
        self.notes_post("notes-setfolder.json", ids, &[("folder_id", &f)])
    }

    pub fn note_report(&self, id: i64, reason: &str) -> Result<(), String> {
        let i = id.to_string();
        self.json("/json/notes-report.json", &[("note_id", &i), ("reason", reason)], true).map(|_| ())
    }

    pub fn notes_empty_trash(&self) -> Result<(), String> {
        self.json("/json/notes-emptytrash.json", &[], true).map(|_| ())
    }

    pub fn note_folder_create(&self, name: &str) -> Result<(), String> {
        self.json("/json/notes-createfolder.json", &[("name", name)], true).map(|_| ())
    }

    pub fn note_folder_rename(&self, id: i64, name: &str) -> Result<(), String> {
        let i = id.to_string();
        self.json("/json/notes-renamefolder.json", &[("folder", &i), ("name", name)], true).map(|_| ())
    }

    pub fn note_folder_delete(&self, id: i64) -> Result<(), String> {
        let i = id.to_string();
        self.json("/json/notes-deletefolder.json", &[("folder", &i)], true).map(|_| ())
    }

    // ------------------------------------------------------------------
    // Message center and counters

    pub fn message_center(&self, category: &str, offset: usize) -> Result<MessageCenter, String> {
        let mut path = format!("/messages.php?offset={offset}");
        if !category.is_empty() {
            path.push_str(&format!("&show={}", urlenc(category)));
        }
        let mut mc = parse_message_center(&self.page(&path)?);
        mc.offset = offset;
        mc.next = mc.next.filter(|&n| n > offset);
        Ok(mc)
    }

    pub fn delete_message(&self, id: &str) -> Result<(), String> {
        self.json("/json/deleteMessage.json", &[("message_id", id)], false).map(|_| ())
    }

    pub fn trash_messages(&self, category: &str) -> Result<(), String> {
        self.json("/json/trashMessages.json", &[("type", category)], false).map(|_| ())
    }

    pub fn counters(&self) -> Result<Counters, String> {
        let notes = self.note_folders()?.iter().find(|f| f.id == 1).map(|f| f.unread).unwrap_or(0);
        let messages = self.message_center("", 0)?.total;
        Ok(Counters { notes, messages })
    }

    // ------------------------------------------------------------------
    // News, roleplay ads, groups, guestbook

    pub fn news(&self, offset: usize) -> Result<(Vec<NewsPost>, i64), String> {
        let off = offset.to_string();
        let v = self.json("/json/getNewsposts.json", &[("offset", &off)], true)?;
        let posts = v
            .get("posts")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|p| NewsPost {
                        title: html_to_text(&vs(p, "title")),
                        author: vs(p, "name"),
                        posted: vi(p, "datetime_posted_timestamp"),
                        ago: vs(p, "datetime_posted"),
                        content: unescape(&vs(p, "content").replace("\r\n", "\n")),
                        comments: vi(p, "commentcount"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok((posts, vi(&v, "count")))
    }

    pub fn personals(&self) -> Result<Vec<Personal>, String> {
        let v = self.json("/json/personals-get.json", &[], false)?;
        Ok(v.get("characters")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|p| Personal {
                        name: vs(p, "name"),
                        gender: vs(p, "gender"),
                        orientation: vs(p, "orientation"),
                        role: vs(p, "role"),
                        pref: vs(p, "pref"),
                        age: vs(p, "age"),
                        // The BBCode comes HTML-escaped.
                        message: unescape(&vs(p, "message")),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn profile_groups(&self, character_id: i64) -> Result<Vec<String>, String> {
        let id = character_id.to_string();
        let v = self.json("/json/profile-groups.json", &[("character_id", &id)], true)?;
        Ok(v.get("groups")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(html_to_text).collect())
            .unwrap_or_default())
    }

    // ------------------------------------------------------------------
    // Browse New, groups

    /// New/updated characters, or (with `newest`) recently created ones.
    pub fn browse(&self, newest: bool, offset: usize) -> Result<BrowsePage, String> {
        let kind = if newest { "newest" } else { "edited" };
        let mut p = parse_browse(&self.page(&format!("/character_browse.php?type={kind}&offset={offset}"))?);
        p.offset = offset;
        p.next = p.next.filter(|&n| n > offset);
        Ok(p)
    }

    pub fn groups(&self) -> Result<Vec<GroupSummary>, String> {
        let list = parse_group_list(&self.page("/group_browse.php")?);
        if list.is_empty() {
            return Err("The group list could not be read.".into());
        }
        Ok(list)
    }

    pub fn group(&self, slug: &str) -> Result<Group, String> {
        let html = self.page(&format!("/group.php?group={}", urlenc(slug)))?;
        let mut g = parse_group(&html).ok_or_else(|| page_message(&html, "That group could not be found."))?;
        g.slug = slug.to_string();
        Ok(g)
    }

    pub fn forum(&self, id: i64) -> Result<ForumPage, String> {
        let html = self.page(&format!("/forum.php?forum={id}"))?;
        let mut f = parse_forum(&html).ok_or_else(|| page_message(&html, "That forum could not be found."))?;
        f.id = id;
        Ok(f)
    }

    /// A page of a thread; `offset` None is the last page.
    pub fn thread(&self, id: i64, offset: Option<usize>) -> Result<ThreadPage, String> {
        let off = offset.map(|o| o.to_string()).unwrap_or_else(|| "last".into());
        let html = self.page(&format!("/thread.php?thread={id}&offset={off}"))?;
        let mut t = parse_thread(&html).ok_or_else(|| page_message(&html, "That thread could not be found."))?;
        t.id = id;
        Ok(t)
    }

    pub fn group_join(&self, group_id: i64, character_id: i64, join: bool) -> Result<(), String> {
        let (g, c) = (group_id.to_string(), character_id.to_string());
        let path = if join { "/json/joinGroup.json" } else { "/json/leaveGroup.json" };
        self.json(path, &[("group_id", &g), ("character_id", &c)], true).map(|_| ())
    }

    pub fn post_reply(&self, group_id: i64, forum_id: i64, thread_id: i64, character_id: i64, message: &str) -> Result<(), String> {
        let (g, f, t, c) = (group_id.to_string(), forum_id.to_string(), thread_id.to_string(), character_id.to_string());
        self.form_post(
            "/action/script_create_post.php",
            &[("group_id", &g), ("thread_id", &t), ("forum_id", &f), ("character_id", &c), ("message", message)],
        )
    }

    pub fn post_thread(&self, group_id: i64, forum_id: i64, character_id: i64, subject: &str, message: &str) -> Result<(), String> {
        let (g, f, c) = (group_id.to_string(), forum_id.to_string(), character_id.to_string());
        self.form_post(
            "/action/script_create_thread.php",
            &[("group_id", &g), ("forum_id", &f), ("subject", subject), ("character_id", &c), ("message", message), ("submitpoll", "0")],
        )
    }

    /// Send one of the site's HTML forms. The site answers a successful post
    /// with a redirect, an error with a "System message" page.
    fn form_post(&self, path: &str, params: &[(&str, &str)]) -> Result<(), String> {
        self.ensure_login()?;
        let token = self.csrf()?;
        let mut form: Vec<(&str, &str)> = params.to_vec();
        form.push(("csrf_token", &token));
        let (status, location, body) = self.request(path, Some(&form))?;
        if (300..400).contains(&status) {
            return Ok(());
        }
        if body.contains("class=\"Thread_Post\"") {
            return Ok(());
        }
        crate::net::log::debug(&format!("site: {path} -> HTTP {status} {location}"));
        Err(page_message(&body, &format!("the website answered with HTTP {status}")))
    }

    pub fn guestbook_post(&self, target_id: i64, source_id: i64, private: bool, message: &str) -> Result<(), String> {
        let (t, s) = (target_id.to_string(), source_id.to_string());
        let p = if private { "true" } else { "false" };
        self.json(
            "/json/guestbook-post.json",
            &[("character_id", &t), ("source_character_id", &s), ("private", p), ("message", message)],
            true,
        )
        .map(|_| ())
    }
}

// ----------------------------------------------------------------------
// HTML helpers

/// The text between the first `start` and the following `end`.
pub fn between(s: &str, start: &str, end: &str) -> Option<String> {
    let a = s.find(start)? + start.len();
    let b = s[a..].find(end)? + a;
    Some(s[a..b].to_string())
}

pub fn urlenc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn url_dec(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={s}").as_bytes()).next().map(|(_, v)| v.to_string()).unwrap_or_default()
}

pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // ';' is ASCII, so its byte position is always a char boundary.
        let Some(end) = rest.as_bytes()[..rest.len().min(12)].iter().position(|&b| b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Value of an attribute in a tag's source (`href='x'` or `href="x"`).
fn attr(tag: &str, name: &str) -> Option<String> {
    let low = tag.to_ascii_lowercase();
    let pat = format!("{name}=");
    let i = low.match_indices(&pat).map(|(i, _)| i).find(|&i| i > 0 && low.as_bytes()[i - 1].is_ascii_whitespace())? + pat.len();
    let rest = &tag[i..];
    let q = rest.chars().next()?;
    if q == '"' || q == '\'' {
        rest[1..].find(q).map(|e| unescape(&rest[1..1 + e]))
    } else {
        Some(unescape(rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or("")))
    }
}

/// Website HTML (formatted BBCode) back to BBCode that the chat renderer understands.
pub fn html_to_bbcode(html: &str) -> String {
    let mut out = String::new();
    // Closing BBCode for each open tag we translated, in order.
    let mut stack: Vec<(String, &'static str)> = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&unescape(&rest[..i].replace(['\n', '\r'], "")));
        let Some(e) = rest[i..].find('>') else {
            // A cut-off tag: drop it.
            rest = "";
            break;
        };
        let tag = &rest[i + 1..i + e];
        rest = &rest[i + e + 1..];
        let closing = tag.starts_with('/');
        let name: String =
            tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        if closing {
            if let Some(p) = stack.iter().rposition(|(n, _)| *n == name) {
                for (_, close) in stack.drain(p..).rev() {
                    out.push_str(close);
                }
            }
            if matches!(name.as_str(), "p" | "div" | "li" | "blockquote" | "tr") && !out.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }
        let (open, close): (String, &'static str) = match name.as_str() {
            "br" => {
                out.push('\n');
                continue;
            }
            "hr" => {
                out.push_str("[hr]");
                continue;
            }
            "img" => {
                // Eicons and icons come as images.
                let src = attr(tag, "src").unwrap_or_default();
                if let Some(n) = src.split("/images/eicon/").nth(1) {
                    out.push_str(&format!("[eicon]{}[/eicon]", url_dec(n.trim_end_matches(".gif"))));
                } else if let Some(n) = src.split("/images/avatar/").nth(1) {
                    out.push_str(&format!("[icon]{}[/icon]", url_dec(n.trim_end_matches(".png"))));
                }
                continue;
            }
            "b" | "strong" => ("[b]".into(), "[/b]"),
            "i" | "em" => ("[i]".into(), "[/i]"),
            "u" => ("[u]".into(), "[/u]"),
            "s" | "strike" | "del" => ("[s]".into(), "[/s]"),
            "sup" => ("[sup]".into(), "[/sup]"),
            "sub" => ("[sub]".into(), "[/sub]"),
            "blockquote" => ("[quote]".into(), "[/quote]"),
            "a" => match attr(tag, "href") {
                Some(h) if h.contains("f-list.net/c/") || h.starts_with("c/") || h.starts_with("/c/") => ("[user]".into(), "[/user]"),
                // Relative links (paging, scripts) lead nowhere outside the site.
                Some(h) if h.contains("://") => (format!("[url={h}]"), "[/url]"),
                _ => (String::new(), ""),
            },
            "span" | "font" | "div" => {
                let style = attr(tag, "style").unwrap_or_default().to_lowercase();
                let class = attr(tag, "class").unwrap_or_default().to_lowercase();
                let color = style
                    .split(';')
                    .find_map(|p| p.trim().strip_prefix("color:").map(|c| c.trim().to_string()))
                    .or_else(|| class.split_whitespace().find_map(color_class))
                    .or_else(|| attr(tag, "color"));
                if class.contains("quote") {
                    ("[quote]".into(), "[/quote]")
                } else if let Some(c) = color.filter(|c| !c.is_empty()) {
                    (format!("[color={c}]"), "[/color]")
                } else {
                    (String::new(), "")
                }
            }
            _ => (String::new(), ""),
        };
        out.push_str(&open);
        stack.push((name, close));
    }
    out.push_str(&unescape(&rest.replace(['\n', '\r'], "")));
    for (_, close) in stack.into_iter().rev() {
        out.push_str(close);
    }
    out
}

/// The site writes `[color=red]` as `<span class="redfont">`.
fn color_class(class: &str) -> Option<String> {
    class.strip_suffix("font").filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_alphabetic())).map(String::from)
}

/// Plain text of an HTML fragment.
pub fn html_to_text(html: &str) -> String {
    crate::bbcode::strip(&html_to_bbcode(html))
}

fn count_in(label: &str) -> i64 {
    label.rsplit('(').next().and_then(|s| s.trim_end_matches(')').trim().parse().ok()).unwrap_or(0)
}

/// End (after the closing tag) of the element whose start tag is at `at`.
fn element_end(html: &str, at: usize, name: &str) -> usize {
    let open = format!("<{}", name.to_ascii_lowercase());
    let close = format!("</{}", name.to_ascii_lowercase());
    let low = html.to_ascii_lowercase();
    let Some(e) = low[at..].find('>') else { return html.len() };
    if low[at..at + e].ends_with('/') {
        return at + e + 1;
    }
    let mut depth = 1;
    let mut i = at + e + 1;
    while depth > 0 {
        let o = low[i..]
            .find(&open)
            .map(|x| x + i)
            .filter(|&x| low[x + open.len()..].starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/'));
        let c = low[i..].find(&close).map(|x| x + i);
        match (o, c) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                i = o + open.len();
            }
            (_, Some(c)) => {
                depth -= 1;
                if depth == 0 {
                    // The text up to the closing tag is the inside.
                    return c;
                }
                i = c + close.len();
            }
            _ => return html.len(),
        }
    }
    html.len()
}

/// Drop the "Delete" links from a message.
fn without_delete_links(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut from = 0;
    while let Some(i) = html[from..].find("<a ") {
        let at = from + i;
        let Some(e) = html[at..].find('>') else { break };
        let tag = &html[at..at + e + 1];
        if attr(tag, "class").unwrap_or_default().contains("Messagecenter_Deletelink") {
            out.push_str(&html[from..at]);
            let end = element_end(html, at, "a");
            from = html[end..].find('>').map(|x| end + x + 1).unwrap_or(html.len());
        } else {
            out.push_str(&html[from..at + e + 1]);
            from = at + e + 1;
        }
    }
    out.push_str(&html[from..]);
    out
}

/// Split the site's change record (`V:2 F+:"Kink",fave D>:+120 AU`) into
/// (code, name, value). Names are quoted but may contain quotes and spaces,
/// so the record is split where the next code starts.
fn change_tokens(s: &str) -> Vec<(String, String, String)> {
    // A code: one or two capitals, maybe +, - or >, then ':' and a quote,
    // digit or sign, or a known code standing alone.
    let code_at = |i: usize| -> Option<(String, usize)> {
        let b = s.as_bytes();
        let mut j = i;
        while j < b.len() && j - i < 2 && b[j].is_ascii_uppercase() {
            j += 1;
        }
        if j == i {
            return None;
        }
        if j < b.len() && matches!(b[j], b'+' | b'-' | b'>') {
            j += 1;
        }
        let code = &s[i..j];
        match b.get(j) {
            Some(b':') if b.get(j + 1).is_some_and(|c| *c == b'"' || c.is_ascii_digit() || *c == b'+' || *c == b'-') => {
                Some((code.to_string(), j + 1))
            }
            None | Some(b' ') if matches!(code, "AU" | "V") => Some((code.to_string(), j)),
            _ => None,
        }
    };
    let mut starts: Vec<(usize, String, usize)> = Vec::new();
    for (i, _) in s.char_indices() {
        if i == 0 || s.as_bytes()[i - 1] == b' ' {
            if let Some((code, v)) = code_at(i) {
                starts.push((i, code, v));
            }
        }
    }
    let mut out = Vec::new();
    for (k, (_, code, v)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map(|x| x.0).unwrap_or(s.len());
        let value = s[*v..end.max(*v)].trim();
        let (name, rest) = match (value.strip_prefix('"'), value.rfind('"')) {
            (Some(_), Some(q)) if q > 0 => (value[1..q].to_string(), value[q + 1..].trim_start_matches(',').trim().to_string()),
            _ => (String::new(), value.to_string()),
        };
        out.push((code.clone(), name, rest));
    }
    out
}

/// A readable summary of a character update's change record.
pub fn describe_changes(record: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut kinks: Vec<(&'static str, Vec<String>)> =
        vec![("Fave", Vec::new()), ("Yes", Vec::new()), ("Maybe", Vec::new()), ("No", Vec::new())];
    let mut customs: Vec<String> = Vec::new();
    let mut info: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    for (code, name, rest) in change_tokens(record) {
        match code.as_str() {
            "V" => {}
            "F+" | "F>" => match kinks.iter_mut().find(|k| k.0.eq_ignore_ascii_case(&rest)) {
                Some(k) => k.1.push(name),
                None => other.push(format!("{name} ({rest})")),
            },
            "F-" => other.push(format!("{name} (removed)")),
            "C+" | "C>" | "C-" => {
                let what = match (code.as_str(), rest.is_empty()) {
                    ("C+", _) => "new".to_string(),
                    ("C-", _) => "removed".to_string(),
                    (_, false) => rest.to_lowercase(),
                    _ => "changed".to_string(),
                };
                let name = if name.is_empty() { "(unnamed)".to_string() } else { name };
                customs.push(format!("{name} ({what})"));
            }
            "I>" | "I+" | "I-" => info.push(name),
            "D>" => lines.push(format!("Description changed ({rest} characters)")),
            "AU" => lines.push("New avatar".into()),
            _ => other.push([code, name, rest].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ")),
        }
    }
    for (label, names) in &kinks {
        if !names.is_empty() {
            lines.push(format!("Kinks now {label}: {}", names.join(", ")));
        }
    }
    if !customs.is_empty() {
        lines.push(format!("Custom kinks: {}", customs.join(", ")));
    }
    if !info.is_empty() {
        lines.push(format!("Info: {}", info.join(", ")));
    }
    if !other.is_empty() {
        lines.push(format!("Other: {}", other.join(", ")));
    }
    lines.join("\n")
}

fn digits_after(s: &str, marker: &str) -> Option<String> {
    let i = s.find(marker)? + marker.len();
    let d: String = s[i..].chars().skip_while(|c| *c == '\'' || *c == '"').take_while(|c| c.is_ascii_digit()).collect();
    (!d.is_empty()).then_some(d)
}

pub fn parse_message_center(html: &str) -> MessageCenter {
    let mut mc = MessageCenter {
        total: between(html, "id=\"NavigationMessages\">", "<")
            .and_then(|s| s.split_whitespace().next().and_then(|n| n.parse().ok()))
            .unwrap_or(0),
        ..Default::default()
    };
    for (key, label) in MESSAGE_CATEGORIES {
        let id = if *key == "friends" { "Messagecenter_friends".to_string() } else { format!("Messagecenter_{key}Category") };
        let count = between(html, &format!("id=\"{id}\">"), "</a>").map(|l| count_in(&l)).unwrap_or(0);
        mc.categories.push((key.to_string(), label.to_string(), count));
    }
    mc.shown = between(html, "id=\"VarMessageShow\" value=\"", "\"").unwrap_or_default();
    mc.range = between(html, "<h2>Message center:", "</h2>")
        .and_then(|h| h.rsplit_once('(').map(|x| x.1.trim_end_matches(')').trim().to_string()))
        .filter(|r| r.starts_with("Showing"))
        .unwrap_or_default();
    // Each message is an element with id "Messagecenter_Message<id>" or a
    // class "Messagecenter_Message" / "MessagesGroup<category>"; its details
    // may be in a separate "message-expanded-<id>" element.
    let content = match html.find("id=\"Content\"").or_else(|| html.find("id='Content'")) {
        Some(i) => &html[i..],
        None => html,
    };
    let content = content.split("name=\"bottom-of-page\"").next().unwrap_or(content);
    let mut expanded: Vec<(String, String)> = Vec::new();
    let mut from = 0;
    while let Some(i) = content[from..].find('<') {
        let at = from + i;
        let Some(e) = content[at..].find('>') else { break };
        let tag = &content[at..at + e + 1];
        from = at + 1;
        let name: String = tag[1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if name.is_empty() {
            continue;
        }
        let id = attr(tag, "id").unwrap_or_default();
        let class = attr(tag, "class").unwrap_or_default();
        if class.split_whitespace().any(|c| c == "next") && name.eq_ignore_ascii_case("a") {
            let href = attr(tag, "href").unwrap_or_default();
            mc.next = href.split(['?', '&']).find_map(|p| p.strip_prefix("offset=")).and_then(|n| n.parse().ok());
            continue;
        }
        if let Some(x) = id.strip_prefix("message-expanded-") {
            let end = element_end(content, at, &name);
            expanded.push((x.to_string(), content[at + e + 1..end].to_string()));
            from = end;
            continue;
        }
        let by_id = id.strip_prefix("Messagecenter_Message").filter(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit()));
        let by_class = class.split_whitespace().any(|c| c == "Messagecenter_Message" || c.starts_with("MessagesGroup"));
        if by_id.is_none() && !by_class {
            continue;
        }
        let end = element_end(content, at, &name);
        let inner = without_delete_links(&content[at + e + 1..end]);
        let inner = inner.as_str();
        let msg_id = by_id
            .map(String::from)
            .or_else(|| digits_after(inner, "Messagecenter_Deletelink"))
            .or_else(|| digits_after(inner, "Messages_delete("));
        mc.items.push((msg_id.unwrap_or_default(), inner.to_string()));
        from = end;
    }
    let tidy = |h: &str| {
        let t = html_to_bbcode(h);
        t.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
    };
    for (id, inner) in &mut mc.items {
        let mut text = tidy(inner);
        if let Some((_, more)) = expanded.iter().find(|x| !id.is_empty() && x.0 == *id) {
            let more = tidy(more);
            let more = if more.starts_with("V:") { describe_changes(&more) } else { more };
            if !more.is_empty() {
                text = format!("{text}\n{more}");
            }
        }
        *inner = text;
    }
    mc.items.retain(|(_, t)| !t.is_empty());
    // Not recognised: show the page text rather than nothing.
    if mc.items.is_empty() && !content.contains("There are no messages") {
        let body = content.split_once("</h2>").map(|x| x.1).unwrap_or("");
        let text = tidy(body);
        if !text.is_empty() {
            mc.items.push((String::new(), text));
        }
    }
    mc
}

/// The page's main part (`id="Content"` up to the page bottom).
fn content_part(html: &str) -> &str {
    let content = match html.find("id=\"Content\"").or_else(|| html.find("id='Content'")) {
        Some(i) => &html[i..],
        None => html,
    };
    content.split("name=\"bottom-of-page\"").next().unwrap_or(content)
}

/// The text of a page's "System message" (`DisplayedMessage`), or `default`.
fn page_message(html: &str, default: &str) -> String {
    between(html, "id='DisplayedMessage'>", "<")
        .or_else(|| between(html, "id=\"DisplayedMessage\">", "<"))
        .map(|m| html_to_text(&m).trim().to_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Start tags in `html` from byte `from`: (position, the tag's source).
fn next_tag(html: &str, from: usize) -> Option<(usize, &str)> {
    let mut from = from;
    loop {
        let at = from + html.get(from..)?.find('<')?;
        let e = html[at..].find('>')?;
        let tag = &html[at..at + e + 1];
        if tag[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            return Some((at, tag));
        }
        from = at + 1;
    }
}

fn tag_name(tag: &str) -> String {
    tag[1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase()
}

/// Elements whose class list contains `class` (not looking inside a match):
/// (start tag, inner HTML).
fn elements_with_class<'a>(html: &'a str, class: &str) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((at, tag)) = next_tag(html, from) {
        from = at + 1;
        if !attr(tag, "class").unwrap_or_default().split_whitespace().any(|c| c == class) {
            continue;
        }
        let end = element_end(html, at, &tag_name(tag));
        out.push((tag, &html[(at + tag.len()).min(end)..end]));
        from = end.max(at + 1);
    }
    out
}

/// Inner HTML of the element with this id.
fn element_by_id<'a>(html: &'a str, id: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some((at, tag)) = next_tag(html, from) {
        from = at + 1;
        if attr(tag, "id").as_deref() == Some(id) {
            let end = element_end(html, at, &tag_name(tag));
            return Some(&html[(at + tag.len()).min(end)..end]);
        }
    }
    None
}

/// Links: (href, inner HTML).
fn anchors(html: &str) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((at, tag)) = next_tag(html, from) {
        from = at + 1;
        if tag_name(tag) != "a" {
            continue;
        }
        let end = element_end(html, at, "a");
        out.push((attr(tag, "href").unwrap_or_default(), &html[(at + tag.len()).min(end)..end]));
        from = end.max(at + 1);
    }
    out
}

/// The value of a form field `<input name="...">`.
fn input_value(html: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some((at, tag)) = next_tag(html, from) {
        from = at + 1;
        if tag_name(tag) == "input" && attr(tag, "name").as_deref() == Some(name) {
            return attr(tag, "value");
        }
    }
    None
}

/// Options of the `<select>` with this id: (value, text).
fn select_options(html: &str, id: &str) -> Vec<(String, String)> {
    let Some(i) = html.find(&format!("id=\"{id}\"")).or_else(|| html.find(&format!("id='{id}'"))) else { return Vec::new() };
    let sel = &html[i..];
    let sel = &sel[..sel.find("</select>").unwrap_or(sel.len())];
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((at, tag)) = next_tag(sel, from) {
        from = at + 1;
        if tag_name(tag) != "option" {
            continue;
        }
        let rest = &sel[at + tag.len()..];
        let text = &rest[..rest.find('<').unwrap_or(rest.len())];
        let value = attr(tag, "value").unwrap_or_default();
        if !value.is_empty() {
            out.push((value, html_to_text(text).trim().to_string()));
        }
    }
    out
}

/// A `FormattedBlock`: the raw BBCode, HTML-escaped.
fn bbcode_block(inner: &str) -> String {
    let mut s = inner.replace('\r', "");
    for br in ["<br />", "<br/>", "<br>"] {
        s = s.replace(&format!("{br}\n"), "\n").replace(br, "\n");
    }
    unescape(s.trim())
}

/// The first number after `marker`.
fn number_after(s: &str, marker: &str) -> i64 {
    let Some(i) = s.find(marker) else { return 0 };
    let d: String = s[i + marker.len()..].trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
    d.parse().unwrap_or(0)
}

/// The `offset=` of the page's "next" link.
fn next_offset(html: &str) -> Option<usize> {
    let mut from = 0;
    while let Some((at, tag)) = next_tag(html, from) {
        from = at + 1;
        if tag_name(tag) == "a" && attr(tag, "class").unwrap_or_default().split_whitespace().any(|c| c == "next") {
            let href = attr(tag, "href").unwrap_or_default();
            return href.split(['?', '&']).find_map(|p| p.strip_prefix("offset=")).and_then(|n| n.parse().ok());
        }
    }
    None
}

/// The `name=` value of an address like `x.php?name=value&...`.
fn query_value(href: &str, name: &str) -> Option<String> {
    let q = href.split_once('?')?.1;
    q.split('&').find_map(|p| p.strip_prefix(&format!("{name}="))).map(url_dec)
}

/// Plain text of an element, with its lines tidied.
fn tidy_text(html: &str) -> Vec<String> {
    html_to_text(html).lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect()
}

pub fn parse_browse(html: &str) -> BrowsePage {
    let content = content_part(html);
    let mut page = BrowsePage { next: next_offset(content), ..Default::default() };
    for (tag, inner) in elements_with_class(content, "ListedCharacter") {
        let name = elements_with_class(inner, "ResultListingName")
            .first()
            .map(|(_, n)| html_to_text(n).trim().to_string())
            .filter(|n| !n.is_empty())
            .or_else(|| attr(tag, "href").map(|h| url_dec(h.trim_start_matches('/').trim_start_matches("c/").trim_end_matches('/'))));
        let Some(name) = name else { continue };
        let info = elements_with_class(inner, "CharacterBrowseInfo")
            .into_iter()
            .filter_map(|(_, i)| {
                let key = html_to_text(&between(i, "<strong>", "</strong>")?).trim().trim_end_matches(':').trim().to_string();
                let value = html_to_text(i.split_once("</strong>")?.1).trim().to_string();
                (!key.is_empty() && !value.is_empty()).then_some((key, value))
            })
            .collect();
        page.characters.push(BrowseCharacter { name, info });
    }
    page
}

pub fn parse_group_list(html: &str) -> Vec<GroupSummary> {
    let joined: Vec<String> = select_options(html, "GroupJumpLabel").into_iter().map(|(v, _)| v.to_lowercase()).collect();
    let content = content_part(html);
    let mut out: Vec<GroupSummary> = Vec::new();
    for (_, inner) in elements_with_class(content, "ListedGroup") {
        let links = anchors(inner);
        let Some(slug) = links.iter().find_map(|(h, _)| query_value(h, "group")) else { continue };
        let name = links
            .iter()
            .filter(|(h, _)| h.contains("group.php?group="))
            .map(|(_, t)| html_to_text(t).trim().to_string())
            .find(|t| !t.is_empty())
            .unwrap_or_else(|| slug.clone());
        let description = elements_with_class(inner, "FormattedBlock").first().map(|(_, d)| bbcode_block(d)).unwrap_or_default();
        let stats = elements_with_class(inner, "ListedGroupStats").first().map(|(_, s)| *s).unwrap_or("");
        let owner = between(stats, "Owner:", "<br").map(|o| html_to_text(&o).trim().to_string()).unwrap_or_default();
        let last_post = between(stats, "Last post:", "<br").map(|o| html_to_text(&o).trim().to_string()).unwrap_or_default();
        out.push(GroupSummary {
            joined: joined.contains(&slug.to_lowercase()),
            slug,
            name,
            description,
            owner,
            posts: number_after(stats, "Posts:"),
            members: number_after(stats, "Members:"),
            last_post,
        });
    }
    // Your groups first.
    out.sort_by_key(|g| !g.joined);
    out
}

/// Own characters in a `<select>` of the page: (id, name).
fn character_options(html: &str, id: &str) -> Vec<(i64, String)> {
    select_options(html, id).into_iter().filter_map(|(v, n)| Some((v.parse().ok()?, n))).collect()
}

pub fn parse_group(html: &str) -> Option<Group> {
    let content = content_part(html);
    let header = element_by_id(content, "GroupHeader")?;
    let name = between(header, "<h3", "</h3>").and_then(|h| h.split_once('>').map(|x| html_to_text(x.1).trim().to_string()));
    let id = anchors(header)
        .into_iter()
        .find_map(|(h, _)| query_value(&h, "group_id"))
        .or_else(|| input_value(content, "group_id"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let forums = elements_with_class(content, "GroupForum")
        .into_iter()
        .filter_map(|(_, inner)| {
            let links = anchors(inner);
            let (href, text) = links.iter().find(|(h, _)| h.contains("forum.php?forum="))?;
            let id = query_value(href, "forum")?.parse().ok()?;
            // The statistics box comes before the forum's link.
            let stats_html = &inner[..inner.find("forum.php?forum=").unwrap_or(0)];
            let stats = tidy_text(stats_html);
            Some(Forum {
                id,
                name: html_to_text(text).trim().to_string(),
                description: elements_with_class(inner, "FormattedBlock").first().map(|(_, d)| bbcode_block(d)).unwrap_or_default(),
                stats: stats.first().cloned().unwrap_or_default(),
                last: stats.iter().skip(1).cloned().collect::<Vec<_>>().join(" "),
            })
        })
        .collect();
    Some(Group {
        slug: String::new(),
        id,
        name: name.unwrap_or_default(),
        description: elements_with_class(header, "FormattedBlock").first().map(|(_, d)| bbcode_block(d)).unwrap_or_default(),
        forums,
        joined: character_options(content, "JoinedUsersList"),
        not_joined: character_options(content, "NotJoinedUsersList"),
    })
}

/// The group (slug, name) and forum (id, name) links of a forum or thread header.
fn header_links(header: &str) -> ((String, String), (i64, String)) {
    let mut group = (String::new(), String::new());
    let mut forum = (0, String::new());
    for (href, text) in anchors(header) {
        let text = html_to_text(text).trim().to_string();
        if let Some(slug) = query_value(&href, "group") {
            if group.1.is_empty() {
                group = (slug, text);
            }
        } else if let Some(id) = query_value(&href, "forum").and_then(|f| f.parse().ok()) {
            forum = (id, text);
        }
    }
    (group, forum)
}

pub fn parse_forum(html: &str) -> Option<ForumPage> {
    let content = content_part(html);
    let header = element_by_id(content, "GroupHeader")?;
    let ((group_slug, group_name), _) = header_links(header);
    let name = between(header, "<h3", "</h3>").and_then(|h| h.split_once('>').map(|x| html_to_text(x.1).trim().to_string()));
    let threads = elements_with_class(content, "Forum_Thread")
        .into_iter()
        .filter_map(|(_, inner)| {
            let links = anchors(inner);
            let (href, title) = links.iter().find(|(h, _)| h.contains("thread.php?thread="))?;
            let id = query_value(href, "thread")?.parse().ok()?;
            let author =
                links.iter().find(|(h, _)| h.starts_with("c/") || h.contains("/c/")).map(|(_, t)| html_to_text(t).trim().to_string());
            let created = inner.rsplit_once("</a>").map(|x| tidy_text(x.1).join(" ")).unwrap_or_default();
            Some(ThreadSummary {
                id,
                title: html_to_text(title).trim().to_string(),
                author: author.unwrap_or_default(),
                posts: between(inner, "Forum_ThreadPostCount\">", "<").and_then(|n| n.trim().parse().ok()).unwrap_or(0),
                created,
                last_post: between(inner, "Forum_LastPostInfo\">", "<").map(|t| html_to_text(&t).trim().to_string()).unwrap_or_default(),
                locked: inner.contains("Forum_LockedThread"),
                poll: inner.contains("Forum_Poll"),
            })
        })
        .collect();
    Some(ForumPage {
        id: 0,
        name: name.unwrap_or_default(),
        group_id: input_value(content, "group_id").and_then(|v| v.parse().ok()).unwrap_or(0),
        group_slug,
        group_name,
        threads,
        can_post: content.contains("script_create_thread.php"),
    })
}

pub fn parse_thread(html: &str) -> Option<ThreadPage> {
    let content = content_part(html);
    let header = element_by_id(content, "GroupHeader")?;
    let ((group_slug, group_name), (forum_id, forum_name)) = header_links(header);
    let subject = element_by_id(header, "Thread_Subject").map(|s| html_to_text(s).trim().to_string()).unwrap_or_default();
    let posts = elements_with_class(content, "Thread_Post")
        .into_iter()
        .map(|(tag, inner)| {
            let id = attr(tag, "id").and_then(|i| i.trim_start_matches("Thread_Post").parse().ok()).unwrap_or(0);
            let meta = &inner[..inner.find("FormattedBlock").unwrap_or(inner.len())];
            let after = meta.split_once("Posted ").map(|x| x.1).unwrap_or("");
            let posted = after.split(" by ").next().map(|p| html_to_text(p).trim().to_string()).unwrap_or_default();
            let author = after
                .split_once(" by ")
                .and_then(|x| anchors(x.1).into_iter().next())
                .map(|(_, t)| html_to_text(t).trim().to_string())
                .unwrap_or_default();
            let body = elements_with_class(inner, "FormattedBlock").first().map(|(_, b)| bbcode_block(b)).unwrap_or_default();
            ForumPost { id, author, posted, body }
        })
        .collect();
    let mut pages: Vec<usize> = Vec::new();
    let mut offset = 0;
    if let Some((_, nav)) = elements_with_class(content, "Thread_Navigation").first() {
        let mut from = 0;
        while let Some((at, tag)) = next_tag(nav, from) {
            from = at + 1;
            if tag_name(tag) != "a" {
                continue;
            }
            let rest = &nav[at + tag.len()..];
            let label = rest[..rest.find('<').unwrap_or(rest.len())].trim();
            let Some(off) = attr(tag, "href").and_then(|h| query_value(&h, "offset")).and_then(|o| o.parse::<usize>().ok()) else {
                continue;
            };
            if !label.is_empty() && label.chars().all(|c| c.is_ascii_digit()) {
                pages.push(off);
                if attr(tag, "class").unwrap_or_default().contains("Thread_ActivePage") {
                    offset = off;
                }
            }
        }
    }
    pages.sort_unstable();
    pages.dedup();
    let reply = content.find("script_create_post.php").map(|i| &content[i..]).unwrap_or("");
    Some(ThreadPage {
        id: 0,
        subject,
        group_id: input_value(reply, "group_id").and_then(|v| v.parse().ok()).unwrap_or(0),
        group_slug,
        group_name,
        forum_id: input_value(reply, "forum_id").and_then(|v| v.parse().ok()).unwrap_or(forum_id),
        forum_name,
        posts,
        pages,
        offset,
        can_reply: !reply.is_empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_conversion() {
        assert_eq!(unescape("&quot;Why&#039;d&quot; &amp; &#x41;"), "\"Why'd\" & A");
        assert_eq!(unescape("a &𝐈𝐈𝐈𝐈 b; &amp;"), "a &𝐈𝐈𝐈𝐈 b; &");
        let html = "Hi <b>there</b><br/><a href='https://www.f-list.net/c/Some%20One/'>Some One</a> \
                    <span class=\"redfont\">red</span> <a href=\"https://x.y/?a=1&amp;b=2\">link</a>\
                    <img src='https://static.f-list.net/images/eicon/wave.gif'/>";
        assert_eq!(
            html_to_bbcode(html),
            "Hi [b]there[/b]\n[user]Some One[/user] [color=red]red[/color] [url=https://x.y/?a=1&b=2]link[/url][eicon]wave[/eicon]"
        );
        assert_eq!(html_to_text("<i>a</i> &lt;b&gt;"), "a <b>");
    }

    #[test]
    fn message_center_page() {
        let html = r#"<a href="https://www.f-list.net/messages.php" class="ImageLink" id="NavigationMessages">3 Messages</a>
            <a href="?show=friends" id="Messagecenter_friends">Friend requests (1)</a>
            <a href="?show=guestbookpost" id="Messagecenter_guestbookpostCategory">Guestbook posts (2)</a>
            <td id="Content"><input type="hidden" id="VarMessageShow" value="guestbookpost"/>
            <div class="Messagecenter_Message" id="Messagecenter_Message77"><a href='https://www.f-list.net/c/Bob/'>Bob</a> posted in your guestbook.<br/>2h ago</div>
            <div id="Messagecenter_Message78">Second</div></td><a name="bottom-of-page"></a>"#;
        let mc = parse_message_center(html);
        assert_eq!(mc.total, 3);
        assert_eq!(mc.categories.iter().find(|c| c.0 == "friends").unwrap().2, 1);
        assert_eq!(mc.categories.iter().find(|c| c.0 == "guestbookpost").unwrap().2, 2);
        assert_eq!(mc.shown, "guestbookpost");
        assert_eq!(
            mc.items,
            vec![("77".into(), "[user]Bob[/user] posted in your guestbook.\n2h ago".into()), ("78".into(), "Second".into())]
        );
    }

    #[test]
    fn message_center_variants() {
        // Single quotes, a class-only item with a delete link, nested markup
        // and separate expanded details.
        let html = r#"<td id='Content'><h2>Message center: Tracking updates</h2>
            <div class='Messagecenter_Message MessagesGroupcharacterupdate' id='Messagecenter_Message5'>
              <div class='Left'><a href='https://www.f-list.net/c/Ann/'>Ann</a> updated their profile.</div><div>1d ago</div></div>
            <div id='message-expanded-5' class='message-expanded'>Changed: <b>Description</b></div>
            <table><tr class="MessagesGroupcharacterupdate"><td><a id="Messagecenter_Deletelink9" onclick="FList.Messages_delete('9');">x</a></td>
              <td><a href="https://www.f-list.net/c/Bo%20B/">Bo B</a> updated</td></tr></table>
            </td><a name="bottom-of-page"></a>"#;
        let mc = parse_message_center(html);
        assert_eq!(
            mc.items,
            vec![
                ("5".into(), "[user]Ann[/user] updated their profile.\n1d ago\nChanged: [b]Description[/b]".into()),
                ("9".into(), "x [user]Bo B[/user] updated".into()),
            ]
        );
        // An unknown layout: the page text is shown instead of nothing.
        let mc = parse_message_center(r#"<td id="Content"><h2>X</h2><p>Ann updated</p></td>"#);
        assert_eq!(mc.items, vec![(String::new(), "Ann updated".into())]);
        let mc = parse_message_center(
            r#"<td id="Content"><h2>X</h2><div class="DisplayedMessage">There are no messages of this category to display.</div></td>"#,
        );
        assert!(mc.items.is_empty());
    }

    #[test]
    fn character_updates_page() {
        // The layout of the real page (names made up).
        let html = r#"<td id="Content">
            <a class="ImageLink Messagecenter_Trashall" onclick="FList.Messages_trash();">Trash all Tracking updates</a>
            <h2>Message center: Tracking updates (Showing message 1 to 20)</h2>
            <input type="hidden" id="VarMessageShow" value="characterupdate"/>
            <table class='content-navigation'><tr><td><a class='prev' href="?offset=0&show=characterupdate">Previous 20</a></td>
            <td><a class='next' href="?offset=20&show=characterupdate">Next 20</a></td></tr></table>
            <div class='Messagecenter_Message MessagesGroupcharacterupdate' id='Messagecenter_Message101'><a class='ImageLink Messagecenter_Deletelink' id='Messagecenter_Deletelink101' onclick='FList.Messages_delete(101);'>Delete</a><span class='message-summary'><a onclick="$('#message-expanded-101').slideDown('fast');">"New kinks &amp; stuff", 12h, 51m ago</a> by <a href='c/some one/'>Some One</a></span></div><div class='message-expanded FormattedBlock' id='message-expanded-101' style='display:none;'>V:2 F+:&quot;Tails&quot;,fave F&gt;:&quot;Horns&quot;,yes F+:&quot;Sweat&quot;,no C+:&quot;&quot;Good Dog!&quot;&quot; C&gt;:&quot;Being a dom&quot;,yes I&gt;:&quot;Height/Length&quot; D&gt;:+192 AU</div>
            <div class='Messagecenter_Message MessagesGroupcharacterupdate' id='Messagecenter_Message102'><a class='ImageLink Messagecenter_Deletelink' id='Messagecenter_Deletelink102' onclick='FList.Messages_delete(102);'>Delete</a><span class='message-summary'><a onclick="x">"expanded gallery", 2w ago</a> by <a href='c/other/'>Other</a></span></div><div class='message-expanded FormattedBlock' id='message-expanded-102' style='display:none;'>V:2</div>
            </td><a name="bottom-of-page"></a>"#;
        let mc = parse_message_center(html);
        assert_eq!(mc.next, Some(20));
        assert_eq!(mc.range, "Showing message 1 to 20");
        assert_eq!(
            mc.items,
            vec![
                (
                    "101".into(),
                    "\"New kinks & stuff\", 12h, 51m ago by [user]Some One[/user]\n\
                     Description changed (+192 characters)\nNew avatar\nKinks now Fave: Tails\nKinks now Yes: Horns\n\
                     Kinks now No: Sweat\nCustom kinks: \"Good Dog!\" (new), Being a dom (yes)\nInfo: Height/Length"
                        .into()
                ),
                ("102".into(), "\"expanded gallery\", 2w ago by [user]Other[/user]".into()),
            ]
        );
    }

    /// Runs against the real website with a test account:
    /// FLIST_TEST_ACCOUNT=… FLIST_TEST_PASSWORD=… cargo test -- --ignored site_live
    #[test]
    #[ignore]
    fn site_live() {
        let (Ok(a), Ok(p)) = (std::env::var("FLIST_TEST_ACCOUNT"), std::env::var("FLIST_TEST_PASSWORD")) else { return };
        let site = Site::new(crate::config::DEFAULT_API, &a, &p);
        let folders = site.note_folders().unwrap();
        assert!(folders.iter().any(|f| f.id == 1), "{folders:?}");
        let (notes, total) = site.notes(&NoteList::Folder(1), 0, 10).unwrap();
        println!("inbox: {total} {notes:?}");
        if let Some(n) = notes.first() {
            println!("{:?}", site.note(n.id).unwrap());
        }
        println!("{:?}", site.counters().unwrap());
        let (news, count) = site.news(0).unwrap();
        assert!(!news.is_empty() && count > 0);
        assert!(!site.personals().unwrap().is_empty());
        println!("{:?}", site.message_center("", 0).unwrap());
        // Browse New and groups (reading only).
        for newest in [false, true] {
            let b = site.browse(newest, 0).unwrap();
            assert!(b.characters.len() >= 10 && b.next == Some(BROWSE_PER_PAGE), "{b:?}");
            assert!(b.characters.iter().any(|c| !c.info.is_empty()), "{b:?}");
        }
        let groups = site.groups().unwrap();
        assert!(groups.len() > 50 && groups.iter().any(|g| g.slug == "f-chat"), "{}", groups.len());
        let g = site.group("f-chat").unwrap();
        assert!(g.id > 0 && !g.forums.is_empty() && !(g.joined.is_empty() && g.not_joined.is_empty()), "{g:?}");
        let f = site.forum(g.forums[0].id).unwrap();
        assert!(!f.threads.is_empty() && f.group_slug == "f-chat", "{f:?}");
        let t = site.thread(f.threads[0].id, Some(0)).unwrap();
        assert!(!t.posts.is_empty() && !t.subject.is_empty() && !t.pages.is_empty(), "{t:?}");
        assert!(t.posts.iter().all(|p| !p.author.is_empty() && !p.posted.is_empty()), "{t:?}");
        let last = site.thread(f.threads[0].id, None).unwrap();
        assert_eq!(last.offset, *last.pages.last().unwrap());
        println!(
            "groups ok: {} groups, forum {} with {} threads, thread \"{}\" {} posts",
            groups.len(),
            f.name,
            f.threads.len(),
            t.subject,
            t.posts.len()
        );
        assert!(site.group("no such group 12345").is_err());
        let bad = Site::new(crate::config::DEFAULT_API, &a, "wrong password");
        println!("bad login: {:?}", bad.note_folders().map(|_| ()));
    }
}

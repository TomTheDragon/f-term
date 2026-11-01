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
        let bad = Site::new(crate::config::DEFAULT_API, &a, "wrong password");
        println!("bad login: {:?}", bad.note_folders().map(|_| ()));
    }
}

//! F-List JSON API (https://www.f-list.net/json/...).
//!
//! All calls are blocking; the UI runs them on worker threads.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::protocol::{vi, vs};

#[derive(Debug, Clone, Default)]
pub struct LoginInfo {
    pub ticket: String,
    pub characters: Vec<String>,
    pub default_character: String,
    /// (my character, their character)
    pub friends: Vec<(String, String)>,
    pub bookmarks: Vec<String>,
}

#[derive(Clone)]
pub struct Api {
    agent: ureq::Agent,
    base: String,
    pub account: String,
    password: Arc<Mutex<String>>,
    ticket: Arc<Mutex<Option<(String, Instant)>>>,
    /// Character name (lowercase) -> id, from the login and profile lookups.
    ids: Arc<Mutex<HashMap<String, i64>>>,
    /// The website session (notes and other site features).
    pub site: crate::site::Site,
}

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .tls_connector(Arc::new(crate::tls::UreqTls))
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        .user_agent(&format!("{}/{}", crate::protocol::CLIENT_NAME, crate::protocol::CLIENT_VERSION))
        .build()
}

fn err_text(e: ureq::Error) -> String {
    // The full error, with its cause chain, for `--debug`.
    crate::net::log::debug(&format!("HTTP error: {e:?}"));
    match e {
        ureq::Error::Status(code, r) => format!("HTTP {code} {}", r.status_text()),
        ureq::Error::Transport(t) => format!("Network error: {t}"),
    }
}

/// Fetch a URL as bytes (used for images).
pub fn get_bytes(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let resp = agent.get(url).call().map_err(err_text)?;
    let mut buf = Vec::new();
    resp.into_reader().take(limit).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

impl Api {
    pub fn new(base: &str, account: &str, password: &str) -> Api {
        Api {
            agent: agent(),
            base: base.trim_end_matches('/').to_string(),
            account: account.to_string(),
            password: Arc::new(Mutex::new(password.to_string())),
            ticket: Arc::new(Mutex::new(None)),
            ids: Arc::new(Mutex::new(HashMap::new())),
            site: crate::site::Site::new(base, account, password),
        }
    }

    fn post_raw(&self, path: &str, form: &[(&str, &str)]) -> Result<Value, String> {
        let url = format!("{}{}", self.base, path);
        let resp = self.agent.post(&url).send_form(form).map_err(err_text)?;
        let text = resp.into_string().map_err(|e| e.to_string())?;
        if text.trim().is_empty() {
            return Ok(Value::Object(Default::default()));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|_| format!("Unexpected reply from {path}: {}", text.chars().take(200).collect::<String>()))?;
        let err = vs(&v, "error");
        if !err.is_empty() {
            return Err(err);
        }
        Ok(v)
    }

    /// Log in with the account password and get a fresh ticket.
    pub fn login(&self) -> Result<LoginInfo, String> {
        let pw = self.password.lock().unwrap().clone();
        let v =
            self.post_raw("/json/getApiTicket.php", &[("account", &self.account), ("password", &pw), ("new_character_list", "true")])?;
        let ticket = vs(&v, "ticket");
        if ticket.is_empty() {
            return Err("Login failed: no ticket received".into());
        }
        *self.ticket.lock().unwrap() = Some((ticket.clone(), Instant::now()));

        // characters is either a list of names or (new format) a name -> id map.
        let mut characters: Vec<(String, i64)> = match v.get("characters") {
            Some(Value::Object(m)) => m.iter().map(|(k, id)| (k.clone(), id.as_i64().unwrap_or(0))).collect(),
            Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|s| (s.to_string(), 0)).collect(),
            _ => Vec::new(),
        };
        characters.sort_by_key(|a| a.0.to_lowercase());
        self.ids.lock().unwrap().extend(characters.iter().filter(|c| c.1 > 0).map(|c| (c.0.to_lowercase(), c.1)));
        let default_character = match v.get("default_character") {
            Some(Value::Number(n)) => {
                let id = n.as_i64().unwrap_or(-1);
                characters.iter().find(|c| c.1 == id).map(|c| c.0.clone()).unwrap_or_default()
            }
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        let friends = v
            .get("friends")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|f| (vs(f, "dest_name"), vs(f, "source_name"))).collect())
            .unwrap_or_default();
        let bookmarks = v.get("bookmarks").and_then(Value::as_array).map(|a| a.iter().map(|b| vs(b, "name")).collect()).unwrap_or_default();
        Ok(LoginInfo { ticket, characters: characters.into_iter().map(|c| c.0).collect(), default_character, friends, bookmarks })
    }

    /// A ticket that is still valid, fetching a new one when needed.
    pub fn ticket(&self) -> Result<String, String> {
        if let Some((t, at)) = self.ticket.lock().unwrap().as_ref() {
            if at.elapsed() < Duration::from_secs(25 * 60) {
                return Ok(t.clone());
            }
        }
        Ok(self.login()?.ticket)
    }

    pub fn invalidate_ticket(&self) {
        *self.ticket.lock().unwrap() = None;
    }

    /// Authenticated API call; retries once with a fresh ticket.
    pub fn call(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<Value, String> {
        for attempt in 0..2 {
            let ticket = self.ticket()?;
            let mut form: Vec<(&str, &str)> = vec![("account", &self.account), ("ticket", &ticket)];
            form.extend_from_slice(params);
            match self.post_raw(&format!("/json/api/{endpoint}"), &form) {
                Err(e) if attempt == 0 && e.to_lowercase().contains("ticket") => self.invalidate_ticket(),
                r => return r,
            }
        }
        unreachable!()
    }

    pub fn character_data(&self, name: &str) -> Result<Value, String> {
        self.call("character-data.php", &[("name", name)])
    }

    pub fn remember_id(&self, name: &str, id: i64) {
        if id > 0 {
            self.ids.lock().unwrap().insert(name.to_lowercase(), id);
        }
    }

    /// A character's id (needed by some website calls).
    pub fn character_id(&self, name: &str) -> Result<i64, String> {
        if let Some(id) = self.ids.lock().unwrap().get(&name.to_lowercase()) {
            return Ok(*id);
        }
        let id = vi(&self.character_data(name)?, "id");
        if id <= 0 {
            return Err(format!("Could not find the id of {name}."));
        }
        self.ids.lock().unwrap().insert(name.to_lowercase(), id);
        Ok(id)
    }

    pub fn character_friends(&self, name: &str) -> Result<Vec<String>, String> {
        let v = self.call("character-friends.php", &[("name", name)])?;
        Ok(v.get("friends").and_then(Value::as_array).map(|a| a.iter().map(|f| vs(f, "name")).collect()).unwrap_or_default())
    }

    pub fn guestbook(&self, name: &str, page: u32) -> Result<Value, String> {
        self.call("character-guestbook.php", &[("name", name), ("page", &page.to_string())])
    }

    pub fn memo_get(&self, name: &str) -> Result<String, String> {
        let v = self.call("character-memo-get2.php", &[("target", name)])?;
        Ok(vs(&v, "note"))
    }

    pub fn memo_save(&self, name: &str, note: &str) -> Result<(), String> {
        self.call("character-memo-save.php", &[("target_name", name), ("note", note)]).map(|_| ())
    }

    pub fn bookmark_add(&self, name: &str) -> Result<(), String> {
        self.call("bookmark-add.php", &[("name", name)]).map(|_| ())
    }

    pub fn bookmark_remove(&self, name: &str) -> Result<(), String> {
        self.call("bookmark-remove.php", &[("name", name)]).map(|_| ())
    }

    pub fn friend_remove(&self, mine: &str, theirs: &str) -> Result<(), String> {
        self.call("friend-remove.php", &[("source_name", mine), ("dest_name", theirs)]).map(|_| ())
    }

    pub fn friend_request(&self, mine: &str, theirs: &str) -> Result<(), String> {
        self.call("request-send2.php", &[("source", mine), ("target", theirs)]).map(|_| ())
    }

    pub fn request_action(&self, action: &str, id: &str) -> Result<(), String> {
        let endpoint = match action {
            "accept" => "request-accept.php",
            "deny" => "request-deny.php",
            _ => "request-cancel.php",
        };
        self.call(endpoint, &[("request_id", id)]).map(|_| ())
    }

    pub fn friend_lists(&self) -> Result<FriendLists, String> {
        let v = self.call(
            "friend-bookmark-lists.php",
            &[("bookmarklist", "true"), ("friendlist", "true"), ("requestlist", "true"), ("requestpending", "true")],
        )?;
        let reqs = |key: &str| -> Vec<FriendRequest> {
            v.get(key)
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|r| FriendRequest { id: vs(r, "id"), source: vs(r, "source"), dest: vs(r, "dest") }).collect())
                .unwrap_or_default()
        };
        Ok(FriendLists {
            bookmarks: v
                .get("bookmarklist")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            friends: v
                .get("friendlist")
                .and_then(Value::as_array)
                // "source" is your character, "dest" the friend (the login
                // ticket's list has it the other way round).
                .map(|a| a.iter().map(|f| (vs(f, "dest"), vs(f, "source"), vi(f, "last_online"))).collect())
                .unwrap_or_default(),
            incoming: reqs("requestlist"),
            pending: reqs("requestpending"),
        })
    }

    /// Upload a chat log for a report; returns the log id.
    pub fn report_submit(&self, my_character: &str, channel: &str, reported: &str, text: &str, log: &str) -> Result<i64, String> {
        let v = self.call(
            "report-submit.php",
            &[
                ("character", my_character),
                ("channel", channel),
                ("reportUser", reported),
                ("reportText", text),
                ("log", log),
                ("text", "true"),
            ],
        )?;
        Ok(vi(&v, "log_id"))
    }

    pub fn mapping_list(&self) -> Result<Mapping, String> {
        let path = crate::paths::cache_dir().join("mapping-list.json");
        let fresh = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().map(|e| e < Duration::from_secs(24 * 3600)).unwrap_or(false))
            .unwrap_or(false);
        let v: Value = match (fresh, std::fs::read_to_string(&path)) {
            (true, Ok(s)) => serde_json::from_str(&s).map_err(|e| e.to_string())?,
            _ => {
                let v = self.post_raw("/json/api/mapping-list.php", &[])?;
                let _ = std::fs::write(&path, v.to_string());
                v
            }
        };
        Ok(Mapping::from_json(&v))
    }

    /// Search fields for the FKS character search.
    pub fn search_fields(&self) -> Result<Value, String> {
        let url = format!("{}/json/chat-search-getfields.json?ids=true", self.base);
        let text = self.agent.get(&url).call().map_err(err_text)?.into_string().map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Default)]
pub struct FriendRequest {
    pub id: String,
    pub source: String,
    pub dest: String,
}

#[derive(Debug, Clone, Default)]
pub struct FriendLists {
    pub bookmarks: Vec<String>,
    /// (their character, my character, last online)
    pub friends: Vec<(String, String, i64)>,
    pub incoming: Vec<FriendRequest>,
    pub pending: Vec<FriendRequest>,
}

/// Lookup tables for kinks and profile info fields.
#[derive(Debug, Clone, Default)]
pub struct Mapping {
    pub kinks: HashMap<String, (String, String, String)>, // id -> (name, description, group id)
    pub kink_groups: HashMap<String, String>,
    pub infotags: HashMap<String, (String, String, String, String)>, // id -> (name, type, list, group id)
    pub infotag_groups: Vec<(String, String)>,                       // (id, name)
    pub listitems: HashMap<String, String>,                          // id -> value
}

impl Mapping {
    fn from_json(v: &Value) -> Mapping {
        let arr = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        let mut m = Mapping::default();
        for k in arr("kinks") {
            m.kinks.insert(vs(&k, "id"), (vs(&k, "name"), vs(&k, "description"), vs(&k, "group_id")));
        }
        for g in arr("kink_groups") {
            m.kink_groups.insert(vs(&g, "id"), vs(&g, "name"));
        }
        for t in arr("infotags") {
            m.infotags.insert(vs(&t, "id"), (vs(&t, "name"), vs(&t, "type"), vs(&t, "list"), vs(&t, "group_id")));
        }
        let mut groups: Vec<(String, String)> = arr("infotag_groups").iter().map(|g| (vs(g, "id"), vs(g, "name"))).collect();
        groups.sort_by_key(|g| g.0.parse::<i64>().unwrap_or(0));
        m.infotag_groups = groups;
        for l in arr("listitems") {
            m.listitems.insert(vs(&l, "id"), vs(&l, "value"));
        }
        m
    }
}

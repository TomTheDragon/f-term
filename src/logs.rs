//! Chat logs: one plain-text file per conversation per day.
//!
//! Layout: <data>/logs/<my character>/<conversation>/<YYYY-MM-DD>.log
//! Line format:
//!   [2026-10-28 11:00:00] <Name> message
//!   [2026-10-28 11:00:00] * Name action
//!   [2026-10-28 11:00:00] [AD] <Name> ad text
//!   [2026-10-28 11:00:00] -!- system text

use std::io::Write;
use std::path::PathBuf;

use crate::paths;
use crate::state::{Message, MsgKind};
use crate::timeutil;

pub fn root(me: &str) -> PathBuf {
    paths::data_dir().join("logs").join(paths::sanitize(me))
}

pub fn conv_dir(me: &str, conv_key: &str) -> PathBuf {
    root(me).join(paths::sanitize(conv_key))
}

pub fn format_line(m: &Message) -> String {
    let text = m.text.replace('\r', "").replace('\n', "\n    ");
    let ts = timeutil::fmt_datetime(m.ts);
    match m.kind {
        MsgKind::Chat | MsgKind::Warn | MsgKind::History => format!("[{ts}] <{}> {text}", m.sender),
        MsgKind::Action => format!("[{ts}] * {} {text}", m.sender),
        MsgKind::Ad => format!("[{ts}] [AD] <{}> {text}", m.sender),
        _ => format!("[{ts}] -!- {text}"),
    }
}

/// `kind` is "channel" or "private".
pub fn append(me: &str, conv_key: &str, title: &str, kind: &str, m: &Message) {
    let dir = conv_dir(me, conv_key);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // Remember the readable title and kind of the conversation.
    let title_file = dir.join("title.txt");
    if !title_file.exists() {
        let _ = std::fs::write(&title_file, format!("{title}\n{kind}\n"));
    }
    let file = dir.join(format!("{}.log", timeutil::fmt_date(m.ts)));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{}", format_line(m));
    }
}

/// Conversations that have logs: (key, title).
pub fn conversations(me: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root(me)) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                let key = e.file_name().to_string_lossy().to_string();
                let title = std::fs::read_to_string(e.path().join("title.txt")).unwrap_or_else(|_| key.clone());
                out.push((key, title.lines().next().unwrap_or("").trim().to_string()));
            }
        }
    }
    out.sort_by_key(|a| a.1.to_lowercase());
    out
}

/// Recently active conversations: (key, title, kind), newest first.
pub fn recent(me: &str, n: usize) -> Vec<(String, String, String)> {
    let mut out: Vec<(std::time::SystemTime, String, String, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root(me)) {
        for e in rd.flatten() {
            let meta = std::fs::read_to_string(e.path().join("title.txt")).unwrap_or_default();
            let mut lines = meta.lines();
            let title = lines.next().unwrap_or("").trim().to_string();
            let kind = lines.next().unwrap_or("").trim().to_string();
            if title.is_empty() || kind.is_empty() {
                continue;
            }
            let newest = days(me, &e.file_name().to_string_lossy()).into_iter().next();
            let mtime = newest
                .and_then(|d| std::fs::metadata(e.path().join(format!("{d}.log"))).and_then(|m| m.modified()).ok())
                .unwrap_or(std::time::UNIX_EPOCH);
            out.push((mtime, e.file_name().to_string_lossy().to_string(), title, kind));
        }
    }
    out.sort_by_key(|o| std::cmp::Reverse(o.0));
    out.into_iter().take(n).map(|(_, k, t, kind)| (k, t, kind)).collect()
}

/// Log days for a conversation, newest first.
pub fn days(me: &str, conv_key: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(conv_dir(me, conv_key))
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".log").map(String::from)).collect())
        .unwrap_or_default();
    out.sort();
    out.reverse();
    out
}

pub fn read_day(me: &str, conv_key: &str, day: &str) -> Vec<String> {
    std::fs::read_to_string(conv_dir(me, conv_key).join(format!("{day}.log")))
        .map(|s| s.lines().map(String::from).collect())
        .unwrap_or_default()
}

/// Parse a log line back into a message (for history display).
pub fn parse_line(line: &str) -> Option<Message> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find("] ")?;
    let stamp = &rest[..close];
    let body = &rest[close + 2..];
    let ts = parse_stamp(stamp).unwrap_or(0);
    let (kind, sender, text) = if let Some(b) = body.strip_prefix("[AD] <") {
        let e = b.find("> ")?;
        (MsgKind::Ad, b[..e].to_string(), b[e + 2..].to_string())
    } else if let Some(b) = body.strip_prefix('<') {
        let e = b.find("> ").or_else(|| b.strip_suffix('>').map(|x| x.len()))?;
        (MsgKind::Chat, b[..e].to_string(), b.get(e + 2..).unwrap_or("").to_string())
    } else if let Some(b) = body.strip_prefix("* ") {
        // Names may contain spaces; we can't know where the name ends, so keep it in the text.
        (MsgKind::Action, String::new(), b.to_string())
    } else {
        (MsgKind::System, String::new(), body.trim_start_matches("-!- ").to_string())
    };
    Some(Message { ts, kind, sender, text, highlight: false })
}

fn parse_stamp(s: &str) -> Option<i64> {
    // Local time "YYYY-MM-DD HH:MM:SS" -> approximate unix time.
    let (d, t) = s.split_once(' ')?;
    let mut dp = d.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (dp.next()??, dp.next()??, dp.next()??);
    let mut tp = t.split(':').map(|x| x.parse::<i64>().ok());
    let (hh, mm, ss) = (tp.next()??, tp.next()??, tp.next()??);
    // days_from_civil
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let local = days * 86400 + hh * 3600 + mm * 60 + ss;
    Some(local - timeutil::local_offset(local))
}

/// Last `n` messages from the most recent log days.
pub fn tail(me: &str, conv_key: &str, n: usize) -> Vec<Message> {
    if n == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    for day in days(me, conv_key).into_iter().take(7) {
        let mut day_lines = merge_continuations(read_day(me, conv_key, &day));
        day_lines.append(&mut lines);
        lines = day_lines;
        if lines.len() >= n {
            break;
        }
    }
    let start = lines.len().saturating_sub(n);
    lines[start..]
        .iter()
        .filter_map(|l| parse_line(l))
        .map(|mut m| {
            if m.kind == MsgKind::Chat {
                m.kind = MsgKind::History;
            }
            m
        })
        .collect()
}

/// Join indented continuation lines back to their message.
pub fn merge_continuations(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in lines {
        if let (Some(rest), Some(last)) = (l.strip_prefix("    "), out.last_mut()) {
            last.push('\n');
            last.push_str(rest);
        } else {
            out.push(l);
        }
    }
    out
}

/// One search hit in the logs.
#[derive(Debug, Clone, Default)]
pub struct Hit {
    pub conv_key: String,
    pub title: String,
    pub day: String,
    pub line: String,
}

/// Search every log of `me` for `query` (case-insensitive), newest days first.
pub fn search(me: &str, query: &str, limit: usize) -> Vec<Hit> {
    let q = query.to_lowercase();
    let mut files: Vec<(String, String, String)> = Vec::new();
    for (key, title) in conversations(me) {
        for day in days(me, &key) {
            files.push((day, key.clone(), title.clone()));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = Vec::new();
    for (day, key, title) in files {
        let mut lines = merge_continuations(read_day(me, &key, &day));
        lines.reverse();
        for line in lines {
            if line.to_lowercase().contains(&q) {
                out.push(Hit { conv_key: key.clone(), title: title.clone(), day: day.clone(), line });
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}

/// Plain text log of the last messages of a conversation, for reports.
pub fn report_log(messages: &[Message]) -> String {
    messages.iter().map(format_line).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let m = Message { ts: 1_700_000_000, kind: MsgKind::Ad, sender: "Some One".into(), text: "hello".into(), highlight: false };
        let p = parse_line(&format_line(&m)).unwrap();
        assert_eq!(p.kind, MsgKind::Ad);
        assert_eq!(p.sender, "Some One");
        assert_eq!(p.text, "hello");
        assert_eq!(p.ts, 1_700_000_000);
    }
}

//! Tab completion for commands, their arguments and character names.

use super::{App, Completion};
use crate::commands::{self, Arg};
use crate::config::{ImageMode, SETTINGS};
use crate::state::Status;

/// What is being completed at the cursor.
struct Ctx {
    start: usize,
    word: String,
    items: Vec<(String, String)>,
    /// Show the popup automatically while typing.
    auto: bool,
    /// Append a space after accepting.
    add_space: bool,
}

impl App {
    fn completion_context(&self) -> Option<Ctx> {
        let text = self.input.text();
        let cur = self.input.cursor();
        let before = &text[..cur];
        if before.contains('\n') && before.starts_with('/') {
            return None;
        }
        if let Some(name_part) = before.strip_prefix('/') {
            if !name_part.contains(char::is_whitespace) {
                if name_part.starts_with('/') {
                    return None;
                }
                let items = commands::matching(name_part)
                    .into_iter()
                    .filter(|c| self.command_visible(c))
                    .map(|c| (format!("/{}", c.name), format!("{} — {}", c.usage, c.help)))
                    .collect();
                return Some(Ctx { start: 0, word: before.to_string(), items, auto: true, add_space: true });
            }
            // Argument completion.
            let (name, _) = commands::split(before)?;
            let def = commands::find(name)?;
            let args_start = before.find(char::is_whitespace)? + 1;
            let args_text = &before[args_start..];
            let multi = def.args.len() > 1;
            let (idx, seg_start) = if multi {
                let commas = args_text.matches(',').count();
                let last_comma = args_text.rfind(',').map(|i| args_start + i + 1).unwrap_or(args_start);
                (commas, last_comma)
            } else {
                (0, args_start)
            };
            let seg_start = seg_start + before[seg_start..].len() - before[seg_start..].trim_start().len();
            let word = before[seg_start..].to_string();
            let kind = def.args.get(idx).copied().unwrap_or(Arg::Text);
            let kind = match (def.name, kind) {
                ("images", _) => Arg::Mode,
                ("help", _) => Arg::Nothing,
                (_, k) => k,
            };
            let mut items = self.arg_items(kind, &word, def.name);
            if def.name == "help" {
                items = commands::matching(word.trim_start_matches('/'))
                    .into_iter()
                    .map(|c| (c.name.to_string(), c.help.to_string()))
                    .collect();
            }
            let auto = !matches!(kind, Arg::Text | Arg::Number | Arg::Nothing) || def.name == "help";
            return Some(Ctx { start: seg_start, word, items, auto, add_space: false });
        }
        // An eicon name inside [eicon]…
        if let Some((start, word)) = super::eicons::eicon_word(before) {
            let items = match &self.eicon_index {
                Some(idx) if !word.is_empty() => {
                    idx.search(word, 50).into_iter().map(|n| (format!("{n}[/eicon]"), String::new())).collect()
                }
                _ => Vec::new(),
            };
            return Some(Ctx { start, word: word.to_string(), items, auto: true, add_space: false });
        }
        // Plain text: complete the last word as a character name.
        let start = before.rfind(char::is_whitespace).map(|i| i + before[i..].chars().next().unwrap().len_utf8()).unwrap_or(0);
        let word = before[start..].to_string();
        if word.is_empty() {
            return None;
        }
        let items =
            self.chat.complete_names(&word, Some(self.chat.active_conv())).into_iter().take(50).map(|n| (n, String::new())).collect();
        Some(Ctx { start, word, items, auto: false, add_space: false })
    }

    fn command_visible(&self, c: &commands::CommandDef) -> bool {
        use commands::Perm;
        match c.perm {
            Perm::User | Perm::ChanOp | Perm::ChanOwner => true,
            Perm::ChatOp | Perm::Admin => self.chat.is_chatop(&self.chat.me),
        }
    }

    fn arg_items(&self, kind: Arg, word: &str, cmd: &str) -> Vec<(String, String)> {
        let w = word.to_lowercase();
        let starts = |s: &str| s.to_lowercase().starts_with(&w);
        let contains = |s: &str| s.to_lowercase().contains(&w);
        match kind {
            Arg::Character => self
                .chat
                .complete_names(word, Some(self.chat.active_conv()))
                .into_iter()
                .take(50)
                .map(|n| {
                    let st = self.chat.status_of(&n);
                    (n, st.label().to_string())
                })
                .collect(),
            Arg::Member => self
                .chat
                .sorted_members(self.chat.active_conv())
                .into_iter()
                .filter(|n| starts(n))
                .take(50)
                .map(|n| (n, String::new()))
                .collect(),
            Arg::Channel => {
                let mut v: Vec<(String, String)> = Vec::new();
                for c in &self.chat.conversations {
                    if let Some(id) = c.channel_id() {
                        if contains(&c.title) || contains(id) {
                            v.push((if id.starts_with("ADH-") { id.to_string() } else { c.title.clone() }, format!("{} (open)", c.title)));
                        }
                    }
                }
                let mut listed: Vec<(String, String, i64)> = self
                    .chat
                    .public_channels
                    .iter()
                    .filter(|c| contains(&c.name))
                    .map(|c| (c.name.clone(), format!("{} users", c.characters), c.characters))
                    .chain(
                        self.chat
                            .private_rooms
                            .iter()
                            .filter(|c| contains(&c.title))
                            .map(|c| (c.name.clone(), format!("{} — {} users", c.title, c.characters), c.characters)),
                    )
                    .collect();
                listed.sort_by_key(|c| (!starts(&c.0) && !starts(&c.1), -c.2));
                for (n, d, _) in listed {
                    if !v.iter().any(|x| x.0 == n) {
                        v.push((n, d));
                    }
                }
                v.truncate(60);
                v
            }
            Arg::Status => {
                Status::SETTABLE.iter().filter(|s| starts(s.as_str())).map(|s| (s.as_str().to_string(), s.label().to_string())).collect()
            }
            Arg::Mode => {
                if cmd == "images" {
                    ImageMode::ALL.iter().filter(|m| starts(m.name())).map(|m| (m.name().to_string(), String::new())).collect()
                } else {
                    ["chat", "ads", "both"].iter().filter(|m| starts(m)).map(|m| (m.to_string(), String::new())).collect()
                }
            }
            Arg::Setting => SETTINGS
                .iter()
                .filter(|s| starts(s.key))
                .map(|s| (s.key.to_string(), format!("{} = {}", s.help, self.settings.get(s.key).unwrap_or_default())))
                .collect(),
            Arg::Dice => ["1d20", "1d6", "2d6", "1d100", "3d6", "1d20+5"]
                .iter()
                .filter(|d| starts(d))
                .map(|d| (d.to_string(), String::new()))
                .collect(),
            Arg::OpenConversation => {
                self.chat.conversations.iter().skip(1).filter(|c| contains(&c.title)).map(|c| (c.title.clone(), String::new())).collect()
            }
            Arg::LogConversation => crate::logs::conversations(&self.chat.me)
                .into_iter()
                .filter(|(_, t)| contains(t))
                .take(60)
                .map(|(_, t)| (t, String::new()))
                .collect(),
            Arg::Nothing | Arg::Text | Arg::Number => Vec::new(),
        }
    }

    /// Update the popup while typing.
    pub(super) fn completion_refresh(&mut self) {
        let keep_sel = self.completion.as_ref().map(|c| c.sel).unwrap_or(0);
        match self.completion_context() {
            Some(ctx) if ctx.auto && !ctx.items.is_empty() => {
                // Hide when the only suggestion is exactly what was typed.
                if ctx.items.len() == 1 && ctx.items[0].0.eq_ignore_ascii_case(&ctx.word) {
                    self.completion = None;
                    return;
                }
                let n = ctx.items.len();
                self.completion = Some(Completion { start: ctx.start, items: ctx.items, sel: keep_sel.min(n - 1), cycling: false });
            }
            _ => self.completion = None,
        }
    }

    pub(super) fn completion_start(&mut self) {
        let before = &self.input.text()[..self.input.cursor()];
        if super::eicons::eicon_word(before).is_some() && self.eicon_index.is_none() {
            return self.ensure_eicons();
        }
        if let Some(ctx) = self.completion_context() {
            if ctx.items.is_empty() {
                return;
            }
            let single = ctx.items.len() == 1;
            self.completion = Some(Completion { start: ctx.start, items: ctx.items, sel: 0, cycling: false });
            self.completion_apply(ctx.add_space && single);
            if single {
                self.completion = None;
                self.completion_refresh();
            }
        }
    }

    /// Tab / Shift+Tab: insert the selection, then cycle.
    pub(super) fn completion_step(&mut self, dir: i32) {
        let Some(c) = &mut self.completion else { return };
        if c.cycling {
            let n = c.items.len() as i32;
            c.sel = ((c.sel as i32 + dir).rem_euclid(n)) as usize;
        } else if dir < 0 {
            let n = c.items.len();
            c.sel = (c.sel + n - 1) % n;
            return;
        } else if c.items.len() == 1 {
            // A single match: complete it and move on to the next argument.
            let is_cmd = c.start == 0 && c.items[0].0.starts_with('/');
            self.completion_apply(is_cmd);
            self.completion = None;
            self.completion_refresh();
            return;
        }
        self.completion_apply(false);
    }

    /// Enter on the popup: insert the selection. Returns false when there was
    /// nothing to change (so Enter should send the message).
    pub(super) fn completion_accept(&mut self) -> bool {
        let Some(c) = &self.completion else { return false };
        let item = c.items[c.sel].0.clone();
        let current = &self.input.text()[c.start.min(self.input.cursor())..self.input.cursor()];
        if current == item {
            self.completion = None;
            return false;
        }
        let is_cmd = item.starts_with('/') && c.start == 0;
        self.completion_apply(is_cmd);
        self.completion = None;
        self.completion_refresh();
        true
    }

    fn completion_apply(&mut self, add_space: bool) {
        let Some(c) = &mut self.completion else { return };
        let mut item = c.items[c.sel].0.clone();
        if add_space {
            item.push(' ');
        }
        let start = c.start;
        c.cycling = true;
        self.input.replace_before_cursor(start, &item);
    }
}

//! F-Chat wire protocol: `XXX {json}` commands over a websocket.

use serde_json::{json, Map, Value};

pub const CLIENT_NAME: &str = "F-Term";
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Lenient JSON helpers (the server is not always consistent about types).

pub fn vs(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

pub fn vi(v: &Value, key: &str) -> i64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        Some(Value::Bool(b)) => *b as i64,
        _ => 0,
    }
}

pub fn vlist(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Characters inside ICH/JCH are objects like `{"identity": "Name"}`.
fn identity(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(_) => vs(v, "identity"),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ListedChannel {
    pub name: String,
    pub title: String,
    pub mode: String,
    pub characters: i64,
}

#[derive(Debug, Clone)]
pub enum ServerCmd {
    Adl { ops: Vec<String> },
    Aop { character: String },
    Bro { message: String, character: String },
    Cds { channel: String, description: String },
    Cha { channels: Vec<ListedChannel> },
    Ciu { sender: String, title: String, name: String },
    Cbu { operator: String, channel: String, character: String },
    Cku { operator: String, channel: String, character: String },
    Coa { character: String, channel: String },
    Col { channel: String, oplist: Vec<String> },
    Con { count: i64 },
    Cor { character: String, channel: String },
    Cso { character: String, channel: String },
    Ctu { operator: String, channel: String, length: i64, character: String },
    Dop { character: String },
    Err { number: i64, message: String },
    Fks { characters: Vec<String> },
    Fln { character: String },
    Hlo { message: String },
    Ich { channel: String, users: Vec<String>, mode: String },
    Idn { character: String },
    Jch { channel: String, character: String, title: String },
    Kid { kind: String, message: String, key: String, value: String },
    Lch { channel: String, character: String },
    Lis { characters: Vec<[String; 4]> },
    Nln { identity: String, gender: String, status: String },
    Ign { action: String, characters: Vec<String>, character: String },
    Frl { characters: Vec<String> },
    Ors { channels: Vec<ListedChannel> },
    Pin,
    Prd { kind: String, message: String, key: String, value: String },
    Pri { character: String, message: String },
    Msg { character: String, message: String, channel: String },
    Lrp { character: String, message: String, channel: String },
    Rll { channel: String, character: String, message: String, kind: String, target: String, recipient: String },
    Rmo { mode: String, channel: String },
    Rtb { kind: String, data: Value },
    Sfc { data: Value },
    Sta { character: String, status: String, statusmsg: String },
    Sys { message: String, channel: String },
    Tpn { character: String, status: String },
    Upt { time: i64, starttime: i64, startstring: String, accepted: i64, channels: i64, users: i64, maxusers: i64 },
    Var { variable: String, value: Value },
    Unknown { code: String, data: Value },
}

pub fn parse(line: &str) -> Result<ServerCmd, String> {
    let line = line.trim_end();
    if line.len() < 3 || !line.is_char_boundary(3) {
        return Err(format!("command too short: {line:?}"));
    }
    let (code, rest) = line.split_at(3);
    let rest = rest.trim();
    let v: Value = if rest.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(rest).map_err(|e| format!("bad JSON in {code}: {e}"))?
    };
    let v = &v;
    let s = |k| vs(v, k);
    let channel_list = |v: &Value, private: bool| -> Vec<ListedChannel> {
        v.get("channels")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|c| {
                        let name = vs(c, "name");
                        ListedChannel {
                            title: if private { vs(c, "title") } else { name.clone() },
                            name,
                            mode: vs(c, "mode"),
                            characters: vi(c, "characters"),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(match code {
        "ADL" => ServerCmd::Adl { ops: vlist(v, "ops") },
        "AOP" => ServerCmd::Aop { character: s("character") },
        "BRO" => ServerCmd::Bro { message: s("message"), character: s("character") },
        "CDS" => ServerCmd::Cds { channel: s("channel"), description: s("description") },
        "CHA" => ServerCmd::Cha { channels: channel_list(v, false) },
        "CIU" => ServerCmd::Ciu { sender: s("sender"), title: s("title"), name: s("name") },
        "CBU" => ServerCmd::Cbu { operator: s("operator"), channel: s("channel"), character: s("character") },
        "CKU" => ServerCmd::Cku { operator: s("operator"), channel: s("channel"), character: s("character") },
        "COA" => ServerCmd::Coa { character: s("character"), channel: s("channel") },
        "COL" => ServerCmd::Col { channel: s("channel"), oplist: vlist(v, "oplist") },
        "CON" => ServerCmd::Con { count: vi(v, "count") },
        "COR" => ServerCmd::Cor { character: s("character"), channel: s("channel") },
        "CSO" => ServerCmd::Cso { character: s("character"), channel: s("channel") },
        "CTU" => ServerCmd::Ctu { operator: s("operator"), channel: s("channel"), length: vi(v, "length"), character: s("character") },
        "DOP" => ServerCmd::Dop { character: s("character") },
        "ERR" => ServerCmd::Err { number: vi(v, "number"), message: s("message") },
        "FKS" => ServerCmd::Fks { characters: vlist(v, "characters") },
        "FLN" => ServerCmd::Fln { character: s("character") },
        "HLO" => ServerCmd::Hlo { message: s("message") },
        "ICH" => ServerCmd::Ich {
            channel: s("channel"),
            users: v.get("users").and_then(Value::as_array).map(|a| a.iter().map(identity).collect()).unwrap_or_default(),
            mode: s("mode"),
        },
        "IDN" => ServerCmd::Idn { character: s("character") },
        "JCH" => {
            ServerCmd::Jch { channel: s("channel"), character: v.get("character").map(identity).unwrap_or_default(), title: s("title") }
        }
        "KID" => ServerCmd::Kid { kind: s("type"), message: s("message"), key: s("key"), value: s("value") },
        "LCH" => ServerCmd::Lch { channel: s("channel"), character: s("character") },
        "LIS" => ServerCmd::Lis {
            characters: v
                .get("characters")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_array())
                        .map(|c| {
                            let g = |i: usize| c.get(i).and_then(Value::as_str).unwrap_or("").to_string();
                            [g(0), g(1), g(2), g(3)]
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        "NLN" => ServerCmd::Nln { identity: s("identity"), gender: s("gender"), status: s("status") },
        "IGN" => ServerCmd::Ign { action: s("action"), characters: vlist(v, "characters"), character: s("character") },
        "FRL" => ServerCmd::Frl { characters: vlist(v, "characters") },
        "ORS" => ServerCmd::Ors { channels: channel_list(v, true) },
        "PIN" => ServerCmd::Pin,
        "PRD" => ServerCmd::Prd { kind: s("type"), message: s("message"), key: s("key"), value: s("value") },
        "PRI" => ServerCmd::Pri { character: s("character"), message: s("message") },
        "MSG" => ServerCmd::Msg { character: s("character"), message: s("message"), channel: s("channel") },
        "LRP" => ServerCmd::Lrp { character: s("character"), message: s("message"), channel: s("channel") },
        "RLL" => ServerCmd::Rll {
            channel: s("channel"),
            character: s("character"),
            message: s("message"),
            kind: s("type"),
            target: s("target"),
            recipient: s("recipient"),
        },
        "RMO" => ServerCmd::Rmo { mode: s("mode"), channel: s("channel") },
        "RTB" => ServerCmd::Rtb { kind: s("type"), data: v.clone() },
        "SFC" => ServerCmd::Sfc { data: v.clone() },
        "STA" => ServerCmd::Sta { character: s("character"), status: s("status"), statusmsg: s("statusmsg") },
        "SYS" => ServerCmd::Sys { message: s("message"), channel: s("channel") },
        "TPN" => ServerCmd::Tpn { character: s("character"), status: s("status") },
        "UPT" => ServerCmd::Upt {
            time: vi(v, "time"),
            starttime: vi(v, "starttime"),
            startstring: s("startstring"),
            accepted: vi(v, "accepted"),
            channels: vi(v, "channels"),
            users: vi(v, "users"),
            maxusers: vi(v, "maxusers"),
        },
        "VAR" => ServerCmd::Var { variable: s("variable"), value: v.get("value").cloned().unwrap_or(Value::Null) },
        other => ServerCmd::Unknown { code: other.to_string(), data: v.clone() },
    })
}

// ---------------------------------------------------------------------------
// Client commands

fn cmd(code: &str, body: Value) -> String {
    format!("{code} {body}")
}

pub fn idn(account: &str, ticket: &str, character: &str) -> String {
    cmd(
        "IDN",
        json!({"method": "ticket", "account": account, "ticket": ticket, "character": character,
               "cname": CLIENT_NAME, "cversion": CLIENT_VERSION}),
    )
}
pub fn pin() -> String {
    "PIN".into()
}
pub fn msg(channel: &str, message: &str) -> String {
    cmd("MSG", json!({"channel": channel, "message": message}))
}
pub fn lrp(channel: &str, message: &str) -> String {
    cmd("LRP", json!({"channel": channel, "message": message}))
}
pub fn pri(recipient: &str, message: &str) -> String {
    cmd("PRI", json!({"recipient": recipient, "message": message}))
}
pub fn jch(channel: &str) -> String {
    cmd("JCH", json!({"channel": channel}))
}
pub fn lch(channel: &str) -> String {
    cmd("LCH", json!({"channel": channel}))
}
pub fn cha() -> String {
    "CHA".into()
}
pub fn ors() -> String {
    "ORS".into()
}
pub fn sta(status: &str, statusmsg: &str) -> String {
    cmd("STA", json!({"status": status, "statusmsg": statusmsg}))
}
pub fn tpn(character: &str, status: &str) -> String {
    cmd("TPN", json!({"character": character, "status": status}))
}
pub fn rll(channel: &str, dice: &str) -> String {
    cmd("RLL", json!({"channel": channel, "dice": dice}))
}
pub fn rll_private(recipient: &str, dice: &str) -> String {
    cmd("RLL", json!({"recipient": recipient, "dice": dice}))
}
pub fn ign(action: &str, character: Option<&str>) -> String {
    match character {
        Some(c) => cmd("IGN", json!({"action": action, "character": c})),
        None => cmd("IGN", json!({"action": action})),
    }
}
pub fn chan_char(code: &str, channel: &str, character: &str) -> String {
    cmd(code, json!({"channel": channel, "character": character}))
}
pub fn chan_only(code: &str, channel: &str) -> String {
    cmd(code, json!({"channel": channel}))
}
pub fn char_only(code: &str, character: &str) -> String {
    cmd(code, json!({"character": character}))
}
pub fn ctu(channel: &str, character: &str, minutes: i64) -> String {
    cmd("CTU", json!({"channel": channel, "character": character, "length": minutes}))
}
pub fn tmo(character: &str, minutes: i64, reason: &str) -> String {
    cmd("TMO", json!({"character": character, "time": minutes, "reason": reason}))
}
pub fn cds(channel: &str, description: &str) -> String {
    cmd("CDS", json!({"channel": channel, "description": description}))
}
pub fn rmo(channel: &str, mode: &str) -> String {
    cmd("RMO", json!({"channel": channel, "mode": mode}))
}
pub fn rst(channel: &str, status: &str) -> String {
    cmd("RST", json!({"channel": channel, "status": status}))
}
pub fn bro(message: &str) -> String {
    cmd("BRO", json!({"message": message}))
}
pub fn rld(save: &str) -> String {
    cmd("RLD", json!({"save": save}))
}
pub fn upt() -> String {
    "UPT".into()
}
pub fn sfc_report(character: &str, report: &str, logid: Option<i64>) -> String {
    let mut v = json!({"action": "report", "report": report, "character": character});
    if let Some(id) = logid {
        v["logid"] = json!(id);
    }
    cmd("SFC", v)
}
pub fn sfc_confirm(callid: &str) -> String {
    cmd("SFC", json!({"action": "confirm", "callid": callid}))
}
pub fn fks(
    kinks: &[String],
    genders: &[String],
    orientations: &[String],
    languages: &[String],
    furryprefs: &[String],
    roles: &[String],
) -> String {
    let mut v = json!({"kinks": kinks});
    for (k, list) in
        [("genders", genders), ("orientations", orientations), ("languages", languages), ("furryprefs", furryprefs), ("roles", roles)]
    {
        if !list.is_empty() {
            v[k] = json!(list);
        }
    }
    cmd("FKS", v)
}

/// Error numbers after which we must not reconnect automatically.
pub fn is_fatal_error(number: i64) -> bool {
    matches!(number, 2 | 9 | 30 | 31 | 33 | 39 | 40)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_samples() {
        match parse(r#"ICH {"users": [{"identity": "Shadlor"}, {"identity": "Hexxy"}], "channel": "Frontpage", "mode": "both"}"#).unwrap() {
            ServerCmd::Ich { users, channel, mode } => {
                assert_eq!(users, vec!["Shadlor", "Hexxy"]);
                assert_eq!(channel, "Frontpage");
                assert_eq!(mode, "both");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse("PIN").unwrap(), ServerCmd::Pin));
        match parse(r#"LIS {"characters": [["Alexandrea", "Female", "online", ""]]}"#).unwrap() {
            ServerCmd::Lis { characters } => assert_eq!(characters[0][1], "Female"),
            other => panic!("{other:?}"),
        }
        match parse(r#"VAR {"value":"35868","variable":"permissions"}"#).unwrap() {
            ServerCmd::Var { variable, value } => {
                assert_eq!(variable, "permissions");
                assert_eq!(value, json!("35868"));
            }
            other => panic!("{other:?}"),
        }
        match parse(r#"CTU {"operator":"M","channel":"ADH-2","length":"10","character":"T"}"#).unwrap() {
            ServerCmd::Ctu { length, .. } => assert_eq!(length, 10),
            other => panic!("{other:?}"),
        }
        assert!(parse("XY").is_err());
    }
    #[test]
    fn build() {
        assert_eq!(msg("Frontpage", "hi"), r#"MSG {"channel":"Frontpage","message":"hi"}"#);
        assert_eq!(pin(), "PIN");
    }
}

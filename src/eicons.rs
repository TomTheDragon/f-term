//! Searchable list of all eicon names.
//!
//! F-List has no eicon search, so this uses the public eicon database at
//! xariah.net (the same one the Horizon client uses): a full list once, then
//! changes since the last update. The list is kept in the cache folder as
//! `name<TAB>timestamp` lines ending with `# As Of: <timestamp>`.

use std::collections::HashSet;
use std::time::Duration;

const BASE_URL: &str = "https://xariah.net/eicons/Home/EiconsDataBase/base.doc";
const DELTA_URL: &str = "https://xariah.net/eicons/Home/EiconsDataDeltaSince/";
/// Update the list when it is older than this.
const MAX_AGE: i64 = 24 * 3600;

pub struct EiconIndex {
    /// All names, lowercase, one per line (much smaller than a Vec<String>).
    names: String,
    pub count: usize,
    pub as_of: i64,
}

fn path() -> std::path::PathBuf {
    crate::paths::cache_dir().join("eicons.txt")
}

/// Parse the stored list (or the downloaded base file).
pub fn parse(text: &str) -> EiconIndex {
    let mut names = String::with_capacity(text.len() / 2);
    let mut count = 0;
    let mut as_of = 0;
    for line in text.lines() {
        if let Some(t) = line.strip_prefix("# As Of:") {
            as_of = t.trim().parse().unwrap_or(0);
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let name = line.split('\t').next().unwrap_or("").trim();
        if !name.is_empty() {
            names.push_str(&name.to_lowercase());
            names.push('\n');
            count += 1;
        }
    }
    EiconIndex { names, count, as_of }
}

impl EiconIndex {
    /// Apply a delta file. Returns (more data available, new "as of").
    fn apply_delta(&mut self, text: &str) -> (bool, i64) {
        let mut more = false;
        let mut as_of = self.as_of;
        let mut add: Vec<String> = Vec::new();
        let mut remove: HashSet<String> = HashSet::new();
        for line in text.lines() {
            if let Some(t) = line.strip_prefix("# As Of:") {
                as_of = t.trim().parse().unwrap_or(as_of);
            } else if let Some(t) = line.strip_prefix("# Has More Data:") {
                more = t.trim().eq_ignore_ascii_case("true");
            } else if let Some(rest) = line.strip_prefix("+\t") {
                let n = rest.split('\t').next().unwrap_or("").trim().to_lowercase();
                remove.remove(&n);
                if !n.is_empty() {
                    add.push(n);
                }
            } else if let Some(rest) = line.strip_prefix("-\t") {
                let n = rest.split('\t').next().unwrap_or("").trim().to_lowercase();
                add.retain(|a| *a != n);
                remove.insert(n);
            }
        }
        if !remove.is_empty() || !add.is_empty() {
            let existing: HashSet<&str> = self.names.lines().collect();
            let add: Vec<String> = add.into_iter().filter(|a| !existing.contains(a.as_str())).collect();
            let mut names = String::with_capacity(self.names.len());
            for n in self.names.lines().filter(|n| !remove.contains(*n)) {
                names.push_str(n);
                names.push('\n');
            }
            for n in &add {
                names.push_str(n);
                names.push('\n');
            }
            self.count = names.lines().count();
            self.names = names;
        }
        self.as_of = as_of;
        (more, as_of)
    }

    fn save(&self) {
        let mut out = String::with_capacity(self.names.len() + 32);
        for n in self.names.lines() {
            out.push_str(n);
            out.push_str("\t0\n");
        }
        out.push_str(&format!("# As Of: {}\n", self.as_of));
        let _ = crate::paths::write_private(&path(), out.as_bytes());
    }

    /// Names containing `query` (lowercase): those starting with it first,
    /// then the others, each group shortest first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<String> {
        let q = query.trim().to_lowercase();
        let mut starts: Vec<&str> = Vec::new();
        let mut contains: Vec<&str> = Vec::new();
        for n in self.names.lines() {
            if q.is_empty() || n.starts_with(&q) {
                starts.push(n);
            } else if n.contains(&q) {
                contains.push(n);
            }
            if q.is_empty() && starts.len() >= limit {
                break;
            }
        }
        let order = |a: &&str, b: &&str| a.len().cmp(&b.len()).then(a.cmp(b));
        starts.sort_by(order);
        contains.sort_by(order);
        starts.into_iter().chain(contains).take(limit).map(String::from).collect()
    }
}

/// Load the list from the cache, downloading or updating it when needed.
/// Blocking; run it on a worker thread.
pub fn load(agent: &ureq::Agent) -> Result<EiconIndex, String> {
    let now = crate::timeutil::now();
    let mut idx = match std::fs::read_to_string(path()) {
        Ok(t) => parse(&t),
        Err(_) => {
            let base = crate::api::get_bytes(agent, BASE_URL, 64 * 1024 * 1024).map_err(|e| format!("Eicon list: {e}"))?;
            let mut idx = parse(&String::from_utf8_lossy(&base));
            if idx.count == 0 {
                return Err("The eicon list could not be read.".into());
            }
            if idx.as_of == 0 {
                idx.as_of = now;
            }
            idx.save();
            return Ok(idx);
        }
    };
    if idx.count > 0 && now - idx.as_of > MAX_AGE {
        // Changes since the last update, in pages.
        for _ in 0..50 {
            let url = format!("{DELTA_URL}{}", idx.as_of);
            let Ok(b) = crate::api::get_bytes(agent, &url, 16 * 1024 * 1024) else { break };
            let (more, _) = idx.apply_delta(&String::from_utf8_lossy(&b));
            if !more {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        idx.save();
    }
    Ok(idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_search_and_delta() {
        let mut idx = parse("wave\t1\nWaveHello\t2\nbigwave\t3\nhug\t4\n# As Of: 100\n");
        assert_eq!((idx.count, idx.as_of), (4, 100));
        assert_eq!(idx.search("wave", 10), vec!["wave", "wavehello", "bigwave"]);
        assert_eq!(idx.search("wave", 2), vec!["wave", "wavehello"]);
        let (more, as_of) =
            idx.apply_delta("# Delta Since: 100\n# As Of: 200\n# Has More Data: false\n-\thug\t150\n+\tnewone\t160\n+\twave\t170\n");
        assert!(!more);
        assert_eq!(as_of, 200);
        assert_eq!(idx.count, 4);
        assert!(idx.search("hug", 10).is_empty());
        assert_eq!(idx.search("new", 10), vec!["newone"]);
    }

    /// Downloads the real list: cargo test -- --ignored eicons_live
    #[test]
    #[ignore]
    fn eicons_live() {
        let dir = std::env::temp_dir().join(format!("f-term-eicons-{}", std::process::id()));
        crate::paths::init(Some(dir.clone()));
        let t = std::time::Instant::now();
        let idx = load(&crate::api::agent()).unwrap();
        println!("{} eicons, as of {}, {:?}", idx.count, idx.as_of, t.elapsed());
        assert!(idx.count > 100_000);
        let t = std::time::Instant::now();
        let r = idx.search("wave", 10);
        println!("search: {r:?} {:?}", t.elapsed());
        // Second load reads the cache file.
        let again = load(&crate::api::agent()).unwrap();
        assert_eq!(again.count, idx.count);
        let _ = std::fs::remove_dir_all(dir);
    }
}

//! Launch history used for ranking ("frecency": frequency + recency).
//!
//! Stored as a tiny tab-separated file: `count<TAB>last_used_unix<TAB>key`.
//! Keys are provider-scoped, e.g. `app:Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::util::{data_dir, now_unix};

#[derive(Clone, Copy)]
struct Entry {
    count: u32,
    last: u64,
}

pub struct History {
    map: HashMap<String, Entry>,
    path: PathBuf,
}

impl History {
    pub fn load() -> Self {
        let path = data_dir().join("history.tsv");
        let mut map = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let mut it = line.splitn(3, '\t');
                let (Some(c), Some(l), Some(k)) = (it.next(), it.next(), it.next()) else {
                    continue;
                };
                if let (Ok(count), Ok(last)) = (c.parse(), l.parse()) {
                    map.insert(k.to_string(), Entry { count, last });
                }
            }
        }
        Self { map, path }
    }

    pub fn record(&mut self, key: &str) {
        let e = self.map.entry(key.to_string()).or_insert(Entry { count: 0, last: 0 });
        e.count = e.count.saturating_add(1);
        e.last = now_unix();
        self.save();
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.save();
    }

    fn save(&self) {
        let mut out = String::with_capacity(self.map.len() * 64);
        for (k, e) in &self.map {
            out.push_str(&format!("{}\t{}\t{}\n", e.count, e.last, k));
        }
        let _ = std::fs::write(&self.path, out);
    }

    /// Combined frequency/recency weight. 0 for never-used items.
    pub fn frecency(&self, key: &str) -> f64 {
        let Some(e) = self.map.get(key) else { return 0.0 };
        let age_h = (now_unix().saturating_sub(e.last)) as f64 / 3600.0;
        let recency = if age_h < 1.0 {
            4.0
        } else if age_h < 24.0 {
            2.0
        } else if age_h < 24.0 * 7.0 {
            1.0
        } else if age_h < 24.0 * 30.0 {
            0.5
        } else {
            0.25
        };
        (e.count as f64).min(100.0) * recency
    }

    /// Score bonus added to fuzzy-match scores. Bounded so a good text match
    /// still beats a frequently used poor match.
    pub fn boost(&self, key: &str) -> i32 {
        let f = self.frecency(key);
        if f <= 0.0 { 0 } else { ((f.ln_1p()) * 10.0).min(45.0) as i32 }
    }

    /// Most relevant keys, best first.
    pub fn top(&self, n: usize) -> Vec<String> {
        let mut v: Vec<(&String, f64)> =
            self.map.keys().map(|k| (k, self.frecency(k))).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v.into_iter().take(n).map(|(k, _)| k.clone()).collect()
    }
}

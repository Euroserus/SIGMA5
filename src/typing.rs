//! Typing test: a timed or fixed-length run over common English words,
//! scored like the popular typing sites (WPM from correctly typed words,
//! raw WPM, accuracy from keystrokes).
//!
//! Pure logic only; the launcher draws it and feeds it keystrokes. Personal
//! bests are kept in `%APPDATA%\SIGMA5\typing.json`.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Most common English words (short, everyday).
pub const WORDS: &[&str] = &[
    "the", "be", "of", "and", "a", "to", "in", "he", "have", "it", "that", "for", "they", "with", "as", "not", "on",
    "she", "at", "by", "this", "we", "you", "do", "but", "from", "or", "which", "one", "would", "all", "will",
    "there", "say", "who", "make", "when", "can", "more", "if", "no", "man", "out", "other", "so", "what", "time",
    "up", "go", "about", "than", "into", "could", "state", "only", "new", "year", "some", "take", "come", "these",
    "know", "see", "use", "get", "like", "then", "first", "any", "work", "now", "may", "such", "give", "over",
    "think", "most", "even", "find", "day", "also", "after", "way", "many", "must", "look", "before", "great",
    "back", "through", "long", "where", "much", "should", "well", "people", "down", "own", "just", "because",
    "good", "each", "those", "feel", "seem", "how", "high", "too", "place", "little", "world", "very", "still",
    "nation", "hand", "old", "life", "tell", "write", "become", "here", "show", "house", "both", "between", "need",
    "mean", "call", "develop", "under", "last", "right", "move", "thing", "general", "school", "never", "same",
    "another", "begin", "while", "number", "part", "turn", "real", "leave", "might", "want", "point", "form",
    "off", "child", "few", "small", "since", "against", "ask", "late", "home", "interest", "large", "person",
    "end", "open", "public", "follow", "during", "present", "without", "again", "hold", "govern", "around",
    "possible", "head", "consider", "word", "program", "problem", "however", "lead", "system", "set", "order",
    "eye", "plan", "run", "keep", "face", "fact", "group", "play", "stand", "increase", "early", "course",
    "change", "help", "line", "city", "put", "close", "case", "force", "meet", "once", "water", "upon", "war",
    "build", "hear", "light", "unite", "live", "every", "country", "bring", "center", "let", "side", "try",
    "provide", "continue", "name", "certain", "power", "pay", "result", "question", "study", "woman", "member",
    "until", "far", "night", "always", "service", "away", "report", "something", "company", "week", "church",
    "toward", "start", "social", "room", "figure", "nature", "though", "young", "less", "enough", "almost",
    "read", "include", "president", "nothing", "yet", "better", "big", "boy", "cost", "business", "value",
    "second", "why", "clear", "expect", "family", "complete", "act", "sense", "mind", "experience", "art",
];

/// What the run is measured by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// Type as much as possible in this many seconds.
    Time(u32),
    /// Type this many words as fast as possible.
    Words(u32),
}

pub const TIMES: [u32; 4] = [15, 30, 60, 120];
pub const COUNTS: [u32; 4] = [10, 25, 50, 100];

impl Default for Kind {
    fn default() -> Self {
        Kind::Time(30)
    }
}

impl Kind {
    /// Key used for personal bests, e.g. "time 30".
    pub fn key(self) -> String {
        match self {
            Kind::Time(s) => format!("time {s}"),
            Kind::Words(n) => format!("words {n}"),
        }
    }
}

/// Final (or live) scores.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub wpm: f32,
    pub raw: f32,
    /// Percent.
    pub acc: f32,
    pub secs: f32,
    pub correct: u32,
    pub incorrect: u32,
    pub extra: u32,
    pub missed: u32,
}

/// Extra letters past the end of a word are capped (as typing sites do).
const MAX_EXTRA: usize = 12;

/// Small, fast PRNG (xorshift) so runs differ without pulling in a crate.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

pub struct Test {
    pub kind: Kind,
    pub words: Vec<&'static str>,
    /// What was typed for each word so far (index = word).
    pub typed: Vec<String>,
    /// Index of the word being typed.
    pub current: usize,
    pub started: Option<Instant>,
    pub finished: Option<Score>,
    /// Keystrokes that produced a character, and how many were right.
    keys: u32,
    keys_ok: u32,
    rng: Rng,
}

impl Test {
    pub fn new(kind: Kind, seed: u64) -> Self {
        let mut t = Test {
            kind,
            words: Vec::new(),
            typed: vec![String::new()],
            current: 0,
            started: None,
            finished: None,
            keys: 0,
            keys_ok: 0,
            rng: Rng::new(seed),
        };
        let n = match kind {
            Kind::Words(n) => n as usize,
            Kind::Time(_) => 60,
        };
        t.add_words(n);
        t
    }

    fn add_words(&mut self, n: usize) {
        for _ in 0..n {
            // Avoid the same word twice in a row.
            let mut w = WORDS[(self.rng.next() % WORDS.len() as u64) as usize];
            if self.words.last() == Some(&w) {
                w = WORDS[(self.rng.next() % WORDS.len() as u64) as usize];
            }
            self.words.push(w);
        }
    }

    pub fn running(&self) -> bool {
        self.started.is_some() && self.finished.is_none()
    }

    pub fn elapsed(&self) -> Duration {
        self.started.map_or(Duration::ZERO, |s| s.elapsed())
    }

    /// Seconds left in a timed run.
    pub fn remaining(&self) -> Option<u32> {
        match self.kind {
            Kind::Time(s) => Some((s as f32 - self.elapsed().as_secs_f32()).ceil().max(0.0) as u32),
            Kind::Words(_) => None,
        }
    }

    fn start_if_needed(&mut self) {
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
    }

    /// A printable character (not space).
    pub fn type_char(&mut self, c: char) {
        if self.finished.is_some() {
            return;
        }
        self.start_if_needed();
        let word = self.words[self.current];
        let typed = &mut self.typed[self.current];
        if typed.chars().count() >= word.chars().count() + MAX_EXTRA {
            return;
        }
        let at = typed.chars().count();
        typed.push(c);
        self.keys += 1;
        if word.chars().nth(at) == Some(c) {
            self.keys_ok += 1;
        }
        // The last word of a word run ends the test as soon as it's right.
        if let Kind::Words(n) = self.kind {
            if self.current + 1 == n as usize && self.typed[self.current] == word {
                self.finish();
            }
        }
    }

    /// Space: on to the next word (ignored at the start of a word).
    pub fn space(&mut self) {
        if self.finished.is_some() || self.typed[self.current].is_empty() {
            return;
        }
        self.keys += 1;
        if self.typed[self.current] == self.words[self.current] {
            self.keys_ok += 1;
        }
        if let Kind::Words(n) = self.kind {
            if self.current + 1 >= n as usize {
                self.finish();
                return;
            }
        }
        self.current += 1;
        self.typed.push(String::new());
        if self.words.len() < self.current + 40 && matches!(self.kind, Kind::Time(_)) {
            self.add_words(40);
        }
    }

    /// Backspace: one letter, or back into the previous word if it was wrong.
    pub fn backspace(&mut self) {
        if self.finished.is_some() {
            return;
        }
        if self.typed[self.current].pop().is_none() && self.can_go_back() {
            self.typed.pop();
            self.current -= 1;
        }
    }

    /// Ctrl+Backspace: the whole word (or back into a wrong previous word).
    pub fn backspace_word(&mut self) {
        if self.finished.is_some() {
            return;
        }
        if self.typed[self.current].is_empty() {
            if self.can_go_back() {
                self.typed.pop();
                self.current -= 1;
                self.typed[self.current].clear();
            }
        } else {
            self.typed[self.current].clear();
        }
    }

    fn can_go_back(&self) -> bool {
        self.current > 0 && self.typed[self.current - 1] != self.words[self.current - 1]
    }

    /// Ends a timed run once its time is up. Returns true if it just ended.
    pub fn tick(&mut self) -> bool {
        if let (Kind::Time(s), true) = (self.kind, self.running()) {
            if self.elapsed().as_secs_f32() >= s as f32 {
                self.finish();
                return true;
            }
        }
        false
    }

    fn finish(&mut self) {
        let secs = match self.kind {
            Kind::Time(s) => s as f32,
            Kind::Words(_) => self.elapsed().as_secs_f32(),
        };
        self.finished = Some(self.score_over(secs.max(0.001)));
    }

    /// Words typed exactly right (in order), for the Solar System harvest.
    pub fn correct_words(&self) -> Vec<&'static str> {
        self.typed.iter().enumerate().filter(|(i, t)| self.words.get(*i).is_some_and(|w| *t == w)).map(|(i, _)| self.words[i]).collect()
    }

    /// Live score so far.
    #[cfg(test)]
    pub fn live(&self) -> Score {
        self.finished.unwrap_or_else(|| self.score_over(self.elapsed().as_secs_f32().max(0.001)))
    }

    fn score_over(&self, secs: f32) -> Score {
        let mut s = Score { secs, ..Score::default() };
        // Characters that count towards WPM: whole correct words plus the
        // space after each, and a correct start of the word in progress.
        let mut good = 0u32;
        let mut all = 0u32;
        for (i, typed) in self.typed.iter().enumerate() {
            let word = self.words[i];
            let done = i < self.current || self.finished.is_some() && i == self.current;
            let (w, t): (Vec<char>, Vec<char>) = (word.chars().collect(), typed.chars().collect());
            for (j, c) in t.iter().enumerate() {
                match w.get(j) {
                    Some(x) if x == c => s.correct += 1,
                    Some(_) => s.incorrect += 1,
                    None => s.extra += 1,
                }
            }
            if done && t.len() < w.len() {
                s.missed += (w.len() - t.len()) as u32;
            }
            let space = u32::from(i < self.current);
            all += t.len() as u32 + space;
            if typed == word {
                good += w.len() as u32 + space;
            } else if i == self.current && w.starts_with(&t[..]) {
                good += t.len() as u32;
            }
        }
        let mins = secs / 60.0;
        s.wpm = good as f32 / 5.0 / mins;
        s.raw = all as f32 / 5.0 / mins;
        s.acc = if self.keys == 0 { 100.0 } else { 100.0 * self.keys_ok as f32 / self.keys as f32 };
        s
    }
}

// ---------------------------------------------------------------------------
// Personal bests.

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Best {
    pub kind: String,
    pub wpm: f32,
    pub acc: f32,
    pub raw: f32,
    pub date: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// Last run's length, reused next time.
    pub kind: Kind,
    pub bests: Vec<Best>,
    /// Finished runs, all time.
    pub runs: u32,
}

impl Store {
    fn path() -> std::path::PathBuf {
        crate::util::data_dir().join("typing.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), t);
        }
    }

    pub fn best(&self, kind: Kind) -> Option<&Best> {
        let k = kind.key();
        self.bests.iter().find(|b| b.kind == k)
    }

    /// Records a finished run; returns true if it is a new personal best.
    pub fn record(&mut self, kind: Kind, s: &Score) -> bool {
        self.runs += 1;
        // Runs with poor accuracy or nothing typed don't count as bests.
        if s.wpm <= 0.0 || s.acc < 75.0 {
            return false;
        }
        let new = Best { kind: kind.key(), wpm: s.wpm, acc: s.acc, raw: s.raw, date: crate::util::today() };
        match self.bests.iter_mut().find(|b| b.kind == new.kind) {
            Some(b) if b.wpm >= s.wpm => false,
            Some(b) => {
                *b = new;
                true
            }
            None => {
                self.bests.push(new);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(kind: Kind, words: &[&'static str]) -> Test {
        let mut t = Test::new(kind, 7);
        t.words = words.to_vec();
        t
    }

    fn type_str(t: &mut Test, s: &str) {
        for c in s.chars() {
            if c == ' ' { t.space() } else { t.type_char(c) }
        }
    }

    #[test]
    fn word_run_finishes_on_last_correct_letter() {
        let mut t = fixed(Kind::Words(2), &["the", "cat"]);
        type_str(&mut t, "the ca");
        assert!(t.finished.is_none());
        t.type_char('t');
        let s = t.finished.unwrap();
        assert_eq!((s.correct, s.incorrect, s.extra, s.missed), (6, 0, 0, 0));
        assert_eq!(s.acc, 100.0);
        // 3 + space + 3 = 7 good characters.
        assert!((s.wpm - 7.0 / 5.0 / (s.secs / 60.0)).abs() < 0.01);
    }

    #[test]
    fn correct_words_for_the_harvest() {
        let mut t = fixed(Kind::Words(4), &["the", "cat", "sat", "down"]);
        type_str(&mut t, "the cot sat do");
        assert_eq!(t.correct_words(), vec!["the", "sat"]);
    }

    #[test]
    fn wrong_last_word_finishes_on_space() {
        let mut t = fixed(Kind::Words(2), &["the", "cat"]);
        type_str(&mut t, "the cot");
        assert!(t.finished.is_none());
        t.space();
        let s = t.finished.unwrap();
        assert_eq!((s.correct, s.incorrect), (5, 1));
    }

    #[test]
    fn errors_extras_and_missed_letters() {
        let mut t = fixed(Kind::Words(3), &["one", "two", "six"]);
        type_str(&mut t, "onee tw ");
        let s = t.live();
        assert_eq!((s.correct, s.incorrect, s.extra, s.missed), (5, 0, 1, 1));
        // 5 correct of 8 keystrokes (o n e e space t w space): o,n,e right; e extra wrong;
        // space after "onee" wrong; t,w right; space after "tw" wrong.
        assert!((s.acc - 62.5).abs() < 0.01, "{}", s.acc);
    }

    #[test]
    fn space_at_word_start_is_ignored() {
        let mut t = fixed(Kind::Words(2), &["a", "b"]);
        t.space();
        assert_eq!(t.current, 0);
        assert!(t.started.is_none());
    }

    #[test]
    fn backspace_returns_only_into_wrong_words() {
        let mut t = fixed(Kind::Words(4), &["ab", "cd", "ef", "gh"]);
        type_str(&mut t, "ab ");
        t.backspace();
        assert_eq!(t.current, 1, "a correct word is locked in");
        type_str(&mut t, "cx ");
        t.backspace();
        assert_eq!((t.current, t.typed[1].as_str()), (1, "cx"));
        t.backspace();
        t.type_char('d');
        assert_eq!(t.typed[1], "cd");
        type_str(&mut t, " zz");
        t.backspace_word();
        assert_eq!(t.typed[2], "");
        t.backspace_word();
        assert_eq!(t.current, 2, "cd is correct now, so no going back");
    }

    #[test]
    fn extra_letters_are_capped() {
        let mut t = fixed(Kind::Words(2), &["a", "b"]);
        type_str(&mut t, &"a".repeat(40));
        assert_eq!(t.typed[0].len(), 1 + MAX_EXTRA);
    }

    #[test]
    fn timed_run_grows_and_ends() {
        let mut t = Test::new(Kind::Time(15), 42);
        let n = t.words.len();
        for _ in 0..n {
            let w = t.words[t.current];
            type_str(&mut t, w);
            t.space();
        }
        assert!(t.words.len() > n, "more words are added as you go");
        assert!(!t.tick());
        t.started = Some(Instant::now() - Duration::from_secs(16));
        assert!(t.tick());
        let s = t.finished.unwrap();
        assert_eq!(s.secs, 15.0);
        assert!(s.wpm > 0.0 && s.acc == 100.0);
        t.type_char('x');
        assert_eq!(t.typed[t.current], "", "no typing after the end");
    }

    #[test]
    fn generator_varies_and_avoids_repeats() {
        let a = Test::new(Kind::Words(50), 1);
        let b = Test::new(Kind::Words(50), 2);
        assert_eq!(a.words.len(), 50);
        assert_ne!(a.words, b.words);
        assert!(WORDS.iter().all(|w| w.chars().all(|c| c.is_ascii_lowercase())));
    }

    #[test]
    fn bests_keep_the_fastest_accurate_run() {
        let mut st = Store::default();
        let s = |wpm: f32, acc: f32| Score { wpm, acc, ..Score::default() };
        assert!(st.record(Kind::Time(30), &s(60.0, 95.0)));
        assert!(!st.record(Kind::Time(30), &s(55.0, 99.0)));
        assert!(!st.record(Kind::Time(30), &s(90.0, 50.0)), "sloppy runs don't count");
        assert!(st.record(Kind::Time(30), &s(70.0, 96.0)));
        assert!(st.record(Kind::Words(25), &s(40.0, 90.0)));
        assert_eq!(st.best(Kind::Time(30)).unwrap().wpm, 70.0);
        assert_eq!(st.runs, 5);
        let back: Store = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert_eq!(back, st);
    }
}

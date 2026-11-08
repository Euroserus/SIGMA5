//! Fuzzy matcher tuned for launcher queries.
//!
//! Scores a query as a subsequence of the target using a small dynamic
//! program (in the spirit of fzf): matches at word starts and camelCase humps
//! earn bonuses, consecutive runs earn more, gaps cost a little. So "vsc"
//! finds "Visual Studio Code", "calc" finds "Calculator", and a prefix or exact
//! match always ranks above a scattered one.

const MATCH: i32 = 16;
const GAP_START: i32 = -3;
const GAP_EXT: i32 = -1;
const BONUS_START: i32 = 10;
const BONUS_BOUNDARY: i32 = 8;
const BONUS_CAMEL: i32 = 7;
const BONUS_DIGIT: i32 = 4;
const BONUS_CONSECUTIVE: i32 = 5;
const FIRST_CHAR_MULT: i32 = 2;
const NEG: i32 = i32::MIN / 4;
const MAX_QUERY: usize = 48;

pub struct Query {
    /// Lowercased, trimmed query as typed (used for exact/prefix checks).
    raw: String,
    /// Lowercased query chars with whitespace removed (used for the DP).
    chars: Vec<char>,
}

#[inline]
fn lower(c: char) -> char {
    if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) }
}

#[inline]
fn is_sep(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.' | '/' | '\\' | '(' | ')' | '[' | ']' | ':' | ',' | '+' | '&')
}

impl Query {
    pub fn new(q: &str) -> Self {
        let raw = q.trim().to_lowercase();
        let chars = raw.chars().filter(|c| !c.is_whitespace()).take(MAX_QUERY).collect();
        Self { raw, chars }
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// The lowercased words of the query.
    pub fn words(&self) -> impl Iterator<Item = &str> {
        self.raw.split_whitespace()
    }

    /// Lowercased query characters, whitespace removed.
    pub fn chars(&self) -> &[char] {
        &self.chars
    }

    /// Returns `None` if the query is not a subsequence of `target`.
    pub fn score(&self, target: &str) -> Option<i32> {
        let q = &self.chars;
        let m = q.len();
        if m == 0 {
            return Some(0);
        }
        let t: Vec<char> = target.chars().collect();
        let n = t.len();
        if m > n {
            return None;
        }
        let tl: Vec<char> = t.iter().map(|&c| lower(c)).collect();

        // Fast reject: must be a subsequence.
        let mut qi = 0;
        for &c in &tl {
            if c == q[qi] {
                qi += 1;
                if qi == m {
                    break;
                }
            }
        }
        if qi < m {
            return None;
        }

        let bonus: Vec<i32> = (0..n)
            .map(|j| {
                if j == 0 {
                    return BONUS_START;
                }
                let (p, c) = (t[j - 1], t[j]);
                if is_sep(p) && !is_sep(c) {
                    BONUS_BOUNDARY
                } else if p.is_lowercase() && c.is_uppercase() {
                    BONUS_CAMEL
                } else if !p.is_ascii_digit() && c.is_ascii_digit() {
                    BONUS_DIGIT
                } else {
                    0
                }
            })
            .collect();

        // Rolling rows: H = best score with q[i] matched at t[j];
        // B = bonus carried along the current consecutive run.
        let mut prev_h = vec![NEG; n];
        let mut prev_b = vec![0i32; n];
        let mut cur_h = vec![NEG; n];
        let mut cur_b = vec![0i32; n];

        for i in 0..m {
            let mut run_best = NEG; // max over k <= j-2 of prev_h[k] + gap(j-k-1)
            for j in 0..n {
                if i > 0 {
                    if run_best > NEG {
                        run_best += GAP_EXT;
                    }
                    if j >= 2 && prev_h[j - 2] > NEG {
                        run_best = run_best.max(prev_h[j - 2] + GAP_START);
                    }
                }
                if tl[j] != q[i] {
                    cur_h[j] = NEG;
                    continue;
                }
                if i == 0 {
                    let lead = if j == 0 { 0 } else { (-GAP_START + j as i32 - 1).min(12) };
                    cur_h[j] = MATCH + bonus[j] * FIRST_CHAR_MULT - lead;
                    cur_b[j] = bonus[j];
                    continue;
                }
                let mut best = NEG;
                let mut best_b = 0;
                if j >= 1 && prev_h[j - 1] > NEG {
                    let b = prev_b[j - 1].max(bonus[j]).max(BONUS_CONSECUTIVE);
                    best = prev_h[j - 1] + MATCH + b;
                    best_b = b;
                }
                if run_best > NEG {
                    let g = run_best + MATCH + bonus[j];
                    if g > best {
                        best = g;
                        best_b = bonus[j];
                    }
                }
                cur_h[j] = best;
                cur_b[j] = best_b;
            }
            std::mem::swap(&mut prev_h, &mut cur_h);
            std::mem::swap(&mut prev_b, &mut cur_b);
        }

        let mut score = *prev_h.iter().max()?;
        if score <= NEG {
            return None;
        }

        let tlow: String = tl.iter().collect();
        if tlow == self.raw {
            score += 1000;
        } else if tlow.starts_with(&self.raw) {
            score += 60;
        } else if tlow.contains(&format!(" {}", self.raw)) {
            score += 25;
        }
        // Mild preference for shorter targets as a tie-breaker.
        score -= ((n - m) as i32).min(40) / 4;
        Some(score)
    }
}

#[cfg(test)]
mod tests {
    use super::Query;

    fn rank<'a>(q: &str, items: &[&'a str]) -> Vec<&'a str> {
        let q = Query::new(q);
        let mut v: Vec<(i32, &str)> =
            items.iter().filter_map(|s| q.score(s).map(|sc| (sc, *s))).collect();
        v.sort_by(|a, b| b.0.cmp(&a.0));
        v.into_iter().map(|x| x.1).collect()
    }

    #[test]
    fn rejects_non_subsequence() {
        assert!(Query::new("xyz").score("Calculator").is_none());
    }

    #[test]
    fn acronym_beats_scattered() {
        let r = rank("vsc", &["Visual Studio Code", "Avast Secure Browser"]);
        assert_eq!(r[0], "Visual Studio Code");
    }

    #[test]
    fn prefix_beats_middle() {
        let r = rank("calc", &["Recalculate Tool", "Calculator"]);
        assert_eq!(r[0], "Calculator");
    }

    #[test]
    fn exact_wins() {
        let r = rank("paint", &["Paint 3D", "Paint", "PaintShop Pro"]);
        assert_eq!(r[0], "Paint");
    }

    #[test]
    fn spaces_ignored() {
        assert!(Query::new("vs code").score("Visual Studio Code").is_some());
    }

    #[test]
    fn word_start_preferred() {
        let r = rank("code", &["Xcodez Helper", "Visual Studio Code"]);
        assert_eq!(r[0], "Visual Studio Code");
    }

    #[test]
    fn solidworks_style() {
        let r = rank("sw", &["SOLIDWORKS 2025", "Snipping Tool", "Microsoft Store"]);
        assert_eq!(r[0], "SOLIDWORKS 2025");
    }
}

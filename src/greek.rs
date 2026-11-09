//! Greek letters, SMath / Mathcad style: a Latin letter followed by the
//! Greek hotkey becomes the Greek letter on the same key of the classic
//! Symbol font (q → θ, t → τ, w → ω, ...). Pressing it again turns it back.

pub struct Letter {
    pub name: &'static str,
    pub lower: char,
    pub upper: char,
    /// The Latin key it lives on.
    pub key: char,
}

/// The 24 letters of the Greek alphabet, in order.
pub const LETTERS: &[Letter] = &[
    Letter { name: "Alpha", lower: 'α', upper: 'Α', key: 'a' },
    Letter { name: "Beta", lower: 'β', upper: 'Β', key: 'b' },
    Letter { name: "Gamma", lower: 'γ', upper: 'Γ', key: 'g' },
    Letter { name: "Delta", lower: 'δ', upper: 'Δ', key: 'd' },
    Letter { name: "Epsilon", lower: 'ε', upper: 'Ε', key: 'e' },
    Letter { name: "Zeta", lower: 'ζ', upper: 'Ζ', key: 'z' },
    Letter { name: "Eta", lower: 'η', upper: 'Η', key: 'h' },
    Letter { name: "Theta", lower: 'θ', upper: 'Θ', key: 'q' },
    Letter { name: "Iota", lower: 'ι', upper: 'Ι', key: 'i' },
    Letter { name: "Kappa", lower: 'κ', upper: 'Κ', key: 'k' },
    Letter { name: "Lambda", lower: 'λ', upper: 'Λ', key: 'l' },
    Letter { name: "Mu", lower: 'μ', upper: 'Μ', key: 'm' },
    Letter { name: "Nu", lower: 'ν', upper: 'Ν', key: 'n' },
    Letter { name: "Xi", lower: 'ξ', upper: 'Ξ', key: 'x' },
    Letter { name: "Omicron", lower: 'ο', upper: 'Ο', key: 'o' },
    Letter { name: "Pi", lower: 'π', upper: 'Π', key: 'p' },
    Letter { name: "Rho", lower: 'ρ', upper: 'Ρ', key: 'r' },
    Letter { name: "Sigma", lower: 'σ', upper: 'Σ', key: 's' },
    Letter { name: "Tau", lower: 'τ', upper: 'Τ', key: 't' },
    Letter { name: "Upsilon", lower: 'υ', upper: 'Υ', key: 'u' },
    Letter { name: "Phi", lower: 'φ', upper: 'Φ', key: 'f' },
    Letter { name: "Chi", lower: 'χ', upper: 'Χ', key: 'c' },
    Letter { name: "Psi", lower: 'ψ', upper: 'Ψ', key: 'y' },
    Letter { name: "Omega", lower: 'ω', upper: 'Ω', key: 'w' },
];

/// The Symbol-font extras on the remaining keys: j → ϕ, J → ϑ, v → ϖ, V → ς.
const VARIANTS: &[(char, char)] = &[('j', 'ϕ'), ('J', 'ϑ'), ('v', 'ϖ'), ('V', 'ς')];

/// Latin letter → Greek letter.
pub fn to_greek(c: char) -> Option<char> {
    if let Some(&(_, g)) = VARIANTS.iter().find(|(l, _)| *l == c) {
        return Some(g);
    }
    let lower = c.to_ascii_lowercase();
    let l = LETTERS.iter().find(|l| l.key == lower)?;
    Some(if c.is_ascii_uppercase() { l.upper } else { l.lower })
}

/// Greek letter → the Latin letter it came from.
pub fn to_latin(g: char) -> Option<char> {
    if let Some(&(l, _)) = VARIANTS.iter().find(|(_, x)| *x == g) {
        return Some(l);
    }
    LETTERS.iter().find_map(|l| {
        if l.lower == g {
            Some(l.key)
        } else if l.upper == g {
            Some(l.key.to_ascii_uppercase())
        } else {
            None
        }
    })
}

/// What the hotkey turns `c` into (either direction).
pub fn toggle(c: char) -> Option<char> {
    to_greek(c).or_else(|| to_latin(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_font_layout() {
        assert_eq!(to_greek('q'), Some('θ'));
        assert_eq!(to_greek('Q'), Some('Θ'));
        assert_eq!(to_greek('t'), Some('τ'));
        assert_eq!(to_greek('w'), Some('ω'));
        assert_eq!(to_greek('D'), Some('Δ'));
        assert_eq!(to_greek('m'), Some('μ'));
        assert_eq!(to_greek('j'), Some('ϕ'));
        assert_eq!(to_greek('1'), None);
    }

    #[test]
    fn every_letter_round_trips() {
        for c in ('a'..='z').chain('A'..='Z') {
            let g = to_greek(c).unwrap_or_else(|| panic!("{c} has no Greek letter"));
            assert_eq!(to_latin(g), Some(c), "{c} → {g}");
            assert_eq!(toggle(toggle(c).unwrap()), Some(c));
        }
        assert_eq!(LETTERS.len(), 24);
    }
}

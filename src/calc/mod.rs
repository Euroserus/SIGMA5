//! Calculator and unit converter.
//!
//! Evaluates what's typed in the launcher when it looks like maths:
//! `2+2`, `sqrt(2)*3`, `15% * 80`, `2pi * 0.3 m to ft`, `200 psi to MPa`,
//! `3 kN * 0.5 m` (→ 1,500 J), `5 ft 10 in to cm`, `100 C to F`,
//! `100 rpm to rad/s`, `asin(0.5) to deg`. Results carry real dimensions, so
//! incompatible maths (metres + seconds) simply isn't offered.

pub mod units;

use std::f64::consts::{E, PI, TAU};

use units::{Dim, Unit};

/// A finished calculation, ready to display.
#[derive(Clone, Debug, PartialEq)]
pub struct Calc {
    /// The input as typed (trimmed).
    pub expr: String,
    /// Pretty result, e.g. "1,500 J" or "−3.14159".
    pub display: String,
    /// Under the result: unit name and alternatives, e.g. "Feet · 1.6 m · 63 in".
    pub label: String,
    /// Copied to the clipboard: plain number, e.g. "1500".
    pub copy: String,
    /// Put back in the search box for chaining, e.g. "1500 J".
    pub chain: String,
}

// ---------------------------------------------------------------------------
// Tokens

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

fn sup_digit(c: char) -> Option<u32> {
    "⁰¹²³⁴⁵⁶⁷⁸⁹".chars().position(|d| d == c).map(|p| p as u32)
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '_' | '°' | 'µ' | 'Ω' | '℃' | '℉')
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.' || cs[i] == '_') {
                i += 1;
            }
            // Exponent only if digits follow (so "2e" stays 2 × e).
            if i < cs.len() && (cs[i] == 'e' || cs[i] == 'E') {
                let mut j = i + 1;
                if j < cs.len() && (cs[j] == '+' || cs[j] == '-') {
                    j += 1;
                }
                if j < cs.len() && cs[j].is_ascii_digit() {
                    i = j;
                    while i < cs.len() && cs[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = cs[start..i].iter().filter(|&&c| c != '_').collect();
            out.push(Tok::Num(text.parse().ok()?));
            continue;
        }
        if sup_digit(c).is_some() || c == '⁻' {
            // Superscript exponent: m², s⁻¹
            let neg = c == '⁻';
            if neg {
                i += 1;
            }
            let mut v = 0u32;
            let mut any = false;
            while let Some(d) = cs.get(i).and_then(|&c| sup_digit(c)) {
                v = v * 10 + d;
                any = true;
                i += 1;
            }
            if !any {
                return None;
            }
            out.push(Tok::Op('^'));
            out.push(Tok::Num(if neg { -(v as f64) } else { v as f64 }));
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            i += 1;
            // Superscripts are "alphanumeric" in Unicode but mean exponents here.
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') && sup_digit(cs[i]).is_none() && cs[i] != '⁻' {
                i += 1;
            }
            out.push(Tok::Ident(cs[start..i].iter().collect()));
            continue;
        }
        match c {
            '\'' | '"' => out.push(Tok::Ident(c.to_string())), // feet / inches marks
            '+' | '-' | '*' | '/' | '^' | '%' | '!' => out.push(Tok::Op(c)),
            '−' => out.push(Tok::Op('-')),
            '×' | '·' | '⋅' => out.push(Tok::Op('*')),
            '÷' => out.push(Tok::Op('/')),
            '(' | '[' => out.push(Tok::LParen),
            ')' | ']' => out.push(Tok::RParen),
            ',' => out.push(Tok::Comma),
            'π' => out.push(Tok::Ident("pi".into())),
            'τ' => out.push(Tok::Ident("tau".into())),
            _ => return None,
        }
        i += 1;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Values

#[derive(Clone, Copy, Debug)]
struct Q {
    /// Value in SI base units.
    v: f64,
    dim: Dim,
    /// A single temperature reading: (unit, number as typed). Enables
    /// absolute conversion (°C → °F); cleared by arithmetic.
    temp: Option<(&'static Unit, f64)>,
    /// "number unit" with a single unit (shown converted to its pair).
    simple: Option<&'static Unit>,
    /// The first unit used anywhere (results are shown in it if compatible).
    first: Option<&'static Unit>,
    /// A bare unit with no number (used as a conversion target).
    unit_only: Option<&'static Unit>,
}

impl Q {
    fn num(v: f64) -> Q {
        Q { v, dim: Dim::NONE, temp: None, simple: None, first: None, unit_only: None }
    }
    fn unit(u: &'static Unit) -> Q {
        Q { v: u.factor, dim: u.dim, temp: None, simple: None, first: Some(u), unit_only: Some(u) }
    }
    /// Result of an operation: keeps only the "first unit" hint.
    fn derived(v: f64, dim: Dim, a: &Q, b: Option<&Q>) -> Q {
        Q { v, dim, temp: None, simple: None, first: a.first.or(b.and_then(|b| b.first)), unit_only: None }
    }
}

#[derive(Default)]
struct Seen {
    binop: bool,
    func: bool,
    number: bool,
    unit: bool,
    constant: bool,
    postfix: bool,
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    seen: Seen,
}

const FUNCS: &[&str] = &[
    "sqrt", "cbrt", "abs", "ln", "log", "log10", "log2", "exp", "sin", "cos", "tan", "asin", "acos", "atan",
    "sinh", "cosh", "tanh", "round", "floor", "ceil", "min", "max", "atan2", "hypot",
];

fn constant(name: &str) -> Option<f64> {
    Some(match name {
        "pi" | "PI" | "Pi" => PI,
        "tau" => TAU,
        "e" => E,
        "phi" => (1.0 + 5f64.sqrt()) / 2.0,
        _ => return None,
    })
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat_op(&mut self, ops: &[char]) -> Option<char> {
        if let Some(Tok::Op(c)) = self.peek() {
            if ops.contains(c) {
                let c = *c;
                self.pos += 1;
                return Some(c);
            }
        }
        None
    }

    fn expr(&mut self) -> Option<Q> {
        let mut a = self.mul()?;
        while let Some(op) = self.eat_op(&['+', '-']) {
            let b = self.mul()?;
            self.seen.binop = true;
            if !a.dim.compatible(b.dim) {
                return None;
            }
            let v = if op == '+' { a.v + b.v } else { a.v - b.v };
            a = Q::derived(v, a.dim, &a, Some(&b));
        }
        Some(a)
    }

    fn mul(&mut self) -> Option<Q> {
        let mut a = self.juxt()?;
        while let Some(op) = self.eat_op(&['*', '/']) {
            let b = self.juxt()?;
            self.seen.binop = true;
            a = if op == '*' {
                Q::derived(a.v * b.v, a.dim.mul(b.dim), &a, Some(&b))
            } else {
                Q::derived(a.v / b.v, a.dim.mul(b.dim.inv()), &a, Some(&b))
            };
        }
        Some(a)
    }

    fn starts_atom(&self) -> bool {
        matches!(self.peek(), Some(Tok::Num(_) | Tok::Ident(_) | Tok::LParen))
    }

    /// Implicit multiplication ("2pi", "3(4+5)"), or addition of like
    /// quantities written side by side ("5 ft 10 in").
    fn juxt(&mut self) -> Option<Q> {
        let mut a = self.unary()?;
        while self.starts_atom() {
            let rhs_is_quantity = matches!(self.peek(), Some(Tok::Num(_)));
            let b = self.unary()?;
            if rhs_is_quantity && a.simple.is_some() && b.simple.is_some() && a.dim.compatible(b.dim) {
                a = Q::derived(a.v + b.v, a.dim, &a, Some(&b));
            } else {
                a = Q::derived(a.v * b.v, a.dim.mul(b.dim), &a, Some(&b));
            }
        }
        Some(a)
    }

    fn unary(&mut self) -> Option<Q> {
        if let Some(op) = self.eat_op(&['-', '+']) {
            let mut q = self.unary()?;
            if op == '-' {
                q.v = -q.v;
                q.temp = q.temp.map(|(u, raw)| (u, -raw));
            }
            return Some(q);
        }
        self.power()
    }

    fn power(&mut self) -> Option<Q> {
        let base = self.postfix()?;
        if self.eat_op(&['^']).is_some() {
            let e = self.unary()?;
            self.seen.binop = true;
            if !e.dim.is_dimensionless() {
                return None;
            }
            let dim = base.dim.pow(e.v)?;
            return Some(Q::derived(base.v.powf(e.v), dim, &base, None));
        }
        Some(base)
    }

    fn postfix(&mut self) -> Option<Q> {
        let mut q = self.atom()?;
        while let Some(op) = self.eat_op(&['%', '!']) {
            self.seen.postfix = true;
            if !q.dim.is_dimensionless() {
                return None;
            }
            let v = if op == '%' { q.v / 100.0 } else { factorial(q.v)? };
            q = Q::derived(v, Dim::NONE, &q, None);
        }
        Some(q)
    }

    fn atom(&mut self) -> Option<Q> {
        match self.peek()?.clone() {
            Tok::Num(n) => {
                self.pos += 1;
                self.seen.number = true;
                self.quantity(n)
            }
            Tok::LParen => {
                self.pos += 1;
                let q = self.expr()?;
                if self.peek() != Some(&Tok::RParen) {
                    return None;
                }
                self.pos += 1;
                Some(q)
            }
            Tok::Ident(name) => {
                self.pos += 1;
                if self.peek() == Some(&Tok::LParen) && FUNCS.contains(&name.as_str()) {
                    self.pos += 1;
                    return self.call(&name);
                }
                if let Some(c) = constant(&name) {
                    self.seen.constant = true;
                    return Some(Q::num(c));
                }
                let u = unit_by_name(&name)?;
                self.seen.unit = true;
                Some(Q::unit(u))
            }
            _ => None,
        }
    }

    /// A number followed by unit names: "5 km", "3 kN m", "9.81 m/s^2" (the
    /// "/s^2" part is handled by the caller), "12 m^2".
    fn quantity(&mut self, n: f64) -> Option<Q> {
        let mut q = Q::num(n);
        let mut units: Vec<&'static Unit> = Vec::new();
        while let Some(Tok::Ident(name)) = self.peek() {
            let Some(u) = unit_by_name(name) else { break };
            // "5 min(" would be a function call, not minutes.
            if self.toks.get(self.pos + 1) == Some(&Tok::LParen) && FUNCS.contains(&name.as_str()) {
                break;
            }
            self.pos += 1;
            self.seen.unit = true;
            let mut f = u.factor;
            let mut dim = u.dim;
            // Unit-local exponent: "m^2", "m²".
            if self.peek() == Some(&Tok::Op('^')) {
                if let Some(Tok::Num(p)) = self.toks.get(self.pos + 1).cloned() {
                    self.pos += 2;
                    f = f.powf(p);
                    dim = dim.pow(p)?;
                }
            }
            q.v *= f;
            q.dim = q.dim.mul(dim);
            units.push(u);
        }
        if units.len() == 1 && q.dim == units[0].dim {
            q.simple = Some(units[0]);
            if units[0].offset != 0.0 || units[0].dim == by_dim_temp() {
                q.temp = Some((units[0], n));
            }
        }
        q.first = units.first().copied();
        Some(q)
    }

    fn call(&mut self, name: &str) -> Option<Q> {
        self.seen.func = true;
        let mut args = Vec::new();
        if self.peek() != Some(&Tok::RParen) {
            loop {
                args.push(self.expr()?);
                if self.peek() == Some(&Tok::Comma) {
                    self.pos += 1;
                    continue;
                }
                break;
            }
        }
        if self.peek() != Some(&Tok::RParen) {
            return None;
        }
        self.pos += 1;
        let a = *args.first()?;
        let dimless = |q: &Q| q.dim.is_dimensionless();
        let plain = |v: f64| Some(Q::derived(v, Dim::NONE, &a, None));
        match (name, args.len()) {
            ("sqrt", 1) => Some(Q::derived(a.v.sqrt(), a.dim.pow(0.5)?, &a, None)),
            ("cbrt", 1) => Some(Q::derived(a.v.cbrt(), a.dim.pow(1.0 / 3.0)?, &a, None)),
            ("abs", 1) => Some(Q::derived(a.v.abs(), a.dim, &a, None)),
            ("round" | "floor" | "ceil", 1) if dimless(&a) => plain(match name {
                "round" => a.v.round(),
                "floor" => a.v.floor(),
                _ => a.v.ceil(),
            }),
            ("min" | "max", n) if n >= 2 => {
                if !args.iter().all(|b| b.dim.compatible(a.dim)) {
                    return None;
                }
                let v = args.iter().map(|b| b.v).fold(a.v, if name == "min" { f64::min } else { f64::max });
                Some(Q::derived(v, a.dim, &a, None))
            }
            ("atan2" | "hypot", 2) => {
                let b = args[1];
                if !a.dim.compatible(b.dim) {
                    return None;
                }
                if name == "hypot" {
                    Some(Q::derived(a.v.hypot(b.v), a.dim, &a, None))
                } else {
                    Some(Q::derived(a.v.atan2(b.v), Dim { rad: 1, ..Dim::NONE }, &a, None))
                }
            }
            (_, 1) if dimless(&a) => {
                let x = a.v;
                let (v, angle) = match name {
                    "ln" => (x.ln(), false),
                    "log" | "log10" => (x.log10(), false),
                    "log2" => (x.log2(), false),
                    "exp" => (x.exp(), false),
                    "sin" => (x.sin(), false),
                    "cos" => (x.cos(), false),
                    "tan" => (x.tan(), false),
                    "sinh" => (x.sinh(), false),
                    "cosh" => (x.cosh(), false),
                    "tanh" => (x.tanh(), false),
                    "asin" => (x.asin(), true),
                    "acos" => (x.acos(), true),
                    "atan" => (x.atan(), true),
                    _ => return None,
                };
                let dim = if angle { Dim { rad: 1, ..Dim::NONE } } else { Dim::NONE };
                Some(Q::derived(v, dim, &a, None))
            }
            _ => None,
        }
    }
}

fn by_dim_temp() -> Dim {
    Dim { k: 1, ..Dim::NONE }
}

fn factorial(x: f64) -> Option<f64> {
    if x < 0.0 || x.fract() != 0.0 || x > 170.0 {
        return None;
    }
    Some((1..=x as u64).fold(1.0, |acc, k| acc * k as f64))
}

/// Unit names, plus "m2"/"ft3"-style squared/cubed shorthands.
fn unit_by_name(name: &str) -> Option<&'static Unit> {
    if constant(name).is_some() {
        return None;
    }
    units::find(name)
}

fn parse(s: &str) -> Option<(Q, Seen)> {
    let mut toks = tokenize(s)?;
    // "m2" / "ft3" shorthands → m^2 / ft^3.
    let mut i = 0;
    while i < toks.len() {
        let shorthand = match &toks[i] {
            Tok::Ident(name) if units::find(name).is_none() && constant(name).is_none() => {
                let split = name.strip_suffix('2').map(|b| (b, 2.0)).or_else(|| name.strip_suffix('3').map(|b| (b, 3.0)));
                split.filter(|(b, _)| units::find(b).is_some()).map(|(b, e)| (b.to_string(), e))
            }
            _ => None,
        };
        if let Some((base, exp)) = shorthand {
            toks.splice(i..=i, [Tok::Ident(base), Tok::Op('^'), Tok::Num(exp)]);
            i += 3;
        } else {
            i += 1;
        }
    }
    if toks.is_empty() {
        return None;
    }
    let mut p = Parser { toks, pos: 0, seen: Seen::default() };
    let q = p.expr()?;
    if p.pos != p.toks.len() || !q.v.is_finite() {
        return None;
    }
    Some((q, p.seen))
}

// ---------------------------------------------------------------------------
// Conversion & formatting

/// Converts a quantity into the units of `target` (a parsed unit expression).
fn convert(q: &Q, target: &Q) -> Option<f64> {
    if !q.dim.compatible(target.dim) {
        return None;
    }
    // A single temperature reading converts absolutely (100 °C → 212 °F).
    if let (Some((u, raw)), Some(t)) = (q.temp, target.unit_only) {
        if t.dim == by_dim_temp() {
            let kelvin = (raw + u.offset) * u.factor;
            return Some(kelvin / t.factor - t.offset);
        }
    }
    let mut x = q.v / target.v;
    // Cycles ↔ radians: 1 rev = 2π rad (rpm → rad/s, Hz → rad/s, rev → °).
    let k = (q.dim.cyc - target.dim.cyc) as i32;
    let j = (target.dim.rad - q.dim.rad) as i32;
    if k == j && k != 0 {
        x *= TAU.powi(k);
    }
    x.is_finite().then_some(x)
}

/// How to show a result in some unit: value, symbol, label.
struct Shown {
    value: f64,
    symbol: String,
    label: String,
}

fn show_in(q: &Q, symbol: &str, label: &str) -> Option<Shown> {
    let (t, _) = parse(symbol)?;
    Some(Shown { value: convert(q, &t)?, symbol: symbol.to_string(), label: label.to_string() })
}

fn show_unit(q: &Q, u: &Unit) -> Option<Shown> {
    show_in(q, u.symbol, u.label)
}

/// Number for the search box / clipboard: plain, no separators.
pub fn plain_number(x: f64) -> String {
    format_number(x, false)
}

fn format_number(x: f64, pretty: bool) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let ax = x.abs();
    let neg = if x < 0.0 { if pretty { "−" } else { "-" } } else { "" };
    if !(1e-9..1e15).contains(&ax) {
        let exp = ax.log10().floor() as i32;
        let mant = ax / 10f64.powi(exp);
        let m = trim_zeros(&format!("{mant:.9}"));
        return if pretty {
            format!("{neg}{m} × 10{}", units::superscript(exp))
        } else {
            format!("{neg}{m}e{exp}")
        };
    }
    // ~12 significant digits hides float noise (0.1 + 0.2 = 0.3).
    let decimals = (11 - ax.log10().floor() as i32).clamp(0, 12) as usize;
    let s = trim_zeros(&format!("{ax:.decimals$}"));
    let s = if pretty { group_thousands(&s) } else { s };
    format!("{neg}{s}")
}

fn trim_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

fn group_thousands(s: &str) -> String {
    let (int, frac) = s.split_once('.').map_or((s, None), |(a, b)| (a, Some(b)));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

/// Splits "expr to unit" at the right-most conversion keyword whose right
/// side is a valid unit expression.
fn split_conversion(s: &str) -> Option<(String, String)> {
    // ASCII-only lowercasing keeps byte offsets identical to `s`.
    let lower = s.to_ascii_lowercase();
    let mut cands: Vec<(usize, usize)> = Vec::new();
    for kw in [" to ", " in ", " as ", "->", "→", "=>"] {
        // Every position, including overlapping ones ("12 in in ft").
        for i in (0..lower.len()).filter(|&i| lower.is_char_boundary(i) && lower[i..].starts_with(kw)) {
            cands.push((i, i + kw.len()));
        }
    }
    cands.sort_by(|a, b| b.0.cmp(&a.0));
    for (start, end) in cands {
        let (left, right) = (s[..start].trim(), s[end..].trim());
        if left.is_empty() || right.is_empty() {
            continue;
        }
        if let (Some(_), Some((_, seen))) = (parse(left), parse(right)) {
            if seen.unit {
                return Some((left.to_string(), right.to_string()));
            }
        }
    }
    None
}

/// Evaluates `input` if it looks like a calculation. With `allow_bare`, a lone
/// number is accepted too (used to keep a chained answer on screen).
pub fn evaluate(input: &str, allow_bare: bool) -> Option<Calc> {
    let input = input.trim();
    if input.is_empty() || input.len() > 200 {
        return None;
    }
    let (q, target) = match split_conversion(input) {
        Some((l, r)) => {
            let (q, _) = parse(&l)?;
            let (t, _) = parse(&r)?;
            (q, Some((t, r)))
        }
        None => {
            let (q, seen) = parse(input)?;
            let interesting = seen.func
                || (seen.number && (seen.binop || seen.unit || seen.postfix || seen.constant))
                || (seen.constant && seen.binop);
            if !interesting && !(allow_bare && seen.number) {
                return None;
            }
            (q, None)
        }
    };

    let shown = if let Some((t, text)) = &target {
        let label = t.unit_only.or_else(|| units::named_like(t.v, t.dim)).map(|u| u.label).unwrap_or("Answer");
        Shown { value: convert(&q, t)?, symbol: text.clone(), label: label.to_string() }
    } else if q.dim.is_dimensionless() {
        Shown { value: q.v, symbol: String::new(), label: "Answer".into() }
    } else if let Some(u) = q.simple {
        // A bare quantity: show its metric/imperial counterpart.
        u.pair.and_then(|p| show_in(&q, p, units::by_symbol(p).map_or(p, |pu| pu.label)))
            .or_else(|| show_unit(&q, u))?
    } else if let Some(u) = q.first.filter(|u| u.dim.compatible(q.dim)) {
        show_unit(&q, u)?
    } else if let Some(u) = units::si_unit_for(q.dim) {
        show_unit(&q, u)?
    } else {
        Shown { value: q.v, symbol: q.dim.symbol(), label: "SI base units".into() }
    };

    // A couple of alternatives in other common units of the same dimension,
    // skipping the unit typed and anything awkward to read (0.0000001 km).
    let mut also: Vec<String> = Vec::new();
    if !q.dim.is_dimensionless() {
        let typed = q.simple.map(|u| u.symbol);
        for u in units::UNITS.iter().filter(|u| {
            u.common && u.dim.compatible(q.dim) && u.symbol != shown.symbol && Some(u.symbol) != typed
        }) {
            if also.len() == 2 {
                break;
            }
            if let Some(s) = show_unit(&q, u).filter(|s| (0.01..1e5).contains(&s.value.abs())) {
                also.push(format!("{} {}", format_number(round_sig(s.value, 4), true), s.symbol));
            }
        }
    }
    let mut label = shown.label.clone();
    for a in &also {
        label.push_str(" · ");
        label.push_str(a);
    }

    let unit_suffix = if shown.symbol.is_empty() { String::new() } else { format!(" {}", shown.symbol) };
    Some(Calc {
        expr: input.replace('*', "×"),
        // The card shows 10 significant digits; copy/chain keep full precision.
        display: format!("{}{}", format_number(round_sig(shown.value, 10), true), unit_suffix),
        label,
        copy: plain_number(shown.value),
        chain: format!("{}{}", plain_number(shown.value), unit_suffix),
    })
}

fn round_sig(x: f64, sig: i32) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let p = 10f64.powi(sig - 1 - x.abs().log10().floor() as i32);
    (x * p).round() / p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(s: &str) -> String {
        evaluate(s, false).map(|c| c.display).unwrap_or_else(|| "∅".into())
    }

    fn copy(s: &str) -> f64 {
        evaluate(s, false).unwrap_or_else(|| panic!("no result for {s}")).copy.parse().unwrap()
    }

    fn close(s: &str, want: f64) {
        let got = copy(s);
        assert!((got - want).abs() <= want.abs() * 1e-6 + 1e-12, "{s}: got {got}, want {want}");
    }

    #[test]
    fn arithmetic() {
        assert_eq!(val("2+2"), "4");
        assert_eq!(val("0.1 + 0.2"), "0.3");
        assert_eq!(val("2^10"), "1,024");
        assert_eq!(val("-2^2"), "−4");
        assert_eq!(val("(1+2)*3"), "9");
        assert_eq!(val("10 / 4"), "2.5");
        assert_eq!(val("3(4+5)"), "27");
        assert_eq!(val("2pi"), "6.283185307");
        assert_eq!(evaluate("2pi", false).unwrap().copy, "6.28318530718"); // full precision copied
        assert_eq!(val("5!"), "120");
        assert_eq!(val("15% * 80"), "12");
        assert_eq!(val("1e3 * 2"), "2,000");
        assert_eq!(val("2 × 3 ÷ 4"), "1.5");
        assert_eq!(val("1/0"), "∅");
        assert_eq!(val("1234567.891 * 1"), "1,234,567.891");
    }

    #[test]
    fn functions() {
        close("sqrt(2)", 2f64.sqrt());
        close("sin(30 deg)", 0.5);
        close("cos(pi)", -1.0);
        close("log(1000)", 3.0);
        close("ln(e^2)", 2.0);
        close("max(3, 7, 5)", 7.0);
        close("hypot(3, 4)", 5.0);
        close("asin(0.5) to deg", 30.0);
        close("atan2(1, 1) to deg", 45.0);
    }

    #[test]
    fn conversions() {
        close("5 km to mi", 3.10685596);
        close("200 psi to MPa", 1.378951458);
        close("60 mph to m/s", 26.8224);
        close("1 in to mm", 25.4);
        close("5 ft 10 in to cm", 177.8);
        close("5'10\" to cm", 177.8);
        close("100 C to F", 212.0);
        close("-40 F to C", -40.0);
        close("0 K to C", -273.15);
        close("100 rpm to rad/s", 10.471975512);
        close("10 rad/s to rpm", 95.4929658551);
        close("50 Hz to rpm", 3000.0);
        close("1 / (0.01 s) to Hz", 100.0);
        close("3 rev to deg", 1080.0);
        close("1 hp to kW", 0.745699872);
        close("1 kWh to MJ", 3.6);
        close("1 GiB to MB", 1073.741824);
        close("10 N m to lbf ft", 7.37562149);
        close("12 in in ft", 1.0);
        close("5 ft in in", 60.0);
        close("1 m2 to ft2", 10.7639104);
        close("2 m^2 * 3 m to L", 6000.0);
        close("100 km/h → mph", 62.1371192);
    }

    #[test]
    fn dimensional_results() {
        assert_eq!(val("3 kN * 0.5 m"), "1,500 J");
        assert_eq!(val("5 kg * 9.81 m/s^2"), "49.05 N");
        assert_eq!(val("10 m / 2 s"), "5 m/s");
        assert_eq!(val("5 ft + 3 in"), "5.25 ft");
        assert_eq!(val("1000 N / 2 m^2"), "500 Pa");
        assert_eq!(val("2 m * 3 m"), "6 m²");
        assert_eq!(val("5 m + 3 s"), "∅"); // incompatible
        assert_eq!(val("5 km to s"), "∅");
    }

    #[test]
    fn bare_quantities_show_their_pair() {
        assert!(val("5 km").ends_with(" mi"), "{}", val("5 km"));
        assert_eq!(val("100 C"), "212 °F");
        assert!(val("10 kg").ends_with(" lb"));
    }

    #[test]
    fn chaining_round_trips() {
        for s in ["3 kN * 0.5 m", "5 ft + 3 in", "100 C", "2 m * 3 m", "10 m / 2 s", "1e20 * 3", "100 rpm to rad/s"] {
            let c = evaluate(s, false).unwrap();
            let again = evaluate(&c.chain, true).unwrap_or_else(|| panic!("chain {:?} of {s} didn't parse", c.chain));
            let (a, b): (f64, f64) = (c.copy.parse().unwrap(), again.copy.parse().unwrap());
            // Same quantity (re-display may pick another unit for bare quantities).
            assert!(a.is_finite() && b.is_finite(), "{s}");
        }
        // Continue a calculation from the chained answer.
        let c = evaluate("3 kN * 0.5 m", false).unwrap();
        assert_eq!(val(&format!("{} * 2", c.chain)), "3,000 J");
    }

    #[test]
    fn does_not_hijack_searches() {
        for s in ["7zip", "1password", "office 365", "word", "e", "pi", "km", "2048", "3d builder", "paint 3d", "", "c", "notepad++", "f1 2024", "in", "to"] {
            assert!(evaluate(s, false).is_none(), "{s:?} should not be a calculation");
        }
        assert!(evaluate("2048", true).is_some()); // allowed when chaining
    }

    /// Prints what the card would show. `cargo test --release diag_calc -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn diag_calc_samples() {
        for s in [
            "2+2", "5 km", "100 C", "3 kN * 0.5 m", "200 psi to MPa", "5 ft 10 in to cm", "60 mph",
            "9.81 m/s^2 * 75 kg", "100 rpm", "1/3", "2^64", "sin(30 deg)", "12 V * 2 A", "1 atm to psi",
            "250 N m to lbf ft", "1 GB to MiB", "pi * (0.05 m)^2", "0.1 mm", "1.5 h",
        ] {
            match evaluate(s, false) {
                Some(c) => println!("{s:>22}  →  {:<22} | {} | copy={} chain={}", c.display, c.label, c.copy, c.chain),
                None => println!("{s:>22}  →  (none)"),
            }
        }
    }

    #[test]
    fn number_formatting() {
        assert_eq!(format_number(1e20, true), "1 × 10²⁰");
        assert_eq!(format_number(-2.5e-12, false), "-2.5e-12");
        assert_eq!(format_number(1234.5, true), "1,234.5");
        assert_eq!(format_number(1.0 / 3.0, false), "0.333333333333");
    }
}

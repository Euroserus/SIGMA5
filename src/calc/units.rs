//! Units for the calculator, with dimensions for real dimensional analysis.
//!
//! Every unit is `factor` × SI base units with exponents `dim`. Angles are
//! dimensionless (radians = 1) but tracked separately as `rad` and `cyc`
//! (cycles/revolutions) so conversions like rpm → rad/s apply the 2π that
//! pure SI would lose, while Hz ↔ 1/s still work.

use std::f64::consts::PI;

/// Dimension exponents: length, mass, time, current, temperature, data (bits),
/// plus the "soft" angle counts (ignored when checking compatibility).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Dim {
    pub m: i8,
    pub kg: i8,
    pub s: i8,
    pub a: i8,
    pub k: i8,
    pub bit: i8,
    pub rad: i8,
    pub cyc: i8,
}

impl Dim {
    pub const NONE: Dim = Dim { m: 0, kg: 0, s: 0, a: 0, k: 0, bit: 0, rad: 0, cyc: 0 };

    fn base(self) -> [i8; 6] {
        [self.m, self.kg, self.s, self.a, self.k, self.bit]
    }

    /// Same physical dimension (angle counts don't matter).
    pub fn compatible(self, o: Dim) -> bool {
        self.base() == o.base()
    }

    pub fn is_dimensionless(self) -> bool {
        self.base() == [0; 6]
    }

    pub fn mul(self, o: Dim) -> Dim {
        Dim {
            m: self.m + o.m,
            kg: self.kg + o.kg,
            s: self.s + o.s,
            a: self.a + o.a,
            k: self.k + o.k,
            bit: self.bit + o.bit,
            rad: self.rad + o.rad,
            cyc: self.cyc + o.cyc,
        }
    }

    pub fn inv(self) -> Dim {
        Dim {
            m: -self.m,
            kg: -self.kg,
            s: -self.s,
            a: -self.a,
            k: -self.k,
            bit: -self.bit,
            rad: -self.rad,
            cyc: -self.cyc,
        }
    }

    /// Raises to a power; None if exponents wouldn't stay integers.
    pub fn pow(self, p: f64) -> Option<Dim> {
        let f = |e: i8| -> Option<i8> {
            let r = e as f64 * p;
            ((r - r.round()).abs() < 1e-9 && r.abs() < 100.0).then_some(r.round() as i8)
        };
        Some(Dim {
            m: f(self.m)?,
            kg: f(self.kg)?,
            s: f(self.s)?,
            a: f(self.a)?,
            k: f(self.k)?,
            bit: f(self.bit)?,
            rad: f(self.rad)?,
            cyc: f(self.cyc)?,
        })
    }

    /// e.g. "kg·m²·s⁻²".
    pub fn symbol(self) -> String {
        let mut parts = Vec::new();
        for (e, sym) in [(self.kg, "kg"), (self.m, "m"), (self.s, "s"), (self.a, "A"), (self.k, "K"), (self.bit, "bit")] {
            match e {
                0 => {}
                1 => parts.push(sym.to_string()),
                e => parts.push(format!("{sym}{}", superscript(e as i32))),
            }
        }
        parts.join("·")
    }
}

pub fn superscript(n: i32) -> String {
    n.to_string()
        .chars()
        .map(|c| match c {
            '-' => '⁻',
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            c => c,
        })
        .collect()
}

const fn d(m: i8, kg: i8, s: i8, a: i8, k: i8, bit: i8) -> Dim {
    Dim { m, kg, s, a, k, bit, rad: 0, cyc: 0 }
}

const L: Dim = d(1, 0, 0, 0, 0, 0);
const AREA: Dim = d(2, 0, 0, 0, 0, 0);
const VOL: Dim = d(3, 0, 0, 0, 0, 0);
const M: Dim = d(0, 1, 0, 0, 0, 0);
const T: Dim = d(0, 0, 1, 0, 0, 0);
const TEMP: Dim = d(0, 0, 0, 0, 1, 0);
const SPEED: Dim = d(1, 0, -1, 0, 0, 0);
const ACCEL: Dim = d(1, 0, -2, 0, 0, 0);
const FORCE: Dim = d(1, 1, -2, 0, 0, 0);
const PRESS: Dim = d(-1, 1, -2, 0, 0, 0);
const ENERGY: Dim = d(2, 1, -2, 0, 0, 0);
const POWER: Dim = d(2, 1, -3, 0, 0, 0);
const CURRENT: Dim = d(0, 0, 0, 1, 0, 0);
const VOLT: Dim = d(2, 1, -3, -1, 0, 0);
const OHM: Dim = d(2, 1, -3, -2, 0, 0);
const CHARGE: Dim = d(0, 0, 1, 1, 0, 0);
const FARAD: Dim = d(-2, -1, 4, 2, 0, 0);
const DATA: Dim = d(0, 0, 0, 0, 0, 1);
const RAD: Dim = Dim { m: 0, kg: 0, s: 0, a: 0, k: 0, bit: 0, rad: 1, cyc: 0 };
const CYC: Dim = Dim { m: 0, kg: 0, s: 0, a: 0, k: 0, bit: 0, rad: 0, cyc: 1 };
const FREQ: Dim = Dim { m: 0, kg: 0, s: -1, a: 0, k: 0, bit: 0, rad: 0, cyc: 1 };
const ANGVEL: Dim = Dim { m: 0, kg: 0, s: -1, a: 0, k: 0, bit: 0, rad: 1, cyc: 0 };

#[derive(Debug)]
pub struct Unit {
    /// Display symbol, e.g. "km/h", "°C". Must parse back as a unit expression.
    pub symbol: &'static str,
    /// Names that refer to this unit (case-sensitive first, then any case).
    pub names: &'static [&'static str],
    /// Long name shown under results, e.g. "Kilometres".
    pub label: &'static str,
    pub factor: f64,
    pub dim: Dim,
    /// Absolute offset for temperatures: K = (x + offset) × factor.
    pub offset: f64,
    /// The unit a bare quantity is converted to (metric ↔ imperial).
    pub pair: Option<&'static str>,
    /// Listed as an alternative under results of this dimension.
    pub common: bool,
}

macro_rules! opt {
    () => {
        None
    };
    ($x:literal) => {
        Some($x)
    };
}

macro_rules! units {
    ($( $sym:literal [$($n:literal),*] $label:literal, $f:expr, $dim:expr $(, off = $off:expr)? $(, pair = $pair:literal)?, $common:literal ;)*) => {
        pub const UNITS: &[Unit] = &[$(Unit {
            symbol: $sym,
            names: &[$sym, $($n),*],
            label: $label,
            factor: $f,
            dim: $dim,
            offset: 0.0 $(+ $off)?,
            pair: opt!($($pair)?),
            common: $common,
        }),*];
    };
}

const IN: f64 = 0.0254;
const FT: f64 = 0.3048;
const LB: f64 = 0.45359237;
const G0: f64 = 9.80665;
const LBF: f64 = LB * G0;
const PSI: f64 = LBF / (IN * IN);
const GAL: f64 = 3.785411784e-3;
const BTU: f64 = 1055.05585262;
const HP: f64 = 745.69987158227022;

units! {
    // Length
    "m" ["meter", "meters", "metre", "metres"] "Metres", 1.0, L, pair = "ft", true;
    "km" ["kilometer", "kilometers", "kilometre", "kilometres"] "Kilometres", 1e3, L, pair = "mi", true;
    "cm" ["centimeter", "centimeters", "centimetre", "centimetres"] "Centimetres", 1e-2, L, pair = "in", true;
    "mm" ["millimeter", "millimeters", "millimetre", "millimetres"] "Millimetres", 1e-3, L, pair = "in", true;
    "µm" ["um", "micron", "microns", "micrometer", "micrometers"] "Micrometres", 1e-6, L, pair = "mil", false;
    "nm" ["nanometer", "nanometers"] "Nanometres", 1e-9, L, false;
    "in" ["inch", "inches", "\""] "Inches", IN, L, pair = "mm", true;
    "ft" ["foot", "feet", "'"] "Feet", FT, L, pair = "m", true;
    "yd" ["yard", "yards"] "Yards", 0.9144, L, pair = "m", false;
    "mi" ["mile", "miles"] "Miles", 1609.344, L, pair = "km", true;
    "nmi" ["nauticalmile", "nauticalmiles"] "Nautical miles", 1852.0, L, pair = "km", false;
    "mil" ["mils", "thou"] "Mils", IN / 1000.0, L, pair = "µm", false;
    // Area
    "ha" ["hectare", "hectares"] "Hectares", 1e4, AREA, pair = "acre", false;
    "acre" ["acres", "ac"] "Acres", 4046.8564224, AREA, pair = "ha", false;
    // Volume
    "L" ["l", "liter", "liters", "litre", "litres"] "Litres", 1e-3, VOL, pair = "gal", true;
    "mL" ["ml", "cc", "milliliter", "milliliters", "millilitre", "millilitres"] "Millilitres", 1e-6, VOL, pair = "floz", false;
    "gal" ["gallon", "gallons"] "US gallons", GAL, VOL, pair = "L", true;
    "qt" ["quart", "quarts"] "US quarts", GAL / 4.0, VOL, pair = "L", false;
    "floz" ["fl_oz"] "US fluid ounces", GAL / 128.0, VOL, pair = "mL", false;
    // Mass
    "kg" ["kilogram", "kilograms", "kilo", "kilos"] "Kilograms", 1.0, M, pair = "lb", true;
    "g" ["gram", "grams"] "Grams", 1e-3, M, pair = "oz", true;
    "mg" ["milligram", "milligrams"] "Milligrams", 1e-6, M, false;
    "t" ["tonne", "tonnes"] "Tonnes", 1e3, M, pair = "ton", false;
    "lb" ["lbs", "lbm", "pound", "pounds"] "Pounds", LB, M, pair = "kg", true;
    "oz" ["ounce", "ounces"] "Ounces", LB / 16.0, M, pair = "g", false;
    "ton" ["tons"] "US tons", 2000.0 * LB, M, pair = "t", false;
    "slug" ["slugs"] "Slugs", LBF / FT, M, pair = "kg", false;
    // Time
    "s" ["sec", "secs", "second", "seconds"] "Seconds", 1.0, T, true;
    "ms" ["millisecond", "milliseconds"] "Milliseconds", 1e-3, T, true;
    "µs" ["us", "microsecond", "microseconds"] "Microseconds", 1e-6, T, false;
    "ns" ["nanosecond", "nanoseconds"] "Nanoseconds", 1e-9, T, false;
    "min" ["mins", "minute", "minutes"] "Minutes", 60.0, T, true;
    "h" ["hr", "hrs", "hour", "hours"] "Hours", 3600.0, T, true;
    "day" ["d", "days"] "Days", 86400.0, T, false;
    "week" ["wk", "weeks"] "Weeks", 604800.0, T, false;
    "yr" ["year", "years"] "Years", 365.25 * 86400.0, T, false;
    // Temperature (absolute when converting a single temperature)
    "K" ["kelvin"] "Kelvin", 1.0, TEMP, pair = "°C", true;
    "°C" ["C", "degC", "celsius", "℃"] "Celsius", 1.0, TEMP, off = 273.15, pair = "°F", true;
    "°F" ["F", "degF", "fahrenheit", "℉"] "Fahrenheit", 5.0 / 9.0, TEMP, off = 459.67, pair = "°C", true;
    "°R" ["degR", "rankine"] "Rankine", 5.0 / 9.0, TEMP, pair = "K", false;
    // Speed & acceleration
    "km/h" ["kph", "kmh"] "Kilometres per hour", 1000.0 / 3600.0, SPEED, pair = "mph", true;
    "mph" [] "Miles per hour", 1609.344 / 3600.0, SPEED, pair = "km/h", true;
    "kn" ["knot", "knots", "kt"] "Knots", 1852.0 / 3600.0, SPEED, pair = "km/h", false;
    "g0" ["gee", "gn"] "Standard gravity", G0, ACCEL, pair = "ft/s^2", false;
    // Force
    "N" ["newton", "newtons"] "Newtons", 1.0, FORCE, pair = "lbf", true;
    "kN" ["kilonewton", "kilonewtons"] "Kilonewtons", 1e3, FORCE, pair = "kip", true;
    "MN" ["meganewton", "meganewtons"] "Meganewtons", 1e6, FORCE, pair = "kip", false;
    "lbf" ["poundforce", "pound-force"] "Pounds-force", LBF, FORCE, pair = "N", true;
    "kip" ["kips"] "Kips", 1000.0 * LBF, FORCE, pair = "kN", false;
    "kgf" ["kilogramforce"] "Kilograms-force", G0, FORCE, pair = "N", false;
    // Pressure & stress
    "Pa" ["pascal", "pascals"] "Pascals", 1.0, PRESS, pair = "psi", true;
    "kPa" ["kilopascal", "kilopascals"] "Kilopascals", 1e3, PRESS, pair = "psi", true;
    "MPa" ["megapascal", "megapascals"] "Megapascals", 1e6, PRESS, pair = "ksi", true;
    "GPa" ["gigapascal", "gigapascals"] "Gigapascals", 1e9, PRESS, pair = "ksi", false;
    "hPa" ["hectopascal", "hectopascals"] "Hectopascals", 1e2, PRESS, pair = "inHg", false;
    "bar" ["bars"] "Bar", 1e5, PRESS, pair = "psi", true;
    "mbar" ["millibar", "millibars"] "Millibar", 1e2, PRESS, pair = "inHg", false;
    "atm" ["atmosphere", "atmospheres"] "Atmospheres", 101325.0, PRESS, pair = "kPa", false;
    "psi" [] "Pounds per sq. inch", PSI, PRESS, pair = "kPa", true;
    "ksi" [] "Kilopounds per sq. inch", 1e3 * PSI, PRESS, pair = "MPa", false;
    "torr" ["mmHg"] "Torr", 101325.0 / 760.0, PRESS, pair = "kPa", false;
    "inHg" [] "Inches of mercury", 3386.389, PRESS, pair = "hPa", false;
    // Energy & torque (same dimension)
    "J" ["joule", "joules"] "Joules", 1.0, ENERGY, pair = "BTU", true;
    "kJ" ["kilojoule", "kilojoules"] "Kilojoules", 1e3, ENERGY, pair = "BTU", true;
    "MJ" ["megajoule", "megajoules"] "Megajoules", 1e6, ENERGY, pair = "kWh", false;
    "Wh" [] "Watt-hours", 3600.0, ENERGY, pair = "kJ", false;
    "kWh" [] "Kilowatt-hours", 3.6e6, ENERGY, pair = "MJ", true;
    "cal" ["calorie", "calories"] "Calories", 4.184, ENERGY, pair = "J", false;
    "kcal" ["Cal", "kilocalorie", "kilocalories"] "Kilocalories", 4184.0, ENERGY, pair = "kJ", false;
    "BTU" ["btu", "btus"] "BTU", BTU, ENERGY, pair = "kJ", false;
    "eV" ["electronvolt", "electronvolts"] "Electronvolts", 1.602176634e-19, ENERGY, pair = "J", false;
    "N·m" ["Nm"] "Newton-metres", 1.0, ENERGY, pair = "lbf·ft", false;
    "lbf·ft" ["ftlbf", "ftlb", "lbfft", "ft·lbf", "lbft"] "Pound-force feet", LBF * FT, ENERGY, pair = "N·m", false;
    "lbf·in" ["inlbf", "inlb", "lbfin", "in·lbf"] "Pound-force inches", LBF * IN, ENERGY, pair = "N·m", false;
    // Power
    "W" ["watt", "watts"] "Watts", 1.0, POWER, pair = "hp", true;
    "kW" ["kilowatt", "kilowatts"] "Kilowatts", 1e3, POWER, pair = "hp", true;
    "MW" ["megawatt", "megawatts"] "Megawatts", 1e6, POWER, pair = "hp", false;
    "mW" ["milliwatt", "milliwatts"] "Milliwatts", 1e-3, POWER, false;
    "hp" ["horsepower"] "Horsepower", HP, POWER, pair = "kW", true;
    "BTU/h" ["btuh", "btu/h"] "BTU per hour", BTU / 3600.0, POWER, pair = "W", false;
    // Angle & rotation
    "rad" ["radian", "radians"] "Radians", 1.0, RAD, pair = "°", false;
    "°" ["deg", "degree", "degrees"] "Degrees", PI / 180.0, RAD, pair = "rad", false;
    "rev" ["revs", "revolution", "revolutions", "turn", "turns", "cycle", "cycles"] "Revolutions", 1.0, CYC, pair = "°", false;
    "rpm" [] "Revolutions per minute", 1.0 / 60.0, FREQ, pair = "rad/s", false;
    "rad/s" [] "Radians per second", 1.0, ANGVEL, pair = "rpm", false;
    "Hz" ["hertz"] "Hertz", 1.0, FREQ, pair = "rpm", true;
    "kHz" ["kilohertz"] "Kilohertz", 1e3, FREQ, false;
    "MHz" ["megahertz"] "Megahertz", 1e6, FREQ, false;
    "GHz" ["gigahertz"] "Gigahertz", 1e9, FREQ, false;
    // Electrical
    "A" ["amp", "amps", "ampere", "amperes"] "Amperes", 1.0, CURRENT, true;
    "mA" ["milliamp", "milliamps"] "Milliamperes", 1e-3, CURRENT, true;
    "V" ["volt", "volts"] "Volts", 1.0, VOLT, true;
    "mV" ["millivolt", "millivolts"] "Millivolts", 1e-3, VOLT, false;
    "kV" ["kilovolt", "kilovolts"] "Kilovolts", 1e3, VOLT, false;
    "Ω" ["ohm", "ohms"] "Ohms", 1.0, OHM, true;
    "kΩ" ["kohm", "kohms"] "Kilohms", 1e3, OHM, true;
    "MΩ" ["Mohm", "Mohms", "megohm", "megohms"] "Megohms", 1e6, OHM, false;
    "coulomb" ["coulombs"] "Coulombs", 1.0, CHARGE, false;
    "mAh" [] "Milliamp-hours", 3.6, CHARGE, false;
    "µF" ["uF", "microfarad", "microfarads"] "Microfarads", 1e-6, FARAD, false;
    "nF" ["nanofarad", "nanofarads"] "Nanofarads", 1e-9, FARAD, false;
    "pF" ["picofarad", "picofarads"] "Picofarads", 1e-12, FARAD, false;
    // Data (decimal and binary prefixes)
    "bit" ["bits"] "Bits", 1.0, DATA, false;
    "B" ["byte", "bytes"] "Bytes", 8.0, DATA, true;
    "kB" ["KB", "kilobyte", "kilobytes"] "Kilobytes", 8e3, DATA, true;
    "MB" ["megabyte", "megabytes"] "Megabytes", 8e6, DATA, true;
    "GB" ["gigabyte", "gigabytes"] "Gigabytes", 8e9, DATA, true;
    "TB" ["terabyte", "terabytes"] "Terabytes", 8e12, DATA, false;
    "KiB" ["kibibyte", "kibibytes"] "Kibibytes", 8.0 * 1024.0, DATA, false;
    "MiB" ["mebibyte", "mebibytes"] "Mebibytes", 8.0 * 1048576.0, DATA, false;
    "GiB" ["gibibyte", "gibibytes"] "Gibibytes", 8.0 * 1073741824.0, DATA, false;
}

/// Looks a unit up: exact (case-sensitive) first, then case-insensitive if unambiguous.
pub fn find(name: &str) -> Option<&'static Unit> {
    if let Some(u) = UNITS.iter().find(|u| u.names.contains(&name)) {
        return Some(u);
    }
    let mut hits = UNITS.iter().filter(|u| u.names.iter().any(|n| n.eq_ignore_ascii_case(name)));
    let first = hits.next()?;
    hits.next().is_none().then_some(first)
}

pub fn by_symbol(sym: &str) -> Option<&'static Unit> {
    UNITS.iter().find(|u| u.symbol == sym)
}

/// A named unit equal to `factor` × `dim` (e.g. "lbf ft" → Pound-force feet).
pub fn named_like(factor: f64, dim: Dim) -> Option<&'static Unit> {
    let derived: [&'static Unit; 4] = [&SQ_M, &CU_M, &M_S, &M_S2];
    UNITS.iter().chain(derived).find(|u| u.dim.compatible(dim) && (u.factor - factor).abs() <= factor.abs() * 1e-9)
}

/// Preferred SI name for a result with no unit of its own (e.g. N, J, Pa).
pub fn si_unit_for(dim: Dim) -> Option<&'static Unit> {
    const PREFERRED: &[&str] = &["m", "kg", "s", "A", "K", "N", "Pa", "J", "W", "V", "Ω", "coulomb", "B", "Hz"];
    PREFERRED.iter().filter_map(|s| by_symbol(s)).find(|u| u.dim.compatible(dim))
        .or_else(|| match (dim.m, dim.kg, dim.s, dim.a, dim.k, dim.bit) {
            (2, 0, 0, 0, 0, 0) => Some(&SQ_M),
            (3, 0, 0, 0, 0, 0) => Some(&CU_M),
            (1, 0, -1, 0, 0, 0) => Some(&M_S),
            (1, 0, -2, 0, 0, 0) => Some(&M_S2),
            _ => None,
        })
}

// Derived SI units without their own named entry.
pub static SQ_M: Unit = Unit { symbol: "m²", names: &[], label: "Square metres", factor: 1.0, dim: AREA, offset: 0.0, pair: Some("ft²"), common: false };
pub static CU_M: Unit = Unit { symbol: "m³", names: &[], label: "Cubic metres", factor: 1.0, dim: VOL, offset: 0.0, pair: Some("L"), common: false };
pub static M_S: Unit = Unit { symbol: "m/s", names: &[], label: "Metres per second", factor: 1.0, dim: SPEED, offset: 0.0, pair: Some("km/h"), common: false };
pub static M_S2: Unit = Unit { symbol: "m/s²", names: &[], label: "Metres per second²", factor: 1.0, dim: ACCEL, offset: 0.0, pair: Some("g0"), common: false };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unambiguous() {
        // Every case-sensitive name maps to exactly one unit.
        for u in UNITS {
            for n in u.names {
                let n_hits = UNITS.iter().filter(|o| o.names.contains(n)).count();
                assert_eq!(n_hits, 1, "duplicate unit name {n}");
            }
        }
    }

    #[test]
    fn pairs_exist_and_match_dimension() {
        for u in UNITS {
            if let Some(p) = u.pair {
                // Pairs may be compound expressions (e.g. "ft/s^2"); named ones must match.
                if let Some(pu) = by_symbol(p) {
                    assert!(pu.dim.compatible(u.dim), "{} → {}", u.symbol, p);
                }
            }
        }
    }

    #[test]
    fn lookup_rules() {
        assert_eq!(find("mm").unwrap().symbol, "mm");
        assert!(find("password").is_none());
        assert_eq!(find("KM").unwrap().symbol, "km");
        assert_eq!(find("PSI").unwrap().symbol, "psi");
        assert_eq!(find("C").unwrap().symbol, "°C");
        assert_eq!(find("MPa").unwrap().symbol, "MPa");
        assert_eq!(find("mpa").map(|u| u.symbol), Some("MPa"));
    }
}

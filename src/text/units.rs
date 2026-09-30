use crate::text::template;

/// Token counts the way model cards print them: 1M, 262K, 32.8K.
pub fn tokens(n: u64) -> String {
    const UNITS: [(u64, &str); 2] = [(1_000_000, "M"), (1_000, "K")];
    for (i, (unit, suffix)) in UNITS.iter().enumerate() {
        if n < *unit {
            continue;
        }
        let value = n as f64 / *unit as f64;
        let text = if value >= 100.0 {
            format!("{value:.0}")
        } else {
            format!("{value:.1}")
        };
        // Rounded up to a thousand of this unit, it reads as one of the next (999_999 → 1M).
        if text == "1000"
            && let Some((bigger, suffix)) = i.checked_sub(1).map(|j| UNITS[j])
        {
            return format!(
                "{}{suffix}",
                format!("{:.1}", n as f64 / bigger as f64).trim_end_matches(".0")
            );
        }
        return format!("{}{suffix}", text.trim_end_matches(".0"));
    }
    n.to_string()
}

/// `n` in the form that fits it: `one` for exactly one, else `many`; `{n}` is filled in.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    template::fill(if n == 1 { one } else { many }, &[("n", &n.to_string())])
}

/// Dollars with two decimals, or more when the price needs them ($0.0009); a debt reads -$1.50.
pub fn usd(value: f64) -> String {
    let sign = if value < 0.0 { "-" } else { "" };
    let text = format!("{:.6}", value.abs());
    let (whole, frac) = text.split_once('.').unwrap_or((&text, ""));
    let frac = frac.trim_end_matches('0');
    format!("{sign}${whole}.{frac:0<2}")
}

/// Millionths of a dollar, the unit crowbot counts money in, as dollars.
pub fn usd_micros(micros: impl Into<i128>) -> String {
    usd(micros.into() as f64 / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_picks_the_form_by_count() {
        let say = |n| plural(n, "{n} more line", "{n} more lines");
        assert_eq!(say(1), "1 more line");
        assert_eq!(say(2), "2 more lines");
        assert_eq!(say(0), "0 more lines");
    }

    #[test]
    fn micro_dollars_read_as_dollars_with_the_sign_first() {
        assert_eq!(usd_micros(12_300_000_i64), "$12.30");
        assert_eq!(usd_micros(-1_500_000_i64), "-$1.50");
        assert_eq!(usd_micros(900_u64), "$0.0009");
        assert_eq!(usd_micros(0_u64), "$0.00");
    }

    #[test]
    fn rounding_up_to_a_thousand_moves_to_the_next_unit() {
        assert_eq!(tokens(999_999), "1M");
        assert_eq!(tokens(999_950), "1M");
        assert_eq!(tokens(999_499), "999K");
    }

    #[test]
    fn tokens_match_model_cards() {
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(1_048_576), "1M");
        assert_eq!(tokens(262_144), "262K");
        assert_eq!(tokens(32_768), "32.8K");
        assert_eq!(tokens(272_000), "272K");
        assert_eq!(tokens(999), "999");
    }

    #[test]
    fn usd_keeps_significant_digits() {
        assert_eq!(usd(6.0), "$6.00");
        assert_eq!(usd(0.045), "$0.045");
        assert_eq!(usd(0.0009), "$0.0009");
        assert_eq!(usd(1.35), "$1.35");
        assert_eq!(usd(0.0), "$0.00");
    }
}

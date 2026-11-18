//! Making a test input from a [`Schema`] and its [`Domains`], deterministically from a seed.
//!
//! The size of the test works through the *size variables*: a variable that sets a length or a
//! count, or that another variable's constraint uses as a bound, is limited to the first `size`
//! values of its range. So `n: 1..8` gives n between 1 and 8, but never more than the size of the
//! test being generated, and small tests really are small however large the constraint is.

use std::collections::{BTreeSet, HashSet};

use crate::schema::{Domains, Env, Item, SIZE, Schema, Spec};

/// A generous bound on the numbers in one input, so a mistaken constraint cannot fill the disk.
pub const MAX_TOKENS: usize = 2_000_000;

/// A small deterministic generator (splitmix64): the same seed always gives the same numbers.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number from `lo` to `hi`, both included. `lo` must not exceed `hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        debug_assert!(lo <= hi);
        let span = (i128::from(hi) - i128::from(lo) + 1) as u128;
        let offset = u128::from(self.next_u64()) % span;
        (i128::from(lo) + offset as i128) as i64
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.range(0, i as i64) as usize;
            items.swap(i, j);
        }
    }
}

/// The variables that are limited by the test size: those in lengths and counts, and those some
/// constraint uses.
pub fn size_vars(schema: &Schema, domains: &Domains) -> BTreeSet<String> {
    let mut vars = schema.length_vars();
    for spec in domains.values() {
        vars.extend(spec.vars());
    }
    vars.remove(SIZE);
    vars
}

/// Generates one test of the given size. The result ends with a newline.
pub fn generate(schema: &Schema, domains: &Domains, seed: u64, size: u32) -> Result<String, String> {
    let mut generator = Generator {
        domains,
        limited: size_vars(schema, domains),
        rng: Rng::new(seed),
        env: Env::with_size(i64::from(size.max(1))),
        size: i64::from(size.max(1)),
        budget: MAX_TOKENS,
        lines: Vec::new(),
    };
    generator.items(&schema.items)?;
    let mut text = generator.lines.join("\n");
    text.push('\n');
    Ok(text)
}

struct Generator<'a> {
    domains: &'a Domains,
    limited: BTreeSet<String>,
    rng: Rng,
    env: Env,
    size: i64,
    /// How many more numbers (and repetitions) may be produced.
    budget: usize,
    lines: Vec<String>,
}

impl Generator<'_> {
    fn spend(&mut self, amount: usize) -> Result<(), String> {
        self.budget = self.budget.checked_sub(amount).ok_or_else(|| {
            format!("the input would have more than {MAX_TOKENS} numbers; lower the upper bounds of the constraints")
        })?;
        Ok(())
    }

    fn spec(&self, name: &str) -> Result<&Spec, String> {
        self.domains.get(name).ok_or_else(|| format!("`{name}` has no constraint"))
    }

    fn items(&mut self, items: &[Item]) -> Result<(), String> {
        for item in items {
            match item {
                Item::Scalars(names) => {
                    let mut values = Vec::with_capacity(names.len());
                    for name in names {
                        let value = self.scalar(name)?;
                        self.env.define(name, value);
                        values.push(value.to_string());
                    }
                    self.spend(names.len())?;
                    self.lines.push(values.join(" "));
                }
                Item::Array { name, len } => {
                    let len = self.length(len, name)?;
                    if len > 0 {
                        self.spend(len)?;
                        let values = self.elements(name, len)?;
                        self.lines.push(values.iter().map(i64::to_string).collect::<Vec<_>>().join(" "));
                    }
                }
                Item::Str { name, len } => {
                    let len = self.length(len, name)?;
                    if len > 0 {
                        self.spend(len)?;
                        let Spec::Chars(alphabet) = self.spec(name)? else {
                            return Err(format!("`{name}` is a string but its constraint is not `chars ...`"));
                        };
                        let alphabet = alphabet.clone();
                        let text: String = (0..len).map(|_| alphabet[self.rng.range(0, alphabet.len() as i64 - 1) as usize]).collect();
                        self.lines.push(text);
                    }
                }
                Item::Repeat { count, body } => {
                    let count = self.env.eval(count).map_err(|e| format!("in a repeat count: {e}"))?;
                    let count = usize::try_from(count).map_err(|_| format!("a repeat count came out as {count}"))?;
                    for _ in 0..count {
                        self.spend(1)?;
                        self.env.push();
                        let result = self.items(body);
                        self.env.pop();
                        result?;
                    }
                }
            }
        }
        Ok(())
    }

    fn length(&self, len: &crate::schema::Expr, name: &str) -> Result<usize, String> {
        let value = self.env.eval(len).map_err(|e| format!("in the length of `{name}`: {e}"))?;
        usize::try_from(value).map_err(|_| format!("the length of `{name}` came out as {value}"))
    }

    /// One integer, from the constraint of `name`, limited by the size if it is a size variable.
    fn scalar(&mut self, name: &str) -> Result<i64, String> {
        let Spec::Range { lo, hi, .. } = self.spec(name)? else {
            return Err(format!("`{name}` is a single number but its constraint is not `lo..hi`"));
        };
        let (lo, hi) = (lo.clone(), hi.clone());
        let lo = self.env.eval(&lo).map_err(|e| format!("in the lower bound of `{name}`: {e}"))?;
        let mut hi = self.env.eval(&hi).map_err(|e| format!("in the upper bound of `{name}`: {e}"))?;
        if lo > hi {
            return Err(format!("the range of `{name}` is empty here: {lo}..{hi}"));
        }
        if self.limited.contains(name) {
            // At size s a variable ranges over its first s values (counting from 1 at least).
            hi = hi.min(lo.max(1).saturating_add(self.size - 1));
        }
        Ok(self.rng.range(lo, hi))
    }

    /// The elements of an array of length `len`.
    fn elements(&mut self, name: &str, len: usize) -> Result<Vec<i64>, String> {
        match self.spec(name)?.clone() {
            Spec::Perm => {
                let mut values: Vec<i64> = (1..=len as i64).collect();
                self.rng.shuffle(&mut values);
                Ok(values)
            }
            Spec::Range { lo, hi, distinct, sorted } => {
                let lo = self.env.eval(&lo).map_err(|e| format!("in the lower bound of `{name}`: {e}"))?;
                let hi = self.env.eval(&hi).map_err(|e| format!("in the upper bound of `{name}`: {e}"))?;
                if lo > hi {
                    return Err(format!("the range of `{name}` is empty here: {lo}..{hi}"));
                }
                let mut values = if distinct { self.distinct(name, lo, hi, len)? } else { (0..len).map(|_| self.rng.range(lo, hi)).collect() };
                if sorted {
                    values.sort_unstable();
                }
                Ok(values)
            }
            Spec::Chars(_) => Err(format!("`{name}` is an array of numbers but its constraint is `chars ...`")),
        }
    }

    fn distinct(&mut self, name: &str, lo: i64, hi: i64, len: usize) -> Result<Vec<i64>, String> {
        let span = (i128::from(hi) - i128::from(lo) + 1) as u128;
        if (len as u128) > span {
            return Err(format!(
                "`{name}` cannot have {len} different values in {lo}..{hi} (only {span}); widen the range or lower the size"
            ));
        }
        if span <= 2 * len as u128 {
            // Nearly every value is used: shuffle them all and keep some.
            let mut all: Vec<i64> = (lo..=hi).collect();
            self.rng.shuffle(&mut all);
            all.truncate(len);
            return Ok(all);
        }
        // Plenty of room: draw until there are enough different ones (fewer than two tries each, on average).
        let mut seen = HashSet::with_capacity(len);
        let mut values = Vec::with_capacity(len);
        while values.len() < len {
            let value = self.rng.range(lo, hi);
            if seen.insert(value) {
                values.push(value);
            }
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(text: &str) -> Schema {
        let lines: Vec<&str> = text.lines().collect();
        Schema::parse(&lines).unwrap_or_else(|e| panic!("{e}"))
    }

    fn domains(pairs: &[(&str, &str)]) -> Domains {
        pairs
            .iter()
            .map(|(name, text)| (name.to_string(), Spec::parse(text).unwrap_or_else(|e| panic!("`{text}`: {e}"))))
            .collect()
    }

    fn numbers(line: &str) -> Vec<i64> {
        line.split_whitespace().map(|t| t.parse().unwrap()).collect()
    }

    // ---- the random number generator -------------------------------------------------

    #[test]
    fn the_same_seed_gives_the_same_numbers() {
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        assert!((0..100).all(|_| a.next_u64() == b.next_u64()));
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn ranges_include_both_ends_and_stay_inside() {
        let mut rng = Rng::new(7);
        let draws: Vec<i64> = (0..2000).map(|_| rng.range(-3, 3)).collect();
        assert!(draws.iter().all(|d| (-3..=3).contains(d)));
        for wanted in -3..=3 {
            assert!(draws.contains(&wanted), "{wanted} never came up");
        }
        assert_eq!(rng.range(5, 5), 5);
    }

    #[test]
    fn the_widest_range_does_not_overflow() {
        let mut rng = Rng::new(3);
        for _ in 0..100 {
            rng.range(i64::MIN, i64::MAX);
        }
    }

    #[test]
    fn shuffling_keeps_the_elements() {
        let mut rng = Rng::new(9);
        let mut items: Vec<i64> = (1..=50).collect();
        rng.shuffle(&mut items);
        assert_ne!(items, (1..=50).collect::<Vec<_>>());
        items.sort();
        assert_eq!(items, (1..=50).collect::<Vec<_>>());
    }

    // ---- generating ------------------------------------------------------------------------

    #[test]
    fn a_generated_input_has_the_lines_the_format_describes() {
        let s = schema("n m\na[n]\nstr w[m]");
        let d = domains(&[("n", "1..size"), ("m", "1..size"), ("a", "-5..5"), ("w", "chars xy")]);
        for seed in 0..50 {
            let text = generate(&s, &d, seed, 9).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.len(), 3, "{text}");
            let header = numbers(lines[0]);
            assert_eq!(header.len(), 2);
            let (n, m) = (header[0] as usize, header[1] as usize);
            assert_eq!(numbers(lines[1]).len(), n);
            assert_eq!(lines[2].len(), m);
            assert!(lines[2].chars().all(|c| c == 'x' || c == 'y'));
            assert!(numbers(lines[1]).iter().all(|v| (-5..=5).contains(v)));
            assert!(text.ends_with('\n'));
        }
    }

    #[test]
    fn the_same_seed_and_size_give_the_same_input() {
        let s = schema("n\na[n]");
        let d = domains(&[("n", "1..size"), ("a", "1..100")]);
        assert_eq!(generate(&s, &d, 5, 10), generate(&s, &d, 5, 10));
        assert_ne!(generate(&s, &d, 5, 10), generate(&s, &d, 6, 10));
    }

    #[test]
    fn a_repeat_block_is_written_as_many_times_as_asked_with_its_own_numbers() {
        let s = schema("t\nrepeat t {\n n\n a[n]\n}");
        let d = domains(&[("t", "1..4"), ("n", "1..size"), ("a", "1..9")]);
        for seed in 0..30 {
            let text = generate(&s, &d, seed, 6).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            let t = numbers(lines[0])[0] as usize;
            assert!((1..=4).contains(&t));
            assert_eq!(lines.len(), 1 + 2 * t, "{text}");
            for case in 0..t {
                let n = numbers(lines[1 + 2 * case])[0] as usize;
                assert_eq!(numbers(lines[2 + 2 * case]).len(), n, "{text}");
            }
        }
    }

    #[test]
    fn rows_blocks_and_grids_come_out_row_by_row() {
        let s = schema("n m\nrepeat n {\n str g[m]\n}\nk\nrepeat k {\n u v\n}");
        let d = domains(&[("n", "1..size"), ("m", "1..size"), ("g", "chars .#"), ("k", "1..size"), ("u", "1..n"), ("v", "1..m")]);
        for seed in 0..30 {
            let text = generate(&s, &d, seed, 7).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            let header = numbers(lines[0]);
            let (n, m) = (header[0] as usize, header[1] as usize);
            for row in &lines[1..1 + n] {
                assert_eq!(row.len(), m);
            }
            let k = numbers(lines[1 + n])[0] as usize;
            assert_eq!(lines.len(), 2 + n + k);
            for edge in &lines[2 + n..] {
                let uv = numbers(edge);
                assert!((1..=n as i64).contains(&uv[0]) && (1..=m as i64).contains(&uv[1]), "{text}");
            }
        }
    }

    #[test]
    fn size_variables_stay_within_the_size_however_large_their_range_is() {
        let s = schema("n\na[n]");
        let d = domains(&[("n", "1..1000"), ("a", "1..9")]);
        for seed in 0..200 {
            let text = generate(&s, &d, seed, 4).unwrap();
            assert!(numbers(text.lines().next().unwrap())[0] <= 4, "{text}");
        }
        // With a big size the whole range is available.
        let biggest = (0..200).map(|seed| numbers(generate(&s, &d, seed, 1000).unwrap().lines().next().unwrap())[0]).max().unwrap();
        assert!(biggest > 100, "n never got large: {biggest}");
    }

    #[test]
    fn a_range_that_starts_above_one_still_starts_there_at_size_one() {
        let s = schema("n\na[n]");
        let d = domains(&[("n", "3..50"), ("a", "1..9")]);
        for seed in 0..50 {
            let n = numbers(generate(&s, &d, seed, 1).unwrap().lines().next().unwrap())[0];
            assert_eq!(n, 3);
        }
        let d = domains(&[("n", "0..50"), ("a", "1..9")]);
        let seen: HashSet<i64> = (0..100).map(|seed| numbers(generate(&s, &d, seed, 1).unwrap().lines().next().unwrap())[0]).collect();
        assert_eq!(seen, HashSet::from([0, 1]), "a range from 0 at size 1 gives 0 or 1");
    }

    #[test]
    fn a_variable_a_bound_depends_on_is_a_size_variable_too() {
        // `n` is not a length, but `a` is bounded by it, so it grows with the size.
        let s = schema("n m\na[m]");
        let d = domains(&[("n", "1..1000"), ("m", "1..size"), ("a", "1..n")]);
        assert!(size_vars(&s, &d).contains("n"));
        for seed in 0..100 {
            let n = numbers(generate(&s, &d, seed, 3).unwrap().lines().next().unwrap())[0];
            assert!(n <= 3);
        }
    }

    #[test]
    fn element_flags_are_honoured() {
        let s = schema("n\nd[n]\ns[n]\np[n]");
        let d = domains(&[("n", "1..size"), ("d", "1..size, distinct"), ("s", "-9..9, sorted"), ("p", "perm")]);
        for seed in 0..100 {
            let text = generate(&s, &d, seed, 12).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            let n = numbers(lines[0])[0] as usize;
            let distinct = numbers(lines[1]);
            let mut unique = distinct.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), n, "distinct: {text}");
            let sorted = numbers(lines[2]);
            assert!(sorted.windows(2).all(|w| w[0] <= w[1]), "sorted: {text}");
            let mut perm = numbers(lines[3]);
            perm.sort();
            assert_eq!(perm, (1..=n as i64).collect::<Vec<_>>(), "perm: {text}");
        }
    }

    #[test]
    fn distinct_works_in_a_wide_range_and_in_a_tight_one() {
        let s = schema("n\na[n]");
        let tight = domains(&[("n", "5..5"), ("a", "1..5, distinct")]);
        let wide = domains(&[("n", "5..5"), ("a", "1..1000000000, distinct")]);
        for seed in 0..50 {
            for d in [&tight, &wide] {
                let line = generate(&s, d, seed, 5).unwrap();
                let mut values = numbers(line.lines().nth(1).unwrap());
                values.sort();
                values.dedup();
                assert_eq!(values.len(), 5);
            }
        }
    }

    #[test]
    fn an_empty_array_leaves_no_line_at_all() {
        let s = schema("n\na[n]\nk");
        let d = domains(&[("n", "0..0"), ("a", "1..9"), ("k", "7..7")]);
        assert_eq!(generate(&s, &d, 1, 5).unwrap(), "0\n7\n");
    }

    // ---- what goes wrong ---------------------------------------------------------------------

    #[test]
    fn impossible_constraints_are_reported_not_papered_over() {
        let s = schema("n\na[n]");
        let error = |pairs: &[(&str, &str)], size| generate(&s, &domains(pairs), 1, size).unwrap_err();
        assert!(error(&[("n", "5..1"), ("a", "1..9")], 5).contains("range of `n` is empty"));
        assert!(error(&[("n", "8..8"), ("a", "9..1")], 8).contains("range of `a` is empty"));
        assert!(error(&[("n", "8..8"), ("a", "1..3, distinct")], 8).contains("cannot have 8 different values in 1..3"));
        assert!(error(&[("n", "-3..-1"), ("a", "1..9")], 5).contains("length of `a` came out as"));
        assert!(error(&[("n", "1..size")], 5).contains("`a` has no constraint"));
    }

    #[test]
    fn a_runaway_input_is_stopped() {
        let s = schema("n\na[n]");
        let d = domains(&[("n", "1..10^12"), ("a", "1..9")]);
        let error = generate(&s, &d, 1, 1_000_000_000).unwrap_err();
        assert!(error.contains("more than"), "{error}");
    }

    #[test]
    fn a_repeat_count_that_is_too_large_is_stopped() {
        let s = schema("t\nrepeat t {\n x\n}");
        let d = domains(&[("t", "1..10^12"), ("x", "1..9")]);
        assert!(generate(&s, &d, 1, 1_000_000_000).unwrap_err().contains("more than"));
    }
}

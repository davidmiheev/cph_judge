//! Working out the format of a test input from sample inputs.
//!
//! The samples come from the `.prob` file. Inference reads them all at once, line by line, and
//! decides for each line what it is:
//!
//! 1. **an array** whose length is an earlier number (`a[n]`, `a[n-1]`),
//! 2. **a block of rows** whose count is an earlier number that nothing used yet (`repeat m { u v }`,
//!    a grid `repeat n { str s[m] }`, a matrix `repeat n { a[m] }`),
//! 3. **a string** (`str s[n]`),
//! 4. **a line of numbers** that are new variables (`n m`).
//!
//! A decision only counts if it fits *every* sample, so more samples mean fewer ambiguities. When a
//! decision leads to a dead end later, the search goes back and tries the next possibility. A
//! multitest (a first number `t`, then `t` copies of a block) is tried too.
//!
//! The rule that keeps this safe is to **guess conservatively and say so**: values that the samples
//! keep inside some range are generated inside it, anything that cannot be understood is refused
//! with a message, and every ambiguity that was resolved is reported in `notes`.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::hints::Hints;
use crate::schema::{Domains, Env, Expr, Item, Op, SIZE, Schema, Spec, VarKind, validate_domains};

// ---- reading the samples ---------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Int(i64),
    Word(String),
}

/// A sample input: its non-blank lines, split into tokens.
pub type Sample = Vec<Vec<Token>>;

/// An integer as competitive programming writes it: no plus sign, no leading zeros, fits in `i64`.
fn parse_int(token: &str) -> Option<i64> {
    let digits = token.strip_prefix('-').unwrap_or(token);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if (digits.len() > 1 && digits.starts_with('0')) || token == "-0" {
        return None;
    }
    token.parse().ok()
}

fn looks_like_float(token: &str) -> bool {
    token.bytes().any(|b| b.is_ascii_digit())
        && token.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
        && token.bytes().any(|b| matches!(b, b'.' | b'e' | b'E'))
        && token.parse::<f64>().is_ok()
}

pub fn tokenize(text: &str) -> Result<Sample, String> {
    let mut lines: Sample = Vec::new();
    let mut blank_before: Option<usize> = None;
    for (index, line) in text.lines().enumerate() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.is_empty() {
            blank_before = Some(index + 1);
            continue;
        }
        if let (Some(blank), false) = (blank_before, lines.is_empty()) {
            return Err(format!("line {blank} is blank, in the middle of the input; inference does not support blank lines"));
        }
        blank_before = None;
        let mut tokens = Vec::with_capacity(words.len());
        for word in words {
            if let Some(n) = parse_int(word) {
                tokens.push(Token::Int(n));
            } else if looks_like_float(word) {
                return Err(format!("line {} has the number `{word}`; inference does not support non-integer numbers", index + 1));
            } else {
                tokens.push(Token::Word(word.to_string()));
            }
        }
        lines.push(tokens);
    }
    Ok(lines)
}

pub(crate) fn all_ints(line: &[Token]) -> bool {
    line.iter().all(|t| matches!(t, Token::Int(_)))
}

pub(crate) fn ints(line: &[Token]) -> Vec<i64> {
    line.iter().filter_map(|t| if let Token::Int(n) = t { Some(*n) } else { None }).collect()
}

/// The single word on a line, if that is all it holds.
pub(crate) fn only_word(line: &[Token]) -> Option<&str> {
    match line {
        [Token::Word(w)] => Some(w),
        _ => None,
    }
}

// ---- names ------------------------------------------------------------------------------------

const SCALAR_NAMES: [&str; 7] = ["n", "m", "k", "q", "p", "r", "l"];
const COLUMN_NAMES: [&str; 6] = ["u", "v", "w", "x", "y", "z"];
const ARRAY_NAMES: [&str; 8] = ["a", "b", "c", "d", "e", "f", "g", "h"];
const STRING_NAMES: [&str; 2] = ["s", "w"];

#[derive(Clone)]
struct Namer {
    used: HashSet<String>,
}

impl Namer {
    fn new(reserved: &[&str]) -> Namer {
        Namer { used: reserved.iter().map(|s| s.to_string()).chain([SIZE.to_string()]).collect() }
    }

    fn take(&mut self, pool: &[&str], prefix: &str) -> String {
        if let Some(name) = pool.iter().find(|n| !self.used.contains(**n)) {
            self.used.insert(name.to_string());
            return name.to_string();
        }
        (1..).map(|i| format!("{prefix}{i}")).find(|name| self.used.insert(name.clone())).expect("endless")
    }
}

// ---- the search --------------------------------------------------------------------------------

/// What has been read so far, in every sample at once.
#[derive(Clone)]
struct State<'a> {
    samples: &'a [Sample],
    /// The next line to read, per sample.
    pos: Vec<usize>,
    /// The integer variables in scope and their values in each sample.
    names: Vec<String>,
    values: Vec<Vec<i64>>,
    /// Whether a variable has been used as a length or a count.
    used: Vec<bool>,
}

/// One way to read the next line(s).
struct Plan {
    item: Item,
    /// How many lines it takes, per sample.
    advance: Vec<usize>,
    /// The variables it defines, with their value in each sample.
    defines: Vec<(String, Vec<i64>)>,
    /// The variables it uses.
    uses: Vec<usize>,
    note: Option<String>,
}

impl<'a> State<'a> {
    fn new(samples: &'a [Sample], start: usize) -> State<'a> {
        State { samples, pos: vec![start; samples.len()], names: Vec::new(), values: vec![Vec::new(); samples.len()], used: Vec::new() }
    }

    fn line(&self, sample: usize) -> Option<&'a Vec<Token>> {
        self.samples[sample].get(self.pos[sample])
    }

    fn apply(&mut self, plan: &Plan) {
        for (s, lines) in plan.advance.iter().enumerate() {
            self.pos[s] += lines;
        }
        for (name, per_sample) in &plan.defines {
            self.names.push(name.clone());
            for (s, value) in per_sample.iter().enumerate() {
                self.values[s].push(*value);
            }
            self.used.push(false);
        }
        for &i in &plan.uses {
            self.used[i] = true;
        }
    }

    fn describe(&self, sample: usize) -> String {
        let line = self.line(sample).map(|l| l.len()).unwrap_or(0);
        let _ = line;
        format!("line {} of sample {}", self.pos[sample] + 1, sample + 1)
    }
}

fn expr_for(name: &str, offset: i64) -> Expr {
    match offset {
        0 => Expr::var(name),
        o if o > 0 => Expr::bin(Op::Add, Expr::var(name), Expr::num(o)),
        o => Expr::bin(Op::Sub, Expr::var(name), Expr::num(-o)),
    }
}

/// The ways a line of integers can be an array: its length is an earlier number, or one less.
/// (One more is not tried: with small samples it explains nearly anything.)
fn array_plans(st: &State, namer: &Namer) -> Vec<(Plan, Namer)> {
    let lines: Option<Vec<&Vec<Token>>> = (0..st.samples.len()).map(|s| st.line(s)).collect();
    let Some(lines) = lines else { return Vec::new() };
    if !lines.iter().all(|l| all_ints(l)) {
        return Vec::new();
    }
    let counts: Vec<i64> = lines.iter().map(|l| l.len() as i64).collect();

    let mut plans = Vec::new();
    for offset in [0, -1] {
        for (i, name) in st.names.iter().enumerate() {
            // The line has `count` numbers: the variable's value plus `offset`.
            if st.values.iter().zip(&counts).all(|(values, &count)| values[i] + offset == count) {
                let mut namer = namer.clone();
                plans.push((
                    Plan {
                        item: Item::Array { name: namer.take(&ARRAY_NAMES, "a"), len: expr_for(name, offset) },
                        advance: vec![1; st.samples.len()],
                        defines: Vec::new(),
                        uses: vec![i],
                        note: None,
                    },
                    namer,
                ));
            }
        }
    }
    // Say so when the length could be one of several numbers that all have the same value.
    let same: Vec<String> = plans
        .iter()
        .filter_map(|(plan, _)| match &plan.item {
            Item::Array { len: Expr::Var(name), .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    if same.len() > 1 {
        let line = st.describe(0);
        plans[0].0.note = Some(format!("{line}: its length could be {}; chose `{}`", join_or(&same), same[0]));
    }
    plans
}

fn join_or(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} or {}", init.join(", "), last),
    }
}

/// The ways the next lines can be a block of rows counted by a number nothing used yet.
fn rows_plans(st: &State, namer: &Namer) -> Vec<(Plan, Namer)> {
    let mut plans = Vec::new();
    let samples = st.samples.len();
    for (i, name) in st.names.iter().enumerate() {
        if st.used[i] {
            continue;
        }
        let counts: Vec<i64> = st.values.iter().map(|v| v[i]).collect();
        if counts.iter().any(|&c| c < 0) || counts.iter().all(|&c| c == 0) {
            continue;
        }
        let counts: Vec<usize> = counts.into_iter().map(|c| c as usize).collect();
        if !(0..samples).all(|s| st.pos[s] + counts[s] <= st.samples[s].len()) {
            continue;
        }
        let rows: Vec<&[Vec<Token>]> = (0..samples).map(|s| &st.samples[s][st.pos[s]..st.pos[s] + counts[s]]).collect();
        let mut trial = namer.clone();
        if let Some((body, body_uses)) = row_body(st, &rows, &mut trial) {
            let mut uses = vec![i];
            uses.extend(body_uses);
            plans.push((
                Plan {
                    item: Item::Repeat { count: Expr::var(name), body: vec![body] },
                    advance: counts,
                    defines: Vec::new(),
                    uses,
                    note: None,
                },
                trial,
            ));
        }
    }
    plans
}

/// What every row of a block looks like, if they are all alike.
fn row_body(st: &State, rows: &[&[Vec<Token>]], namer: &mut Namer) -> Option<(Item, Vec<usize>)> {
    let lines: Vec<&Vec<Token>> = rows.iter().flat_map(|r| r.iter()).collect();
    if lines.iter().all(|l| all_ints(l)) {
        // A width per sample that has rows; the rows of a sample must all have it.
        let mut widths: Vec<Option<usize>> = Vec::new();
        for sample_rows in rows {
            let width = sample_rows.first().map(|l| l.len());
            if sample_rows.iter().any(|l| Some(l.len()) != width) {
                return None;
            }
            widths.push(width);
        }
        // A width that is an earlier number in every sample is that number (a matrix).
        let mut var_widths = (0..st.names.len()).filter(|&j| {
            widths.iter().enumerate().all(|(s, width)| width.is_none_or(|w| st.values[s][j] == w as i64))
        });
        if let Some(j) = var_widths.next() {
            return Some((Item::Array { name: namer.take(&ARRAY_NAMES, "a"), len: Expr::var(&st.names[j]) }, vec![j]));
        }
        let known: Vec<usize> = widths.iter().flatten().copied().collect();
        let width = *known.first()?;
        if known.iter().any(|&w| w != width) {
            return None;
        }
        let names = (0..width).map(|_| namer.take(&COLUMN_NAMES, "c")).collect();
        return Some((Item::Scalars(names), Vec::new()));
    }

    let words: Option<Vec<usize>> = lines.iter().map(|l| only_word(l).map(|w| w.chars().count())).collect();
    let words = words?;
    let mut per_sample: Vec<Option<usize>> = Vec::new();
    let mut at = 0;
    for sample_rows in rows {
        let lens = &words[at..at + sample_rows.len()];
        at += sample_rows.len();
        if lens.iter().any(|&l| Some(l) != lens.first().copied()) {
            return None;
        }
        per_sample.push(lens.first().copied());
    }
    let (len, uses) = string_length(st, &per_sample)?;
    Some((Item::Str { name: namer.take(&STRING_NAMES, "s"), len }, uses))
}

/// A length for strings that have these lengths in the samples: an earlier number, or a constant.
fn string_length(st: &State, lens: &[Option<usize>]) -> Option<(Expr, Vec<usize>)> {
    if let Some(j) = (0..st.names.len()).find(|&j| lens.iter().enumerate().all(|(s, l)| l.is_none_or(|l| st.values[s][j] == l as i64))) {
        return Some((Expr::var(&st.names[j]), vec![j]));
    }
    let known: Vec<usize> = lens.iter().flatten().copied().collect();
    let first = *known.first()?;
    known.iter().all(|&l| l == first).then(|| (Expr::num(first as i64), Vec::new()))
}

fn str_plans(st: &State, namer: &Namer) -> Vec<(Plan, Namer)> {
    let lines: Option<Vec<&Vec<Token>>> = (0..st.samples.len()).map(|s| st.line(s)).collect();
    let Some(lines) = lines else { return Vec::new() };
    let lens: Option<Vec<usize>> = lines.iter().map(|l| only_word(l).map(|w| w.chars().count())).collect();
    let Some(lens) = lens else { return Vec::new() };
    let lens: Vec<Option<usize>> = lens.into_iter().map(Some).collect();
    let Some((len, uses)) = string_length(st, &lens) else { return Vec::new() };
    let mut namer = namer.clone();
    vec![(
        Plan {
            item: Item::Str { name: namer.take(&STRING_NAMES, "s"), len },
            advance: vec![1; st.samples.len()],
            defines: Vec::new(),
            uses,
            note: None,
        },
        namer,
    )]
}

fn scalars_plans(st: &State, namer: &Namer) -> Vec<(Plan, Namer)> {
    let lines: Option<Vec<&Vec<Token>>> = (0..st.samples.len()).map(|s| st.line(s)).collect();
    let Some(lines) = lines else { return Vec::new() };
    if !lines.iter().all(|l| all_ints(l)) || lines.iter().any(|l| l.len() != lines[0].len()) {
        return Vec::new();
    }
    let mut namer = namer.clone();
    let names: Vec<String> = (0..lines[0].len()).map(|_| namer.take(&SCALAR_NAMES, "v")).collect();
    let defines = names
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), lines.iter().map(|l| ints(l)[i]).collect()))
        .collect();
    vec![(
        Plan { item: Item::Scalars(names), advance: vec![1; st.samples.len()], defines, uses: Vec::new(), note: None },
        namer,
    )]
}

/// Every way to read the next line(s), the likeliest first.
fn plans(st: &State, namer: &Namer) -> Vec<(Plan, Namer)> {
    let mut all = array_plans(st, namer);
    let rows = rows_plans(st, namer);
    // Reading as an array was preferred over rows although both fit: say so.
    if !all.is_empty() && !rows.is_empty() && all[0].0.note.is_none() {
        let line = st.describe(0);
        if let Item::Repeat { count, .. } = &rows[0].0.item {
            all[0].0.note = Some(format!(
                "{line}: these lines could also be rows, repeated {count} times; add more samples or give a `format` hint if so"
            ));
        }
    }
    all.extend(rows);
    all.extend(str_plans(st, namer));
    all.extend(scalars_plans(st, namer));
    all
}

fn shorten(line: &[Token]) -> String {
    let text: Vec<String> = line
        .iter()
        .map(|t| match t {
            Token::Int(n) => n.to_string(),
            Token::Word(w) => w.clone(),
        })
        .collect();
    let text = text.join(" ");
    if text.chars().count() > 40 { format!("{}…", text.chars().take(40).collect::<String>()) } else { text }
}

/// Why no way of reading the next line works.
fn explain_unsupported(st: &State) -> String {
    for s in 0..st.samples.len() {
        let Some(line) = st.line(s) else { continue };
        let at = st.describe(s);
        if line.iter().any(|t| matches!(t, Token::Int(_))) && line.iter().any(|t| matches!(t, Token::Word(_))) {
            return format!("{at} (`{}`) mixes numbers and words, which inference does not support", shorten(line));
        }
        if !all_ints(line) && only_word(line).is_none() {
            return format!("{at} (`{}`) has several words; inference supports one string per line", shorten(line));
        }
    }
    let first = (0..st.samples.len()).find_map(|s| st.line(s).map(|l| (s, l)));
    match first {
        Some((s, line)) => {
            let at = st.describe(s);
            format!(
                "{at} (`{}`) does not fit the other samples, or its length is not given by any earlier number",
                shorten(line)
            )
        }
        None => "the samples end at different places".to_string(),
    }
}

const NODE_BUDGET: usize = 50_000;

/// Reads items until every sample ends or `limit` items have been read.
fn search(
    st: &State,
    namer: &Namer,
    items: &mut Vec<Item>,
    notes: &mut Vec<String>,
    limit: usize,
    budget: &mut usize,
) -> Result<(), String> {
    if *budget == 0 {
        return Err("the samples can be read in too many ways to try them all; add a `format` hint".to_string());
    }
    *budget -= 1;

    let ended: Vec<bool> = (0..st.samples.len()).map(|s| st.pos[s] >= st.samples[s].len()).collect();
    if ended.iter().all(|&e| e) || items.len() >= limit {
        return Ok(());
    }
    if let Some(done) = ended.iter().position(|&e| e) {
        let other = ended.iter().position(|&e| !e).expect("not all ended");
        return Err(format!(
            "sample {} has fewer lines than sample {} (it ends at line {}): the samples do not follow one format",
            done + 1,
            other + 1,
            st.pos[done]
        ));
    }

    let candidates = plans(st, namer);
    if candidates.is_empty() {
        return Err(explain_unsupported(st));
    }
    let mut first_error = None;
    for (plan, next_namer) in candidates {
        let mut next = st.clone();
        next.apply(&plan);
        let (items_before, notes_before) = (items.len(), notes.len());
        items.push(plan.item.clone());
        notes.extend(plan.note.clone());
        match search(&next, &next_namer, items, notes, limit, budget) {
            Ok(()) => return Ok(()),
            Err(e) => {
                items.truncate(items_before);
                notes.truncate(notes_before);
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.expect("there was at least one candidate"))
}

/// The most items a multitest block may have.
const MAX_BODY_ITEMS: usize = 16;

fn infer_plain(samples: &[Sample]) -> Result<(Schema, Vec<String>), String> {
    let (mut items, mut notes) = (Vec::new(), Vec::new());
    search(&State::new(samples, 0), &Namer::new(&[]), &mut items, &mut notes, usize::MAX, &mut NODE_BUDGET.clone())?;
    Ok((Schema { items }, notes))
}

fn infer_multitest(samples: &[Sample]) -> Result<(Schema, Vec<String>), String> {
    let counts: Option<Vec<i64>> = samples
        .iter()
        .map(|sample| match sample.first().map(Vec::as_slice) {
            Some([Token::Int(t)]) if (1..=100_000).contains(t) => Some(*t),
            _ => None,
        })
        .collect();
    if counts.is_none() {
        return Err("the first line of every sample must be a single positive number (the number of tests)".to_string());
    }

    let mut last_error = "the samples are not a number of tests followed by that many blocks".to_string();
    for body_items in 1..=MAX_BODY_ITEMS {
        let (mut items, mut notes) = (Vec::new(), Vec::new());
        let state = State::new(samples, 1);
        match search(&state, &Namer::new(&["t"]), &mut items, &mut notes, body_items, &mut NODE_BUDGET.clone()) {
            Ok(()) if items.len() == body_items => {}
            Ok(()) => break, // the samples ran out of lines
            Err(e) => {
                last_error = e;
                continue;
            }
        }
        let schema = Schema {
            items: vec![Item::Scalars(vec!["t".to_string()]), Item::Repeat { count: Expr::var("t"), body: items }],
        };
        match observe(&schema, samples, None) {
            Ok(_) => return Ok((schema, notes)),
            Err(e) => last_error = e,
        }
    }
    Err(last_error)
}

/// How badly a format explains the samples: the numbers nothing uses, the lengths that are more
/// than a plain number (`n-1`: a guess that small samples make easy), then the items.
fn score(schema: &Schema) -> (usize, usize, usize) {
    fn offsets(items: &[Item]) -> usize {
        items
            .iter()
            .map(|item| match item {
                Item::Scalars(_) => 0,
                Item::Array { len, .. } | Item::Str { len, .. } => usize::from(!matches!(len, Expr::Var(_) | Expr::Num(_))),
                Item::Repeat { count, body } => usize::from(!matches!(count, Expr::Var(_) | Expr::Num(_))) + offsets(body),
            })
            .sum()
    }
    let lengths = schema.length_vars();
    let unused = schema.variables().iter().filter(|v| v.kind == VarKind::Scalar && !lengths.contains(&v.name)).count();
    (unused, offsets(&schema.items), schema.lines().len())
}

/// Infers the format from the samples. `multitest` forces (`Some(true)`) or rules out
/// (`Some(false)`) reading the first number as a number of tests.
pub fn infer_structure(samples: &[Sample], multitest: Option<bool>) -> Result<(Schema, Vec<String>), String> {
    let plain = if multitest == Some(true) { Err("multitest was asked for".to_string()) } else { infer_plain(samples) };
    let multi = if multitest == Some(false) { Err("multitest was ruled out".to_string()) } else { infer_multitest(samples) };

    let (schema, mut notes) = match (plain, multi) {
        (Ok(plain), Ok(multi)) => {
            if multitest == Some(true) || score(&multi.0) < score(&plain.0) {
                let (schema, mut notes) = multi;
                notes.push("the first number is read as the number of tests (a multitest); set `\"multitest\": false` if it is not".to_string());
                (schema, notes)
            } else {
                let (schema, mut notes) = plain;
                notes.push("the input could also be a multitest (the first number is the number of tests); set `\"multitest\": true` if it is".to_string());
                (schema, notes)
            }
        }
        (Ok(one), Err(_)) | (Err(_), Ok(one)) => one,
        (Err(plain), Err(multi)) => {
            return Err(if multitest == Some(true) { multi } else { plain });
        }
    };
    schema.validate()?;
    notes.dedup();
    Ok((schema, notes))
}

// ---- reading the samples with a known format ----------------------------------------------------------

/// One occurrence of a variable in a sample.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub values: Vec<i64>,
    /// The integers in scope when it was read, with their values.
    pub visible: Vec<(String, i64)>,
}

#[derive(Clone, Debug, Default)]
pub struct VarObs {
    pub instances: Vec<Instance>,
    pub chars: BTreeSet<char>,
}

#[derive(Debug, Default)]
pub struct Observed {
    pub vars: BTreeMap<String, VarObs>,
    /// Places where a sample breaks the constraints that were given (only with `domains`).
    pub violations: Vec<String>,
}

/// What `size` is while checking samples against constraints: so large that `1..size` accepts anything.
const UNLIMITED_SIZE: i64 = 1_000_000_000;

/// Reads the samples with a known format, failing where one does not fit it, and returns what they
/// contain. With `domains`, also reports values the constraints do not allow.
pub fn observe(schema: &Schema, samples: &[Sample], domains: Option<&Domains>) -> Result<Observed, String> {
    let mut observed = Observed::default();
    for (index, sample) in samples.iter().enumerate() {
        let mut walker = Walker { sample, pos: 0, number: index + 1, env: Env::with_size(UNLIMITED_SIZE), observed: &mut observed, domains };
        walker.items(&schema.items)?;
        if walker.pos < sample.len() {
            return Err(format!("sample {}: line {} is left over: the format ends before it", index + 1, walker.pos + 1));
        }
    }
    Ok(observed)
}

struct Walker<'a> {
    sample: &'a Sample,
    pos: usize,
    number: usize,
    env: Env,
    observed: &'a mut Observed,
    domains: Option<&'a Domains>,
}

const MAX_VIOLATIONS: usize = 10;

impl Walker<'_> {
    fn fail(&self, message: String) -> String {
        format!("sample {}, line {}: {message}", self.number, self.pos + 1)
    }

    fn next_line(&mut self, what: &str) -> Result<&Vec<Token>, String> {
        let line = self.sample.get(self.pos).ok_or_else(|| format!("sample {} ends before {what}", self.number))?;
        self.pos += 1;
        Ok(line)
    }

    fn violation(&mut self, message: String) {
        if self.observed.violations.len() < MAX_VIOLATIONS {
            self.observed.violations.push(format!("sample {}: {message}", self.number));
        }
    }

    fn record(&mut self, name: &str, values: Vec<i64>) {
        let visible = self.env.visible();
        self.observed.vars.entry(name.to_string()).or_default().instances.push(Instance { values, visible });
    }

    fn items(&mut self, items: &[Item]) -> Result<(), String> {
        for item in items {
            match item {
                Item::Scalars(names) => {
                    let line = self.next_line(&format!("the line with `{}`", names.join(" ")))?.clone();
                    if !all_ints(&line) || line.len() != names.len() {
                        return Err(self.fail_back(format!("expected {} number(s) for `{}`, found `{}`", names.len(), names.join(" "), shorten(&line))));
                    }
                    for (name, value) in names.iter().zip(ints(&line)) {
                        self.check_scalar(name, value);
                        self.record(name, vec![value]);
                        self.env.define(name, value);
                    }
                }
                Item::Array { name, len } => {
                    let expected = self.length(len, name)?;
                    if expected == 0 {
                        self.record(name, Vec::new());
                        continue;
                    }
                    let line = self.next_line(&format!("the array `{name}`"))?.clone();
                    if !all_ints(&line) || line.len() != expected {
                        return Err(self.fail_back(format!("`{name}` should have {expected} number(s) (`{len}`), found `{}`", shorten(&line))));
                    }
                    let values = ints(&line);
                    self.check_array(name, &values);
                    self.record(name, values);
                }
                Item::Str { name, len } => {
                    let expected = self.length(len, name)?;
                    if expected == 0 {
                        self.record(name, Vec::new());
                        continue;
                    }
                    let line = self.next_line(&format!("the string `{name}`"))?.clone();
                    let word = only_word(&line).map(str::to_string);
                    match word {
                        Some(word) if word.chars().count() == expected => {
                            self.check_string(name, &word);
                            self.observed.vars.entry(name.clone()).or_default().chars.extend(word.chars());
                            self.record(name, Vec::new());
                        }
                        _ => {
                            return Err(self.fail_back(format!("`{name}` should be a string of {expected} character(s) (`{len}`), found `{}`", shorten(&line))));
                        }
                    }
                }
                Item::Repeat { count, body } => {
                    let times = self.env.eval(count).map_err(|e| self.fail(e))?;
                    if !(0..=10_000_000).contains(&times) {
                        return Err(self.fail(format!("a repeat count is {times}")));
                    }
                    for _ in 0..times {
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

    /// An error that points at the line just read.
    fn fail_back(&self, message: String) -> String {
        format!("sample {}, line {}: {message}", self.number, self.pos)
    }

    fn length(&self, len: &Expr, name: &str) -> Result<usize, String> {
        let value = self.env.eval(len).map_err(|e| self.fail(format!("the length of `{name}`: {e}")))?;
        usize::try_from(value).map_err(|_| self.fail(format!("the length of `{name}` is {value}")))
    }

    fn bounds(&self, name: &str, lo: &Expr, hi: &Expr) -> Option<(i64, i64)> {
        let _ = name;
        Some((self.env.eval(lo).ok()?, self.env.eval(hi).ok()?))
    }

    fn check_scalar(&mut self, name: &str, value: i64) {
        let Some(Spec::Range { lo, hi, .. }) = self.domains.and_then(|d| d.get(name)) else { return };
        if let Some((lo, hi)) = self.bounds(name, lo, hi)
            && (value < lo || value > hi)
        {
            self.violation(format!("`{name}` is {value}, outside {lo}..{hi}"));
        }
    }

    fn check_array(&mut self, name: &str, values: &[i64]) {
        match self.domains.and_then(|d| d.get(name)) {
            Some(Spec::Range { lo, hi, distinct, sorted }) => {
                if let Some((lo, hi)) = self.bounds(name, lo, hi)
                    && let Some(bad) = values.iter().find(|&&v| v < lo || v > hi)
                {
                    self.violation(format!("`{name}` contains {bad}, outside {lo}..{hi}"));
                }
                if *distinct && values.iter().collect::<HashSet<_>>().len() != values.len() {
                    self.violation(format!("`{name}` has repeated values, but is constrained to be distinct"));
                }
                if *sorted && values.windows(2).any(|w| w[0] > w[1]) {
                    self.violation(format!("`{name}` is not sorted, but is constrained to be"));
                }
            }
            Some(Spec::Perm) => {
                let mut sorted = values.to_vec();
                sorted.sort_unstable();
                if sorted != (1..=values.len() as i64).collect::<Vec<_>>() {
                    self.violation(format!("`{name}` is not a permutation of 1..{}, but is constrained to be", values.len()));
                }
            }
            _ => {}
        }
    }

    fn check_string(&mut self, name: &str, word: &str) {
        if let Some(Spec::Chars(allowed)) = self.domains.and_then(|d| d.get(name))
            && let Some(bad) = word.chars().find(|c| !allowed.contains(c))
        {
            self.violation(format!("`{name}` contains `{bad}`, which is not in its character set"));
        }
    }
}

// ---- default constraints ---------------------------------------------------------------------------------

/// Bounds for values the samples show: inside what they show, widened a little where that is
/// very likely allowed (positive numbers start at 1, numbers are at most 10).
fn hull(values: &[i64]) -> Spec {
    let (Some(&lo), Some(&hi)) = (values.iter().min(), values.iter().max()) else {
        return Spec::range(Expr::num(1), Expr::num(10));
    };
    let lo = if lo >= 1 {
        1
    } else if lo == 0 {
        0
    } else {
        lo.min(-10)
    };
    Spec::range(Expr::num(lo), Expr::num(hi.max(10)))
}

fn lower_bound(values: &[i64]) -> i64 {
    if values.contains(&0) { 0 } else { 1 }
}

/// Chooses constraints from what the samples contain.
pub fn default_domains(schema: &Schema, observed: &Observed) -> Domains {
    let variables = schema.variables();
    let lengths = schema.length_vars();
    let mut array_len_vars: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    fn collect(items: &[Item], out: &mut BTreeMap<String, BTreeSet<String>>) {
        for item in items {
            match item {
                Item::Array { name, len } | Item::Str { name, len } => {
                    out.insert(name.clone(), len.vars());
                }
                Item::Repeat { body, .. } => collect(body, out),
                Item::Scalars(_) => {}
            }
        }
    }
    collect(&schema.items, &mut array_len_vars);

    let values_of = |name: &str| -> Vec<i64> {
        observed.vars.get(name).map(|o| o.instances.iter().flat_map(|i| i.values.iter().copied()).collect()).unwrap_or_default()
    };

    // A value that stays within some earlier number in every sample is bounded by it: an index
    // into a list of n things must not exceed n.
    let mut bounds: BTreeMap<String, String> = BTreeMap::new();
    for variable in &variables {
        if variable.kind == VarKind::Str || (variable.kind == VarKind::Scalar && lengths.contains(&variable.name)) {
            continue;
        }
        let Some(obs) = observed.vars.get(&variable.name) else { continue };
        let values = values_of(&variable.name);
        if values.is_empty() || values.iter().any(|&v| v < 0) {
            continue;
        }
        // The array's own length first, then the numbers in the order they were read.
        let own = array_len_vars.get(&variable.name).cloned().unwrap_or_default();
        let candidates = variable.visible.iter().filter(|c| own.contains(*c)).chain(variable.visible.iter().filter(|c| !own.contains(*c)));
        let bound = candidates.filter(|c| **c != variable.name).find(|candidate| {
            obs.instances.iter().all(|instance| {
                instance.visible.iter().rev().find(|(n, _)| n == *candidate).is_some_and(|(_, bound)| instance.values.iter().all(|v| v <= bound))
            })
        });
        if let Some(bound) = bound {
            bounds.insert(variable.name.clone(), bound.clone());
        }
    }
    let size_like: BTreeSet<String> = lengths.iter().cloned().chain(bounds.values().cloned()).collect();

    let mut domains = Domains::new();
    for variable in &variables {
        let name = &variable.name;
        let values = values_of(name);
        let spec = match variable.kind {
            VarKind::Str => {
                let chars: BTreeSet<char> = observed.vars.get(name).map(|o| o.chars.clone()).unwrap_or_default();
                if chars.is_empty() { Spec::Chars(('a'..='z').collect()) } else { Spec::Chars(chars.into_iter().collect()) }
            }
            VarKind::Scalar if size_like.contains(name) => {
                let hi = if name == "t" { Expr::num(5) } else { Expr::var(SIZE) };
                Spec::range(Expr::num(lower_bound(&values)), hi)
            }
            VarKind::Scalar => match bounds.get(name) {
                Some(bound) => Spec::range(Expr::num(lower_bound(&values)), Expr::var(bound)),
                None => hull(&values),
            },
            VarKind::Array => {
                let instances = observed.vars.get(name).map(|o| o.instances.as_slice()).unwrap_or_default();
                let longest = instances.iter().map(|i| i.values.len()).max().unwrap_or(0);
                let each = |test: &dyn Fn(&[i64]) -> bool| !instances.is_empty() && instances.iter().all(|i| test(&i.values));
                let is_perm = |v: &[i64]| {
                    let mut sorted = v.to_vec();
                    sorted.sort_unstable();
                    sorted == (1..=v.len() as i64).collect::<Vec<_>>()
                };
                let is_distinct = |v: &[i64]| v.iter().collect::<HashSet<_>>().len() == v.len();
                let is_sorted = |v: &[i64]| v.windows(2).all(|w| w[0] <= w[1]);

                // Properties the samples have in common are kept, if they show enough elements to mean something.
                if longest >= 3 && each(&is_perm) {
                    Spec::Perm
                } else {
                    let distinct = longest >= 4 && each(&is_distinct);
                    let sorted = longest >= 4 && each(&is_sorted);
                    let (lo, hi) = match (bounds.get(name), hull(&values)) {
                        (Some(bound), _) => (Expr::num(lower_bound(&values)), Expr::var(bound)),
                        (None, Spec::Range { lo, hi: Expr::Num(top), .. }) if distinct => {
                            // Different values need room: more of them than the size.
                            (lo, Expr::bin(Op::Add, Expr::num(top), Expr::var(SIZE)))
                        }
                        (None, Spec::Range { lo, hi, .. }) => (lo, hi),
                        (None, _) => unreachable!("hull is always a range"),
                    };
                    Spec::Range { lo, hi, distinct, sorted }
                }
            }
        };
        domains.insert(name.clone(), spec);
    }
    domains
}

// ---- putting it together -------------------------------------------------------------------------------------

/// The generator inferred from the samples and the hints.
#[derive(Debug)]
pub struct Inferred {
    pub schema: Schema,
    pub domains: Domains,
    /// Choices that were made between several possibilities.
    pub notes: Vec<String>,
    /// Things in the samples that the final constraints do not allow.
    pub warnings: Vec<String>,
    pub sample_count: usize,
    /// Whether the format came from the `format` hint rather than from the samples.
    pub format_from_hints: bool,
}

/// Builds the generator: the format from the `format` hint or the samples, default constraints from
/// the samples, then the constraints in the `vars` hint on top.
pub fn build(sample_texts: &[String], hints: &Hints) -> Result<Inferred, String> {
    let mut warnings = Vec::new();
    let mut samples = Vec::new();
    for (index, text) in sample_texts.iter().enumerate() {
        if text.trim().is_empty() {
            continue;
        }
        match tokenize(text) {
            Ok(sample) => samples.push(sample),
            Err(e) if hints.format.is_some() => warnings.push(format!("sample {} was not used: {e}", index + 1)),
            Err(e) => return Err(format!("sample {}: {e}", index + 1)),
        }
    }

    let format_from_hints = hints.format.is_some();
    let (schema, notes) = match &hints.format {
        Some(lines) => (Schema::parse(lines).map_err(|e| format!("the `format` hint: {e}"))?, Vec::new()),
        None => {
            if samples.is_empty() {
                return Err("there are no samples in the .prob file to infer the input format from".to_string());
            }
            infer_structure(&samples, hints.multitest)?
        }
    };

    let observed = observe(&schema, &samples, None).map_err(|e| {
        if format_from_hints { format!("the samples do not fit the `format` hint: {e}") } else { e }
    })?;
    let mut domains = default_domains(&schema, &observed);

    for (name, text) in &hints.vars {
        let spec = Spec::parse(text).map_err(|e| format!("the constraint of `{name}` in the `vars` hint: {e}"))?;
        domains.insert(name.clone(), spec);
    }
    validate_domains(&schema, &domains).map_err(|e| format!("the constraints: {e}"))?;

    warnings.extend(check_samples(&schema, &samples, &domains));
    Ok(Inferred { schema, domains, notes, warnings, sample_count: samples.len(), format_from_hints })
}

/// What in the samples breaks the given constraints.
pub fn check_samples(schema: &Schema, samples: &[Sample], domains: &Domains) -> Vec<String> {
    match observe(schema, samples, Some(domains)) {
        Ok(observed) => observed.violations,
        Err(e) => vec![e],
    }
}

/// What in the samples (given as text) breaks the constraints; samples that cannot be read are skipped.
pub fn warnings_for(schema: &Schema, sample_texts: &[String], domains: &Domains) -> Vec<String> {
    let samples: Vec<Sample> = sample_texts.iter().filter_map(|text| tokenize(text).ok()).filter(|s| !s.is_empty()).collect();
    check_samples(schema, &samples, domains)
}

/// The format and the constraints as text, for showing to the user.
pub fn summary(schema: &Schema, domains: &Domains) -> Vec<String> {
    let mut lines: Vec<String> = schema.lines().iter().map(|l| format!("  {l}")).collect();
    lines.push("  constraints:".to_string());
    for variable in schema.variables() {
        if let Some(spec) = domains.get(&variable.name) {
            lines.push(format!("    {}: {spec}", variable.name));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::generate;

    fn samples(texts: &[&str]) -> Vec<Sample> {
        texts.iter().map(|t| tokenize(t).unwrap_or_else(|e| panic!("{e}"))).collect()
    }

    fn infer(texts: &[&str]) -> (Vec<String>, Vec<String>) {
        let (schema, notes) = infer_structure(&samples(texts), None).unwrap_or_else(|e| panic!("{e}\n{texts:?}"));
        (schema.lines(), notes)
    }

    fn format_of(texts: &[&str]) -> String {
        infer(texts).0.join("\n")
    }

    fn error_of(texts: &[&str]) -> String {
        infer_structure(&samples(texts), None).unwrap_err()
    }

    // ---- reading tokens ------------------------------------------------------------------

    #[test]
    fn integers_are_written_the_way_competitive_programming_writes_them() {
        for (token, expected) in [("5", Some(5)), ("-5", Some(-5)), ("0", Some(0)), ("007", None), ("-0", None), ("+5", None), ("1e5", None), ("5.0", None), ("-", None), ("99999999999999999999", None)] {
            assert_eq!(parse_int(token), expected, "{token}");
        }
    }

    #[test]
    fn tokens_that_look_like_a_binary_string_are_words_not_numbers() {
        assert_eq!(tokenize("0101\n").unwrap(), vec![vec![Token::Word("0101".to_string())]]);
        assert_eq!(tokenize("101\n").unwrap(), vec![vec![Token::Int(101)]]);
    }

    #[test]
    fn floats_blank_lines_in_the_middle_and_the_like_are_refused() {
        assert!(tokenize("3\n1.5 2\n").unwrap_err().contains("non-integer"));
        assert!(tokenize("1e5\n").unwrap_err().contains("non-integer"));
        assert!(tokenize("1\n\n2\n").unwrap_err().contains("blank"));
        // Blank lines at the ends are harmless.
        assert_eq!(tokenize("\n1 2\n3\n\n\n").unwrap().len(), 2);
        // Words that merely contain digits or dots are words.
        assert!(tokenize("a.b v1 e5\n").is_ok());
    }

    // ---- inferring the format --------------------------------------------------------------

    #[test]
    fn a_count_and_an_array() {
        assert_eq!(format_of(&["5\n1 2 3 4 5\n"]), "n\na[n]");
    }

    #[test]
    fn several_samples_must_all_fit() {
        assert_eq!(format_of(&["3\n1 2 3\n", "2\n7 8\n", "4\n1 1 1 1\n"]), "n\na[n]");
    }

    #[test]
    fn two_numbers_and_an_array_of_the_second() {
        assert_eq!(format_of(&["5 2\n7 8\n"]), "n m\na[m]");
    }

    #[test]
    fn a_length_can_be_a_number_minus_one() {
        assert_eq!(format_of(&["4\n1 2 3\n", "3\n5 6\n"]), "n\na[n-1]");
    }

    #[test]
    fn a_list_of_edges_needs_a_second_sample_to_be_told_from_arrays() {
        // One sample with m = 2 cannot tell two arrays of length 2 from 2 rows of 2: it picks arrays and says so.
        let (format, notes) = infer(&["3 2\n1 2\n2 3\n"]);
        assert_eq!(format.join("\n"), "n m\na[m]\nb[m]");
        assert!(notes.iter().any(|n| n.contains("could also be rows")), "{notes:?}");
        // A second sample with a different number of edges settles it.
        let (format, _) = infer(&["3 2\n1 2\n2 3\n", "4 3\n1 2\n2 3\n3 4\n"]);
        assert_eq!(format.join("\n"), "n m\nrepeat m {\n  u v\n}");
    }

    #[test]
    fn a_matrix_is_rows_of_an_array_once_the_samples_disagree_on_the_row_count() {
        let (format, _) = infer(&["2 3\n1 2 3\n4 5 6\n", "3 3\n1 2 3\n4 5 6\n7 8 9\n"]);
        assert_eq!(format.join("\n"), "n m\nrepeat n {\n  a[m]\n}");
    }

    #[test]
    fn a_grid_of_characters() {
        assert_eq!(format_of(&["2 3\n.#.\n#.#\n", "3 2\n##\n..\n#.\n"]), "n m\nrepeat n {\n  str s[m]\n}");
    }

    #[test]
    fn a_string_with_its_length() {
        assert_eq!(format_of(&["5\nhello\n", "3\nabc\n"]), "n\nstr s[n]");
        // Digits with a leading zero are a string, not a number.
        assert_eq!(format_of(&["4\n0101\n", "2\n01\n"]), "n\nstr s[n]");
    }

    #[test]
    fn a_list_of_numbers_one_per_line() {
        assert_eq!(format_of(&["3\n5\n6\n7\n", "2\n1\n2\n"]), "n\nrepeat n {\n  u\n}");
    }

    #[test]
    fn queries_follow_their_count() {
        assert_eq!(
            format_of(&["3 2\n1 2 3\n1 2\n2 3\n", "4 3\n1 1 1 1\n1 4\n2 3\n3 4\n"]),
            "n m\na[n]\nrepeat m {\n  u v\n}"
        );
    }

    #[test]
    fn a_multitest_is_found_when_the_first_number_counts_blocks() {
        let (format, notes) = infer(&["2\n3\n1 2 3\n2\n4 5\n"]);
        assert_eq!(format.join("\n"), "t\nrepeat t {\n  n\n  a[n]\n}");
        assert!(notes.iter().any(|n| n.contains("multitest")), "{notes:?}");
    }

    #[test]
    fn a_multitest_with_a_single_test_is_still_recognised_when_nothing_else_fits() {
        assert_eq!(format_of(&["1\n3\n1 2 3\n"]), "t\nrepeat t {\n  n\n  a[n]\n}");
    }

    #[test]
    fn a_multitest_whose_blocks_have_rows_and_different_lengths() {
        let (format, _) = infer(&["2\n3 2\n1 2\n2 3\n2 1\n1 2\n", "1\n4 3\n1 2\n2 3\n3 4\n"]);
        assert_eq!(format.join("\n"), "t\nrepeat t {\n  n m\n  repeat m {\n    u v\n  }\n}");
    }

    #[test]
    fn an_ordinary_input_is_not_mistaken_for_a_multitest() {
        // "1\n5" is n = 1 and one number; as a multitest the body "5" would leave a number unused.
        assert_eq!(format_of(&["1\n5\n"]), "n\na[n]");
    }

    #[test]
    fn the_multitest_hint_decides_when_the_samples_cannot() {
        let s = samples(&["1\n5\n"]);
        assert_eq!(infer_structure(&s, Some(true)).unwrap().0.lines().join("\n"), "t\nrepeat t {\n  n\n}");
        assert_eq!(infer_structure(&s, Some(false)).unwrap().0.lines().join("\n"), "n\na[n]");
        let (_, notes) = infer_structure(&s, None).unwrap();
        assert!(notes.iter().any(|n| n.contains("could also be a multitest")), "{notes:?}");
    }

    #[test]
    fn an_ambiguous_length_is_reported_with_the_choice_made() {
        let (format, notes) = infer(&["3 3\n1 2 3\n"]);
        assert_eq!(format.join("\n"), "n m\na[n]");
        assert!(notes.iter().any(|n| n.contains("`n` or `m`") && n.contains("chose `n`")), "{notes:?}");
    }

    // ---- inputs that are refused ------------------------------------------------------------------

    #[test]
    fn what_cannot_be_understood_is_refused_with_the_line() {
        assert!(error_of(&["3\n1 a 3\n"]).contains("mixes numbers and words"));
        assert!(error_of(&["2\nfoo bar\n"]).contains("several words"));
        // One sample has numbers where the other has a word.
        assert!(error_of(&["3\n1 2 3\n", "3\nab\n"]).contains("does not fit the other samples"));
    }

    #[test]
    fn samples_that_do_not_follow_one_format_are_refused() {
        let error = error_of(&["3\n1 2 3\n", "3\n1 2 3\n4\n"]);
        assert!(error.contains("sample"), "{error}");
        let error = error_of(&["3\n1 2 3\n7\n", "3\n1 2 3\n"]);
        assert!(error.contains("fewer lines"), "{error}");
    }

    // ---- reading with a known format ---------------------------------------------------------------

    fn schema(lines: &[&str]) -> Schema {
        Schema::parse(lines).unwrap()
    }

    #[test]
    fn observing_records_the_values_with_what_was_in_scope() {
        let s = schema(&["n", "a[n]"]);
        let observed = observe(&s, &samples(&["3\n4 5 6\n"]), None).unwrap();
        let a = &observed.vars["a"].instances[0];
        assert_eq!(a.values, [4, 5, 6]);
        assert_eq!(a.visible, [("n".to_string(), 3)]);
    }

    #[test]
    fn observing_explains_where_a_sample_does_not_fit() {
        let s = schema(&["n", "a[n]"]);
        let error = |text: &str| observe(&s, &samples(&[text]), None).unwrap_err();
        assert!(error("3\n1 2\n").contains("sample 1, line 2") && error("3\n1 2\n").contains("`a` should have 3"));
        assert!(error("3\n").contains("ends before the array `a`"));
        assert!(error("3\n1 2 3\n9\n").contains("left over"));
        assert!(error("x\n1\n").contains("expected 1 number"));
    }

    #[test]
    fn observing_checks_constraints_and_lists_what_is_broken() {
        let s = schema(&["n", "a[n]", "str w[n]"]);
        let domains = crate::schema::Domains::from_iter([
            ("n".to_string(), Spec::parse("1..3").unwrap()),
            ("a".to_string(), Spec::parse("1..n, distinct, sorted").unwrap()),
            ("w".to_string(), Spec::parse("chars ab").unwrap()),
        ]);
        let s1 = samples(&["4\n3 3 9 1\nabcd\n"]);
        let violations = observe(&s, &s1, Some(&domains)).unwrap().violations;
        let all = violations.join("\n");
        assert!(all.contains("`n` is 4, outside 1..3"), "{all}");
        assert!(all.contains("`a` contains 9, outside 1..4"), "{all}");
        assert!(all.contains("repeated values"), "{all}");
        assert!(all.contains("not sorted"), "{all}");
        assert!(all.contains("`c`"), "{all}");
    }

    // ---- default constraints -----------------------------------------------------------------------------

    fn domains_of(lines: &[&str], texts: &[&str]) -> BTreeMap<String, String> {
        let s = schema(lines);
        let observed = observe(&s, &samples(texts), None).unwrap();
        default_domains(&s, &observed).into_iter().map(|(k, v)| (k, v.to_string())).collect()
    }

    #[test]
    fn lengths_default_to_the_size_and_values_to_what_the_samples_show() {
        let d = domains_of(&["n", "a[n]"], &["3\n1 5 9\n", "2\n4 4\n"]);
        assert_eq!(d["n"], "1..size");
        // The values are not all at most n in every sample (9 > 3), so they are not tied to it.
        assert_eq!(d["a"], "1..10");
    }

    #[test]
    fn values_that_never_exceed_a_size_variable_are_bounded_by_it() {
        // Indices into the list: every value is at most n in both samples.
        let d = domains_of(&["n", "a[n]"], &["3\n1 3 2\n", "4\n4 1 4 2\n"]);
        assert_eq!(d["a"], "1..n");
    }

    #[test]
    fn the_vertices_of_edges_are_bounded_by_the_vertex_count() {
        let d = domains_of(
            &["n m", "repeat m {", "  u v", "}"],
            &["4 3\n1 2\n2 3\n3 4\n", "3 2\n1 2\n3 1\n"],
        );
        assert_eq!(d["m"], "1..size");
        // `n` bounds the vertices, so it is a size variable too.
        assert_eq!(d["n"], "1..size");
        assert_eq!(d["u"], "1..n");
        assert_eq!(d["v"], "1..n");
    }

    #[test]
    fn negative_values_widen_the_range_downwards_and_zero_is_kept() {
        // `b` has a 0 in it and goes above n, so it is not bounded by n.
        let d = domains_of(&["n", "a[n]", "b[n]"], &["3\n-2 5 0\n0 7 9\n"]);
        assert_eq!(d["a"], "-10..10");
        assert_eq!(d["b"], "0..10");
    }

    #[test]
    fn large_values_keep_their_size() {
        let d = domains_of(&["n", "a[n]"], &["2\n1000000000 7\n"]);
        assert_eq!(d["a"], "1..1000000000");
    }

    #[test]
    fn properties_the_samples_share_are_kept_when_there_is_enough_to_see() {
        // A permutation of 1..3 in every sample.
        assert_eq!(domains_of(&["n", "a[n]"], &["3\n2 3 1\n", "4\n4 1 3 2\n"])["a"], "perm");
        // Sorted and different, with four elements to show it.
        // Different values need room, so the upper bound grows with the size.
        assert_eq!(domains_of(&["n", "a[n]"], &["4\n2 9 14 30\n"])["a"], "1..30+size, distinct, sorted");
        // Two elements prove nothing: no flags (the values stay within n, so they are bounded by it).
        assert_eq!(domains_of(&["n", "a[n]"], &["2\n1 2\n"])["a"], "1..n");
        // One sample that breaks the pattern is enough to drop it.
        assert_eq!(domains_of(&["n", "a[n]"], &["4\n1 2 3 4\n", "4\n4 1 2 3\n"])["a"], "perm");
        // Different values in both samples, but the second is not sorted: only `distinct` stays.
        assert_eq!(domains_of(&["n", "a[n]"], &["4\n1 2 3 5\n", "4\n9 1 2 3\n"])["a"], "1..10+size, distinct");
        // A repeated value in one sample drops `distinct` as well.
        assert_eq!(domains_of(&["n", "a[n]"], &["4\n1 2 3 5\n", "4\n9 1 2 2\n"])["a"], "1..10");
    }

    #[test]
    fn a_string_keeps_the_characters_it_was_seen_with() {
        let d = domains_of(&["n", "str s[n]"], &["4\n0110\n", "2\n01\n"]);
        assert_eq!(d["s"], "chars 01");
    }

    #[test]
    fn the_number_of_tests_defaults_to_a_few() {
        let d = domains_of(&["t", "repeat t {", "  n", "  a[n]", "}"], &["2\n3\n1 2 3\n2\n4 5\n"]);
        assert_eq!(d["t"], "1..5");
    }

    // ---- everything together --------------------------------------------------------------------------------

    fn texts(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_defaults_always_describe_the_samples_they_came_from() {
        for sample_set in [
            vec!["5\n1 2 3 4 5\n", "3\n7 7 7\n"],
            vec!["3 2\n1 2\n2 3\n", "4 3\n1 2\n2 3\n3 4\n"],
            vec!["2\n3\n1 2 3\n2\n4 5\n"],
            vec!["2 3\n.#.\n#.#\n", "3 2\n##\n..\n#.\n"],
            vec!["4\n1 2 3 4\n", "5\n2 4 6 8 10\n"],
        ] {
            let inferred = build(&texts(&sample_set), &Hints::default()).unwrap();
            assert!(inferred.warnings.is_empty(), "{sample_set:?}: {:?}", inferred.warnings);
        }
    }

    #[test]
    fn generated_inputs_read_back_under_the_same_format_and_constraints() {
        for sample_set in [
            vec!["5\n1 2 3 4 5\n", "3\n7 7 7\n"],
            vec!["3 2\n1 2\n2 3\n", "4 3\n1 2\n2 3\n3 4\n"],
            vec!["2\n3\n1 2 3\n2\n4 5\n"],
            vec!["2 3\n.#.\n#.#\n", "3 2\n##\n..\n#.\n"],
            vec!["4\n2 9 14 30\n"],
            vec!["3\n2 3 1\n", "4\n4 1 3 2\n"],
        ] {
            let inferred = build(&texts(&sample_set), &Hints::default()).unwrap();
            for size in [1, 2, 5, 20] {
                for seed in 0..40 {
                    let text = generate(&inferred.schema, &inferred.domains, seed, size).unwrap();
                    let sample = tokenize(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
                    let observed = observe(&inferred.schema, &[sample], Some(&inferred.domains))
                        .unwrap_or_else(|e| panic!("{sample_set:?}: {e}\n{text}"));
                    assert!(observed.violations.is_empty(), "{sample_set:?}: {:?}\n{text}", observed.violations);
                }
            }
        }
    }

    #[test]
    fn the_vars_hint_replaces_a_default() {
        let hints = Hints { vars: BTreeMap::from([("n".to_string(), "1..8".to_string())]), ..Hints::default() };
        let inferred = build(&texts(&["3\n1 2 3\n"]), &hints).unwrap();
        assert_eq!(inferred.domains["n"].to_string(), "1..8");
    }

    #[test]
    fn the_format_hint_replaces_inference_and_is_checked_against_the_samples() {
        let hints = Hints { format: Some(vec!["n".into(), "a[n]".into()]), ..Hints::default() };
        let inferred = build(&texts(&["3\n1 2 3\n"]), &hints).unwrap();
        assert!(inferred.format_from_hints);
        assert_eq!(inferred.schema.lines(), ["n", "a[n]"]);

        let error = build(&texts(&["3\n1 2\n"]), &hints).unwrap_err();
        assert!(error.contains("do not fit the `format` hint"), "{error}");
    }

    #[test]
    fn a_format_hint_works_without_samples_if_it_gives_every_constraint_it_needs() {
        let hints = Hints {
            format: Some(vec!["n".into(), "a[n]".into(), "str s[n]".into()]),
            vars: BTreeMap::from([("s".to_string(), "chars xyz".to_string())]),
            ..Hints::default()
        };
        let inferred = build(&[], &hints).unwrap();
        assert_eq!(inferred.domains["s"].to_string(), "chars x-z");
        assert_eq!(inferred.domains["n"].to_string(), "1..size", "{:?}", inferred.domains);
    }

    #[test]
    fn nothing_to_go_on_is_an_error_that_says_so() {
        assert!(build(&[], &Hints::default()).unwrap_err().contains("no samples"));
        assert!(build(&texts(&["", "  \n"]), &Hints::default()).unwrap_err().contains("no samples"));
    }

    #[test]
    fn bad_hints_are_explained() {
        let with_vars = |name: &str, text: &str| Hints { vars: BTreeMap::from([(name.to_string(), text.to_string())]), ..Hints::default() };
        let s = texts(&["3\n1 2 3\n"]);
        assert!(build(&s, &with_vars("n", "oops")).unwrap_err().contains("`n` in the `vars` hint"));
        assert!(build(&s, &with_vars("zzz", "1..2")).unwrap_err().contains("not a variable"));
        assert!(build(&s, &with_vars("a", "1..m")).unwrap_err().contains("`m`"));
        let bad_format = Hints { format: Some(vec!["n".into(), "a[m]".into()]), ..Hints::default() };
        assert!(build(&s, &bad_format).unwrap_err().contains("the `format` hint"));
    }

    #[test]
    fn constraints_that_the_samples_break_are_warned_about() {
        let hints = Hints { vars: BTreeMap::from([("n".to_string(), "1..2".to_string())]), ..Hints::default() };
        let inferred = build(&texts(&["3\n1 2 3\n"]), &hints).unwrap();
        assert!(inferred.warnings.iter().any(|w| w.contains("`n` is 3, outside 1..2")), "{:?}", inferred.warnings);
    }

    #[test]
    fn the_summary_lists_the_format_and_the_constraints() {
        let inferred = build(&texts(&["3\n1 3 2\n"]), &Hints::default()).unwrap();
        let text = summary(&inferred.schema, &inferred.domains).join("\n");
        assert!(text.contains("  n\n  a[n]\n  constraints:\n    n: 1..size\n    a: perm") || text.contains("a: 1..n"), "{text}");
    }
}

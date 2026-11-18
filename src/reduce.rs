//! Making a failing test smaller while it stays a valid test: schema-aware shrinking.
//!
//! Searching smaller *sizes* finds a small test, but not a clean one: the numbers in it are whatever the
//! random generator drew. With the input format and constraints known, the test can be taken apart and
//! reduced step by step: fewer repetitions, smaller numbers, a simpler string. Each step is a *single
//! edit* of a value tree; lengths are then re-derived from the numbers that give them and values pulled
//! back into their constraints, so a step that changes `n` also shortens the arrays that depend on it.
//! A test only counts as a candidate if it still reads back under the format with no constraint
//! broken, and the caller keeps it only if the solution still fails in the same way.

use std::collections::HashSet;

use crate::infer::{Sample, Token, all_ints, ints, observe, only_word, tokenize};
use crate::schema::{Domains, Env, Expr, Item, Schema, Spec};

/// The values of one occurrence of an item.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Scalars(Vec<i64>),
    Array(Vec<i64>),
    Str(Vec<char>),
    /// One entry per repetition, each holding the nodes of the block's items.
    Repeat(Vec<Vec<Node>>),
}

/// The values of a whole input, one node per item of the format.
pub type Doc = Vec<Node>;

/// What `size` is while reading: so large that `1..size` accepts anything.
const UNLIMITED_SIZE: i64 = 1_000_000_000;

// ---- reading and writing --------------------------------------------------------------------

pub fn parse(schema: &Schema, sample: &Sample) -> Result<Doc, String> {
    let mut reader = Reader { sample, pos: 0, env: Env::with_size(UNLIMITED_SIZE) };
    let doc = reader.items(&schema.items)?;
    if reader.pos < sample.len() {
        return Err(format!("line {} is left over", reader.pos + 1));
    }
    Ok(doc)
}

struct Reader<'a> {
    sample: &'a Sample,
    pos: usize,
    env: Env,
}

impl Reader<'_> {
    fn line(&mut self) -> Result<&Vec<Token>, String> {
        let line = self.sample.get(self.pos).ok_or("the input ends too soon")?;
        self.pos += 1;
        Ok(line)
    }

    fn length(&self, len: &Expr) -> Result<usize, String> {
        usize::try_from(self.env.eval(len)?).map_err(|_| "a negative length".to_string())
    }

    fn items(&mut self, items: &[Item]) -> Result<Doc, String> {
        let mut nodes = Vec::with_capacity(items.len());
        for item in items {
            nodes.push(match item {
                Item::Scalars(names) => {
                    let line = self.line()?.clone();
                    if !all_ints(&line) || line.len() != names.len() {
                        return Err(format!("line {} does not hold the numbers `{}`", self.pos, names.join(" ")));
                    }
                    let values = ints(&line);
                    for (name, value) in names.iter().zip(&values) {
                        self.env.define(name, *value);
                    }
                    Node::Scalars(values)
                }
                Item::Array { len, .. } => {
                    let len = self.length(len)?;
                    if len == 0 {
                        Node::Array(Vec::new())
                    } else {
                        let line = self.line()?.clone();
                        if !all_ints(&line) || line.len() != len {
                            return Err(format!("line {} is not an array of {len} numbers", self.pos));
                        }
                        Node::Array(ints(&line))
                    }
                }
                Item::Str { len, .. } => {
                    let len = self.length(len)?;
                    if len == 0 {
                        Node::Str(Vec::new())
                    } else {
                        let line = self.line()?.clone();
                        match only_word(&line) {
                            Some(word) if word.chars().count() == len => Node::Str(word.chars().collect()),
                            _ => return Err(format!("line {} is not a string of {len} characters", self.pos)),
                        }
                    }
                }
                Item::Repeat { count, body } => {
                    let count = self.length(count)?;
                    let mut iterations = Vec::with_capacity(count);
                    for _ in 0..count {
                        self.env.push();
                        let result = self.items(body);
                        self.env.pop();
                        iterations.push(result?);
                    }
                    Node::Repeat(iterations)
                }
            });
        }
        Ok(nodes)
    }
}

/// The input as text, in the form [`crate::generate::generate`] writes it.
pub fn render(schema: &Schema, doc: &Doc) -> String {
    fn walk(items: &[Item], nodes: &[Node], lines: &mut Vec<String>) {
        for (item, node) in items.iter().zip(nodes) {
            match (item, node) {
                (Item::Scalars(_), Node::Scalars(values)) | (Item::Array { .. }, Node::Array(values)) => {
                    if !values.is_empty() {
                        lines.push(values.iter().map(i64::to_string).collect::<Vec<_>>().join(" "));
                    }
                }
                (Item::Str { .. }, Node::Str(chars)) => {
                    if !chars.is_empty() {
                        lines.push(chars.iter().collect());
                    }
                }
                (Item::Repeat { body, .. }, Node::Repeat(iterations)) => {
                    for nodes in iterations {
                        walk(body, nodes, lines);
                    }
                }
                _ => {}
            }
        }
    }
    let mut lines = Vec::new();
    walk(&schema.items, doc, &mut lines);
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

// ---- keeping an edited input consistent ------------------------------------------------------------

/// Re-derives everything that follows from the numbers: lengths and counts are evaluated again and the
/// arrays, strings and repetitions cut to them, and values are pulled into their constraints. `None`
/// if that is impossible (a length that would have to grow).
pub fn normalize(schema: &Schema, domains: &Domains, doc: &Doc) -> Option<Doc> {
    fn bounds(spec: Option<&Spec>, env: &Env) -> Option<Option<(i64, i64)>> {
        match spec {
            Some(Spec::Range { lo, hi, .. }) => {
                let (lo, hi) = (env.eval(lo).ok()?, env.eval(hi).ok()?);
                (lo <= hi).then_some(Some((lo, hi)))
            }
            _ => Some(None),
        }
    }

    fn walk(items: &[Item], nodes: &[Node], domains: &Domains, env: &mut Env) -> Option<Doc> {
        if items.len() != nodes.len() {
            return None;
        }
        let mut out = Vec::with_capacity(nodes.len());
        for (item, node) in items.iter().zip(nodes) {
            out.push(match (item, node) {
                (Item::Scalars(names), Node::Scalars(values)) if names.len() == values.len() => {
                    let mut fixed = Vec::with_capacity(values.len());
                    for (name, &value) in names.iter().zip(values) {
                        let value = match bounds(domains.get(name), env)? {
                            Some((lo, hi)) => value.clamp(lo, hi),
                            None => value,
                        };
                        env.define(name, value);
                        fixed.push(value);
                    }
                    Node::Scalars(fixed)
                }
                (Item::Array { name, len }, Node::Array(values)) => {
                    let len = usize::try_from(env.eval(len).ok()?).ok()?;
                    if values.len() < len {
                        return None;
                    }
                    let spec = domains.get(name);
                    let mut kept: Vec<i64> = if matches!(spec, Some(Spec::Perm)) && values.len() > len {
                        // A permutation of 1..m cut down to 1..len: what is left of it after dropping the larger ones.
                        values.iter().copied().filter(|&v| (1..=len as i64).contains(&v)).collect()
                    } else {
                        values[..len].to_vec()
                    };
                    if kept.len() != len {
                        return None;
                    }
                    if let Some((lo, hi)) = bounds(spec, env)? {
                        kept.iter_mut().for_each(|v| *v = (*v).clamp(lo, hi));
                        if matches!(spec, Some(Spec::Range { sorted: true, .. })) {
                            kept.sort_unstable();
                        }
                    }
                    Node::Array(kept)
                }
                (Item::Str { len, .. }, Node::Str(chars)) => {
                    let len = usize::try_from(env.eval(len).ok()?).ok()?;
                    if chars.len() < len {
                        return None;
                    }
                    Node::Str(chars[..len].to_vec())
                }
                (Item::Repeat { count, body }, Node::Repeat(iterations)) => {
                    let count = usize::try_from(env.eval(count).ok()?).ok()?;
                    if iterations.len() < count {
                        return None;
                    }
                    let mut kept = Vec::with_capacity(count);
                    for nodes in &iterations[..count] {
                        env.push();
                        let result = walk(body, nodes, domains, env);
                        env.pop();
                        kept.push(result?);
                    }
                    Node::Repeat(kept)
                }
                _ => return None,
            });
        }
        Some(out)
    }
    walk(&schema.items, doc, domains, &mut Env::with_size(UNLIMITED_SIZE))
}

/// Whether the text reads back under the format with every constraint kept.
pub fn is_valid(schema: &Schema, domains: &Domains, text: &str) -> bool {
    let Ok(sample) = tokenize(text) else { return false };
    observe(schema, &[sample], Some(domains)).is_ok_and(|observed| observed.violations.is_empty())
}

// ---- the edits ------------------------------------------------------------------------------------------------

/// Where a node is: which repetition of which block, down from the top, and its index in that block.
#[derive(Clone, Debug)]
struct Loc {
    /// `(index of a repeat item, which repetition)`, from the top.
    frame: Vec<(usize, usize)>,
    item: usize,
}

#[derive(Clone, Debug)]
enum Edit {
    /// Take one repetition out of a block counted by a number, and lower that number to match.
    RemoveIteration { block: Loc, iteration: usize, count: Loc, slot: usize },
    Scalar { at: Loc, slot: usize, to: i64 },
    /// Set one element, or every element (`None`).
    Element { at: Loc, index: Option<usize>, to: i64 },
    Char { at: Loc, index: usize, to: char },
}

fn frame_mut<'a>(doc: &'a mut Doc, path: &[(usize, usize)]) -> &'a mut Doc {
    let mut frame = doc;
    for &(item, iteration) in path {
        let Node::Repeat(iterations) = &mut frame[item] else { unreachable!("a path only goes through repeat blocks") };
        frame = &mut iterations[iteration];
    }
    frame
}

fn apply(doc: &mut Doc, edit: &Edit) {
    match edit {
        Edit::RemoveIteration { block, iteration, count, slot } => {
            let remaining = {
                let Node::Repeat(iterations) = &mut frame_mut(doc, &block.frame)[block.item] else { unreachable!() };
                iterations.remove(*iteration);
                iterations.len()
            };
            let Node::Scalars(values) = &mut frame_mut(doc, &count.frame)[count.item] else { unreachable!() };
            values[*slot] = remaining as i64;
        }
        Edit::Scalar { at, slot, to } => {
            let Node::Scalars(values) = &mut frame_mut(doc, &at.frame)[at.item] else { unreachable!() };
            values[*slot] = *to;
        }
        Edit::Element { at, index, to } => {
            let Node::Array(values) = &mut frame_mut(doc, &at.frame)[at.item] else { unreachable!() };
            match index {
                Some(i) => values[*i] = *to,
                None => values.iter_mut().for_each(|v| *v = *to),
            }
        }
        Edit::Char { at, index, to } => {
            let Node::Str(chars) = &mut frame_mut(doc, &at.frame)[at.item] else { unreachable!() };
            chars[*index] = *to;
        }
    }
}

/// Simpler values to try in place of `value`, simplest first: zero (which normalizing pulls up to
/// the constraint's lower bound), half way to zero, and one step towards it.
fn simpler(value: i64) -> Vec<i64> {
    let mut targets = vec![0, value / 2, value - value.signum()];
    targets.retain(|&t| t != value);
    targets.dedup();
    let mut seen = HashSet::new();
    targets.retain(|t| seen.insert(*t));
    targets
}

/// Every single edit that could make the input simpler, the ones that remove the most first.
fn edits(schema: &Schema, domains: &Domains, doc: &Doc) -> Vec<Edit> {
    struct Collected {
        structural: Vec<Edit>,
        scalars: Vec<Edit>,
        arrays: Vec<Edit>,
        elements: Vec<Edit>,
        chars: Vec<Edit>,
    }

    fn collect(
        items: &[Item],
        nodes: &[Node],
        frame: &[(usize, usize)],
        scope: &mut Vec<(String, Loc, usize)>,
        domains: &Domains,
        out: &mut Collected,
    ) {
        let base = scope.len();
        for (index, (item, node)) in items.iter().zip(nodes).enumerate() {
            let at = Loc { frame: frame.to_vec(), item: index };
            match (item, node) {
                (Item::Scalars(names), Node::Scalars(values)) => {
                    for (slot, (name, &value)) in names.iter().zip(values).enumerate() {
                        out.scalars.extend(simpler(value).into_iter().map(|to| Edit::Scalar { at: at.clone(), slot, to }));
                        scope.push((name.clone(), at.clone(), slot));
                    }
                }
                (Item::Array { .. }, Node::Array(values)) => {
                    if values.iter().any(|&v| v != 0) {
                        out.arrays.push(Edit::Element { at: at.clone(), index: None, to: 0 });
                    }
                    for (i, &value) in values.iter().enumerate() {
                        out.elements.extend(simpler(value).into_iter().map(|to| Edit::Element { at: at.clone(), index: Some(i), to }));
                    }
                }
                (Item::Str { name, .. }, Node::Str(chars)) => {
                    if let Some(Spec::Chars(alphabet)) = domains.get(name)
                        && let Some(&simplest) = alphabet.first()
                    {
                        for (i, &c) in chars.iter().enumerate() {
                            if c != simplest {
                                out.chars.push(Edit::Char { at: at.clone(), index: i, to: simplest });
                            }
                        }
                    }
                }
                (Item::Repeat { count, body }, Node::Repeat(iterations)) => {
                    if let Expr::Var(var) = count
                        && let Some((_, count_at, slot)) = scope.iter().rev().find(|(name, _, _)| name == var)
                    {
                        for iteration in 0..iterations.len() {
                            out.structural.push(Edit::RemoveIteration {
                                block: at.clone(),
                                iteration,
                                count: count_at.clone(),
                                slot: *slot,
                            });
                        }
                    }
                    for (iteration, nodes) in iterations.iter().enumerate() {
                        let mut inner = frame.to_vec();
                        inner.push((index, iteration));
                        collect(body, nodes, &inner, scope, domains, out);
                    }
                }
                _ => {}
            }
        }
        scope.truncate(base);
    }

    let mut out = Collected { structural: vec![], scalars: vec![], arrays: vec![], elements: vec![], chars: vec![] };
    collect(&schema.items, doc, &[], &mut Vec::new(), domains, &mut out);
    [out.structural, out.scalars, out.arrays, out.elements, out.chars].concat()
}

/// One step simpler than `doc`: each result is a different, valid input, the most reduced first.
pub fn candidates(schema: &Schema, domains: &Domains, doc: &Doc) -> Vec<(Doc, String)> {
    let mut seen: HashSet<Doc> = HashSet::from([doc.clone()]);
    let mut out = Vec::new();
    for edit in edits(schema, domains, doc) {
        let mut edited = doc.clone();
        apply(&mut edited, &edit);
        let Some(edited) = normalize(schema, domains, &edited) else { continue };
        if !seen.insert(edited.clone()) {
            continue;
        }
        let text = render(schema, &edited);
        if is_valid(schema, domains, &text) {
            out.push((edited, text));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(lines: &[&str]) -> Schema {
        Schema::parse(lines).unwrap()
    }

    fn domains(pairs: &[(&str, &str)]) -> Domains {
        pairs.iter().map(|(n, t)| (n.to_string(), Spec::parse(t).unwrap())).collect()
    }

    fn doc_of(schema: &Schema, text: &str) -> Doc {
        parse(schema, &tokenize(text).unwrap()).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    fn texts(candidates: &[(Doc, String)]) -> Vec<String> {
        candidates.iter().map(|(_, t)| t.clone()).collect()
    }

    // ---- reading and writing ------------------------------------------------------------------

    #[test]
    fn an_input_is_read_and_written_back_unchanged() {
        let cases: [(&[&str], &str); 4] = [
            (&["n", "a[n]"], "3\n4 5 6\n"),
            (&["t", "repeat t {", "  n", "  a[n]", "}"], "2\n3\n1 2 3\n1\n9\n"),
            (&["n m", "repeat n {", "  str s[m]", "}"], "2 3\n.#.\n#.#\n"),
            (&["n m", "repeat m {", "  u v", "}"], "4 2\n1 2\n3 4\n"),
        ];
        for (lines, text) in cases {
            let s = schema(lines);
            assert_eq!(render(&s, &doc_of(&s, text)), text);
        }
    }

    #[test]
    fn an_input_that_does_not_fit_is_not_read() {
        let s = schema(&["n", "a[n]"]);
        assert!(parse(&s, &tokenize("3\n1 2\n").unwrap()).is_err());
        assert!(parse(&s, &tokenize("2\n1 2\n3\n").unwrap()).unwrap_err().contains("left over"));
    }

    #[test]
    fn an_empty_array_has_no_line() {
        let s = schema(&["n", "a[n]", "k"]);
        let doc = doc_of(&s, "0\n7\n");
        assert_eq!(doc[1], Node::Array(vec![]));
        assert_eq!(render(&s, &doc), "0\n7\n");
    }

    // ---- normalizing -----------------------------------------------------------------------------

    #[test]
    fn lowering_a_length_cuts_what_depends_on_it() {
        let s = schema(&["n", "a[n]", "str w[n]"]);
        let d = domains(&[("n", "1..size"), ("a", "1..9"), ("w", "chars ab")]);
        let mut doc = doc_of(&s, "4\n1 2 3 4\nabab\n");
        doc[0] = Node::Scalars(vec![2]);
        let normalized = normalize(&s, &d, &doc).unwrap();
        assert_eq!(render(&s, &normalized), "2\n1 2\nab\n");
    }

    #[test]
    fn values_are_pulled_back_into_their_constraints() {
        let s = schema(&["n", "a[n]"]);
        let d = domains(&[("n", "2..8"), ("a", "1..n")]);
        let mut doc = doc_of(&s, "5\n5 4 3 2 1\n");
        doc[0] = Node::Scalars(vec![0]);
        // n goes up to its lower bound 2, and the values are cut to 2 numbers no larger than 2.
        assert_eq!(render(&s, &normalize(&s, &d, &doc).unwrap()), "2\n2 2\n");
    }

    #[test]
    fn a_sorted_array_stays_sorted_after_clamping() {
        let s = schema(&["n", "a[n]"]);
        let d = domains(&[("n", "1..size"), ("a", "1..n, sorted")]);
        let mut doc = doc_of(&s, "4\n1 2 3 4\n");
        doc[0] = Node::Scalars(vec![2]);
        assert_eq!(render(&s, &normalize(&s, &d, &doc).unwrap()), "2\n1 2\n");
    }

    #[test]
    fn a_permutation_stays_a_permutation_when_it_is_shortened() {
        let s = schema(&["n", "p[n]"]);
        let d = domains(&[("n", "1..size"), ("p", "perm")]);
        let mut doc = doc_of(&s, "5\n3 5 1 4 2\n");
        doc[0] = Node::Scalars(vec![3]);
        assert_eq!(render(&s, &normalize(&s, &d, &doc).unwrap()), "3\n3 1 2\n");
    }

    #[test]
    fn a_length_that_would_have_to_grow_is_impossible() {
        let s = schema(&["n", "a[n]"]);
        let d = domains(&[("n", "1..size"), ("a", "1..9")]);
        let mut doc = doc_of(&s, "2\n1 2\n");
        doc[0] = Node::Scalars(vec![5]);
        assert_eq!(normalize(&s, &d, &doc), None);
    }

    // ---- the candidates ----------------------------------------------------------------------------------

    #[test]
    fn candidates_lower_lengths_and_values() {
        let s = schema(&["n", "a[n]"]);
        let d = domains(&[("n", "1..size"), ("a", "1..9")]);
        let found = texts(&candidates(&s, &d, &doc_of(&s, "3\n6 5 6\n")));
        // The length drops (to its lower bound, half way, one less) taking the array with it ...
        assert!(found.contains(&"1\n6\n".to_string()), "{found:?}");
        assert!(found.contains(&"2\n6 5\n".to_string()), "{found:?}");
        // ... and values go down, to the lower bound of 1 and half way.
        assert!(found.contains(&"3\n1 1 1\n".to_string()), "{found:?}");
        assert!(found.contains(&"3\n6 2 6\n".to_string()), "{found:?}");
        // The most aggressive come first: structure before values.
        assert_eq!(found[0], "1\n6\n");
    }

    #[test]
    fn candidates_are_valid_distinct_and_never_the_input_itself() {
        let s = schema(&["n", "a[n]"]);
        let d = domains(&[("n", "1..size"), ("a", "1..n, distinct")]);
        let doc = doc_of(&s, "3\n3 1 2\n");
        let found = candidates(&s, &d, &doc);
        let all = texts(&found);
        assert!(!all.contains(&render(&s, &doc)));
        assert_eq!(all.iter().collect::<HashSet<_>>().len(), all.len(), "no duplicates: {all:?}");
        assert!(all.iter().all(|t| is_valid(&s, &d, t)), "{all:?}");
        // Setting a value to 0 clamps it to 1, which repeats a value: not allowed with `distinct`.
        assert!(!all.contains(&"3\n1 1 2\n".to_string()), "{all:?}");
    }

    #[test]
    fn a_whole_block_can_be_dropped_and_its_count_follows() {
        let s = schema(&["t", "repeat t {", "  n", "  a[n]", "}"]);
        let d = domains(&[("t", "1..5"), ("n", "1..size"), ("a", "1..9")]);
        let found = texts(&candidates(&s, &d, &doc_of(&s, "3\n1\n5\n2\n6 7\n1\n8\n")));
        // Any one of the three tests can go, not only the last.
        assert!(found.contains(&"2\n2\n6 7\n1\n8\n".to_string()), "{found:?}");
        assert!(found.contains(&"2\n1\n5\n1\n8\n".to_string()), "{found:?}");
        assert!(found.contains(&"2\n1\n5\n2\n6 7\n".to_string()), "{found:?}");
    }

    #[test]
    fn the_last_block_is_not_dropped_if_the_count_must_stay_positive() {
        let s = schema(&["t", "repeat t {", "  n", "}"]);
        let d = domains(&[("t", "1..5"), ("n", "1..9")]);
        let found = texts(&candidates(&s, &d, &doc_of(&s, "1\n7\n")));
        assert!(!found.iter().any(|t| t.starts_with("0\n")), "{found:?}");
    }

    #[test]
    fn strings_become_the_simplest_character() {
        let s = schema(&["n", "str w[n]"]);
        let d = domains(&[("n", "1..size"), ("w", "chars abc")]);
        let found = texts(&candidates(&s, &d, &doc_of(&s, "3\ncab\n")));
        assert!(found.contains(&"3\naab\n".to_string()), "{found:?}");
        assert!(found.contains(&"3\ncaa\n".to_string()), "{found:?}");
    }

    #[test]
    fn a_fully_reduced_input_has_no_candidates() {
        let s = schema(&["n", "a[n]"]);
        let d = domains(&[("n", "1..size"), ("a", "1..9")]);
        assert!(candidates(&s, &d, &doc_of(&s, "1\n1\n")).is_empty());
    }

    #[test]
    fn candidates_keep_dependent_relations() {
        // Edges between vertices 1..n: lowering n must not leave a vertex above it.
        let s = schema(&["n m", "repeat m {", "  u v", "}"]);
        let d = domains(&[("n", "1..size"), ("m", "1..size"), ("u", "1..n"), ("v", "1..n")]);
        let doc = doc_of(&s, "5 3\n1 5\n2 4\n3 5\n");
        for (_, text) in candidates(&s, &d, &doc) {
            assert!(is_valid(&s, &d, &text), "{text}");
        }
        assert!(texts(&candidates(&s, &d, &doc)).contains(&"2 3\n1 2\n2 2\n2 2\n".to_string()));
    }
}

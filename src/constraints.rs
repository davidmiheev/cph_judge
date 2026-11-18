//! Setting the constraints of the generated input by asking for them (`--ask-constraints`), and
//! remembering the answers.
//!
//! The samples show what values *can* occur, never what the problem *allows*, so the inferred
//! constraints are guesses. This asks about each variable in turn, shows the current constraint,
//! and takes a new one in the same syntax as the `vars` hint. Answers are checked as they are
//! typed, and a wrong one is explained and asked again.
//!
//! Input and output are passed in, so the dialogue is tested with scripted answers. A closed input
//! (end of file) keeps whatever is left as it is.

use std::{
    fs,
    io::{self, BufRead, Write},
    path::Path,
};

use serde_json::{Map, Value};

use crate::graph::count_range;
use crate::schema::{Domains, EdgeBlock, Expr, SIZE, Schema, Spec, VarKind, Variable, validate_spec};

pub const SYNTAX: &str = "\
    lo..hi            an integer from lo to hi; bounds may use numbers defined earlier and `size`
                      (n, n-1, 2*n, 10^9, 1e5)
    lo..hi, distinct  an array whose elements are all different (also: sorted)
    perm              an array that is a permutation of 1..its length
    chars a-z         a string over these characters (chars 01, chars .#)";

pub const EDGE_SYNTAX: &str = "\
    tree              the edges of a tree: one fewer than the vertices, all connected
    graph             a simple graph: no loops, no repeated edges
    graph, connected  ... that is also connected      (or `, directed`: an edge and its reverse differ)
    ..., ordered      every edge lists its smaller vertex first
    none              no constraint on the pairs";

/// What the dialogue ended with.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Whether any constraint was changed.
    pub changed: bool,
    /// Whether the input ended before every variable was asked about.
    pub ended: bool,
}

fn kind_name(kind: VarKind) -> &'static str {
    match kind {
        VarKind::Scalar => "number",
        VarKind::Array => "array",
        VarKind::Str => "string",
    }
}

/// Asks about each variable in turn, updating `domains` with the answers.
pub fn ask<R: BufRead, W: Write>(schema: &Schema, domains: &mut Domains, input: &mut R, out: &mut W) -> io::Result<Outcome> {
    writeln!(out, "Constraints of the generated input (Enter keeps the value in brackets, `?` shows the syntax):")?;
    let mut changed = false;

    for variable in schema.variables() {
        loop {
            let current = domains.get(&variable.name).map(Spec::to_string).unwrap_or_default();
            write!(out, "  {} ({}) [{}]: ", variable.name, kind_name(variable.kind), current)?;
            out.flush()?;

            let mut line = String::new();
            if input.read_line(&mut line)? == 0 {
                writeln!(out)?;
                return Ok(Outcome { changed, ended: true });
            }
            let answer = line.trim();
            if answer.is_empty() {
                break;
            }
            if answer == "?" {
                writeln!(out, "{SYNTAX}")?;
                writeln!(out, "    here you may use: {}", usable(&variable))?;
                continue;
            }
            match Spec::parse(answer).and_then(|spec| validate_spec(&variable, &spec).map(|()| spec)) {
                Ok(spec) => {
                    if domains.get(&variable.name) != Some(&spec) {
                        changed = true;
                        domains.insert(variable.name.clone(), spec);
                    }
                    break;
                }
                Err(message) => writeln!(out, "    ✗ {message}")?,
            }
        }
    }

    let outcome = ask_relations(schema, domains, input, out)?;
    Ok(Outcome { changed: changed || outcome.changed, ended: outcome.ended })
}

/// The questions that are about pairs of columns rather than one variable: whether a list of edges is a
/// tree or a graph, and whether the second of two columns is never below the first.
fn ask_relations<R: BufRead, W: Write>(schema: &Schema, domains: &mut Domains, input: &mut R, out: &mut W) -> io::Result<Outcome> {
    let blocks = schema.edge_blocks();
    let rows = schema.row_lines();
    if blocks.is_empty() && rows.is_empty() {
        return Ok(Outcome { changed: false, ended: false });
    }
    writeln!(out, "Relations between columns (Enter keeps the value in brackets, `?` shows the syntax):")?;
    let mut changed = false;

    for block in &blocks {
        let key = block.key();
        loop {
            let current = domains.get(&key).map(Spec::to_string).unwrap_or_else(|| "none".to_string());
            write!(out, "  edges {key} [{current}]: ")?;
            out.flush()?;
            let mut line = String::new();
            if input.read_line(&mut line)? == 0 {
                writeln!(out)?;
                return Ok(Outcome { changed, ended: true });
            }
            let answer = line.trim();
            if answer.is_empty() {
                break;
            }
            if answer == "?" {
                writeln!(out, "{EDGE_SYNTAX}")?;
                continue;
            }
            if answer == "none" {
                changed |= domains.remove(&key).is_some();
                break;
            }
            match Spec::parse(answer) {
                Ok(Spec::Edges(edges)) => {
                    let spec = Spec::Edges(edges);
                    if domains.get(&key) != Some(&spec) {
                        changed = true;
                        domains.insert(key.clone(), spec);
                        if let Some(message) = fit_edge_count(block, &edges, domains) {
                            writeln!(out, "    → {message}")?;
                        }
                    }
                    break;
                }
                Ok(_) => writeln!(out, "    ✗ expected `tree`, `graph` (with flags) or `none`")?,
                Err(message) => writeln!(out, "    ✗ {message}")?,
            }
        }
    }

    for names in &rows {
        for pair in names.windows(2) {
            let (first, second) = (&pair[0], &pair[1]);
            // The two columns of a list of edges with a constraint have `ordered` for this.
            if names.len() == 2 && domains.contains_key(&format!("{first} {second}")) {
                continue;
            }
            // "No" goes back to where the row starts: the lower bound of its first column, which no
            // answer here changes (an earlier answer may have tied the one before this to it).
            let (Some(Spec::Range { lo: base_lo, .. }), Some(Spec::Range { lo, hi, distinct, sorted })) =
                (domains.get(&names[0]).cloned(), domains.get(second).cloned())
            else {
                continue;
            };
            let ordered = lo == Expr::var(first);
            loop {
                write!(out, "  {first} ≤ {second} in every row? [{}]: ", if ordered { "y" } else { "n" })?;
                out.flush()?;
                let mut line = String::new();
                if input.read_line(&mut line)? == 0 {
                    writeln!(out)?;
                    return Ok(Outcome { changed, ended: true });
                }
                let new_lo = match line.trim().to_ascii_lowercase().as_str() {
                    "" => break,
                    "y" | "yes" => Expr::var(first),
                    "n" | "no" => base_lo.clone(),
                    _ => {
                        writeln!(out, "    ✗ answer y or n")?;
                        continue;
                    }
                };
                if new_lo != lo {
                    changed = true;
                    domains.insert(second.clone(), Spec::Range { lo: new_lo, hi, distinct, sorted });
                }
                break;
            }
        }
    }
    Ok(Outcome { changed, ended: false })
}

/// Sets the number of edges to what a list of this kind has (`n-1` for a tree, up to `n*(n-1)/2` for a
/// graph), when it is a number of its own. Says what it did, or why it could not.
fn fit_edge_count(block: &EdgeBlock, edges: &crate::schema::EdgeSpec, domains: &mut Domains) -> Option<String> {
    let Some(Spec::Range { hi: Expr::Var(vertices), .. }) = domains.get(&block.columns[0]).cloned() else {
        return Some(format!(
            "the vertices of `{}` are not bounded by one number, so set the number of edges yourself",
            block.columns[0]
        ));
    };
    match &block.count {
        Expr::Var(count) if matches!(domains.get(count), Some(Spec::Range { .. })) => {
            let range = count_range(edges, &vertices);
            domains.insert(count.clone(), Spec::parse(&range).ok()?);
            Some(format!("`{count}` is now {range}"))
        }
        // `n-1` rows and the like: the count is already a formula.
        _ => None,
    }
}

/// The names a constraint of this variable may use.
fn usable(variable: &Variable) -> String {
    let mut names = vec![SIZE.to_string()];
    names.extend(variable.visible.iter().cloned());
    names.join(", ")
}

/// Asks a yes/no question; anything but yes, and a closed input, is no.
pub fn confirm<R: BufRead, W: Write>(question: &str, input: &mut R, out: &mut W) -> io::Result<bool> {
    write!(out, "{question} [y/N] ")?;
    out.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        writeln!(out)?;
        return Ok(false);
    }
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// Writes the constraints (and the format, if given) into a hints file, keeping everything else
/// in it. The constraints are merged by variable name; the file is replaced atomically.
pub fn save(path: &Path, format: Option<&[String]>, domains: &Domains) -> Result<(), String> {
    let mut object = match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => map,
            Ok(_) => return Err(format!("{} is not a JSON object", path.display())),
            Err(e) => return Err(format!("{} is not valid JSON: {e}", path.display())),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };

    if let Some(lines) = format {
        object.insert("format".to_string(), Value::Array(lines.iter().cloned().map(Value::String).collect()));
    }
    let mut vars = match object.remove("vars") {
        Some(Value::Object(existing)) => existing,
        _ => Map::new(),
    };
    for (name, spec) in domains {
        vars.insert(name.clone(), Value::String(spec.to_string()));
    }
    object.insert("vars".to_string(), Value::Object(vars));

    let mut text = serde_json::to_string_pretty(&Value::Object(object)).map_err(|e| e.to_string())?;
    text.push('\n');
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, text).map_err(|e| format!("could not write {}: {e}", temp.display()))?;
    fs::rename(&temp, path).map_err(|e| format!("could not replace {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn schema() -> Schema {
        let lines = ["n", "a[n]", "str s[n]"];
        Schema::parse(&lines).unwrap()
    }

    fn defaults() -> Domains {
        [("n", "1..size"), ("a", "1..10"), ("s", "chars a-c")]
            .into_iter()
            .map(|(name, text)| (name.to_string(), Spec::parse(text).unwrap()))
            .collect()
    }

    /// Runs the dialogue with these lines typed; returns the outcome, the constraints and what was printed.
    fn dialogue(typed: &str) -> (Outcome, Domains, String) {
        let mut domains = defaults();
        let mut out = Vec::new();
        let outcome = ask(&schema(), &mut domains, &mut Cursor::new(typed.as_bytes()), &mut out).unwrap();
        (outcome, domains, String::from_utf8(out).unwrap())
    }

    fn show(domains: &Domains) -> Vec<String> {
        domains.iter().map(|(name, spec)| format!("{name}: {spec}")).collect()
    }

    #[test]
    fn pressing_enter_keeps_every_constraint() {
        let (outcome, domains, out) = dialogue("\n\n\n");
        assert_eq!(outcome, Outcome { changed: false, ended: false });
        assert_eq!(domains, defaults());
        // Each prompt names the variable, what it is, and the current constraint.
        assert!(out.contains("n (number) [1..size]: "), "{out}");
        assert!(out.contains("a (array) [1..10]: "), "{out}");
        assert!(out.contains("s (string) [chars a-c]: "), "{out}");
    }

    #[test]
    fn answers_replace_constraints() {
        let (outcome, domains, _) = dialogue("1..8\n-5..5, sorted\nchars xy\n");
        assert_eq!(outcome, Outcome { changed: true, ended: false });
        assert_eq!(show(&domains), ["a: -5..5, sorted", "n: 1..8", "s: chars xy"]);
    }

    #[test]
    fn some_can_be_kept_while_others_change() {
        let (_, domains, _) = dialogue("\n1..n\n\n");
        assert_eq!(show(&domains), ["a: 1..n", "n: 1..size", "s: chars a-c"]);
    }

    #[test]
    fn typing_the_value_already_there_is_not_a_change() {
        let (outcome, _, _) = dialogue("1..size\n\n\n");
        assert!(!outcome.changed);
    }

    #[test]
    fn a_wrong_answer_is_explained_and_asked_again() {
        // Three wrong tries for `n`, then Enter for `n`, `a` and `s`.
        let (outcome, domains, out) = dialogue("oops\n1..m\n1..5, distinct\n\n\n\n");
        assert!(out.contains("✗ expected `lo..hi`"), "{out}");
        assert!(out.contains("✗ the constraint of `n` uses `m`, which is not a number defined before it"), "{out}");
        assert!(out.contains("✗ `n` is a single number: `distinct` and `sorted` are for arrays"), "{out}");
        // Nothing wrong was accepted.
        assert_eq!(outcome, Outcome { changed: false, ended: false });
        assert_eq!(domains, defaults());
    }

    #[test]
    fn a_constraint_of_the_wrong_kind_is_refused() {
        // `chars` for the number `n`, then Enter; Enter for `a`; `perm` for the string `s`, then Enter.
        let (outcome, domains, out) = dialogue("chars ab\n\n\nperm\n\n");
        assert!(out.contains("✗ `n` holds numbers; `chars ...` is for strings"), "{out}");
        assert!(out.contains("✗ `s` is a string"), "{out}");
        assert_eq!((outcome.changed, outcome.ended, domains), (false, false, defaults()));
    }

    #[test]
    fn the_question_mark_shows_the_syntax_and_what_may_be_used() {
        let (_, domains, out) = dialogue("\n?\n1..n\n\n");
        assert!(out.contains("lo..hi"), "{out}");
        assert!(out.contains("here you may use: size, n"), "{out}");
        assert_eq!(domains["a"].to_string(), "1..n", "and it asks again afterwards");
    }

    #[test]
    fn a_closed_input_keeps_what_is_left() {
        let (outcome, domains, _) = dialogue("1..4\n");
        assert_eq!(outcome, Outcome { changed: true, ended: true });
        assert_eq!(show(&domains), ["a: 1..10", "n: 1..4", "s: chars a-c"]);
        let (outcome, domains, _) = dialogue("");
        assert_eq!(outcome, Outcome { changed: false, ended: true });
        assert_eq!(domains, defaults());
    }

    #[test]
    fn windows_line_endings_are_fine() {
        let (_, domains, _) = dialogue("1..8\r\n\r\n\r\n");
        assert_eq!(domains["n"].to_string(), "1..8");
    }

    #[test]
    fn confirm_means_yes_only_for_yes() {
        let ask = |typed: &str| confirm("Save?", &mut Cursor::new(typed.as_bytes()), &mut Vec::new()).unwrap();
        assert!(ask("y\n") && ask("Y\n") && ask("yes\n") && ask(" YES \n"));
        assert!(!ask("n\n") && !ask("\n") && !ask("maybe\n") && !ask(""));
    }

    // ---- relations: edges and order ------------------------------------------------------------

    fn edges_schema() -> Schema {
        Schema::parse(&["n m", "repeat m {", "  u v", "}"]).unwrap()
    }

    fn edges_defaults() -> Domains {
        [("n", "1..size"), ("m", "1..size"), ("u", "1..n"), ("v", "1..n")]
            .into_iter()
            .map(|(name, text)| (name.to_string(), Spec::parse(text).unwrap()))
            .collect()
    }

    /// Four Enters for the variables `n`, `m`, `u`, `v`, then `typed` for the questions about relations.
    fn relations(schema: &Schema, defaults: Domains, typed: &str) -> (Outcome, Domains, String) {
        let mut domains = defaults;
        let mut out = Vec::new();
        let outcome = ask(schema, &mut domains, &mut Cursor::new(typed.as_bytes()), &mut out).unwrap();
        (outcome, domains, String::from_utf8(out).unwrap())
    }

    fn graph_dialogue(relation_answers: &str) -> (Outcome, Domains, String) {
        relations(&edges_schema(), edges_defaults(), &format!("\n\n\n\n{relation_answers}"))
    }

    #[test]
    fn a_list_of_edges_is_asked_about_after_the_variables() {
        let (outcome, domains, out) = graph_dialogue("\n\n");
        assert!(out.contains("Relations between columns"), "{out}");
        assert!(out.contains("edges u v [none]: "), "{out}");
        assert!(out.contains("u ≤ v in every row? [n]: "), "{out}");
        assert_eq!(outcome, Outcome { changed: false, ended: false });
        assert_eq!(domains, edges_defaults());
    }

    #[test]
    fn answering_tree_makes_the_number_of_edges_n_minus_one_and_says_so() {
        let (outcome, domains, out) = graph_dialogue("tree\n\n");
        assert_eq!(domains["u v"].to_string(), "tree");
        assert_eq!(domains["m"].to_string(), "n-1..n-1");
        assert!(out.contains("→ `m` is now n-1..n-1"), "{out}");
        assert!(outcome.changed);
        // The two columns of a list with a constraint have `ordered` for order, so no second question about them.
        assert!(!out.contains("≤"), "{out}");
    }

    #[test]
    fn answering_a_graph_sets_the_number_of_edges_to_what_fits() {
        let (_, domains, _) = graph_dialogue("graph\n\n");
        assert_eq!((domains["u v"].to_string(), domains["m"].to_string()), ("graph".to_string(), "0..n*(n-1)/2".to_string()));
        let (_, domains, _) = graph_dialogue("graph, connected, ordered\n");
        assert_eq!((domains["u v"].to_string(), domains["m"].to_string()), ("graph, connected, ordered".to_string(), "n-1..n*(n-1)/2".to_string()));
        let (_, domains, _) = graph_dialogue("graph, directed\n");
        assert_eq!(domains["m"].to_string(), "0..n*(n-1)");
    }

    #[test]
    fn none_removes_a_constraint_on_edges() {
        let mut defaults = edges_defaults();
        defaults.insert("u v".to_string(), Spec::parse("tree").unwrap());
        let (outcome, domains, out) = relations(&edges_schema(), defaults, "\n\n\n\nnone\n\n");
        assert!(out.contains("edges u v [tree]: "), "{out}");
        assert!(!domains.contains_key("u v") && outcome.changed);
    }

    #[test]
    fn a_wrong_answer_about_edges_is_explained_and_asked_again() {
        let (_, domains, out) = graph_dialogue("forest\n1..5\ntree, directed\ntree\n\n");
        assert!(out.contains("✗ expected `lo..hi`") || out.contains("✗ expected `tree`"), "{out}");
        assert!(out.contains("✗ expected `tree`, `graph` (with flags) or `none`"), "{out}");
        assert!(out.contains("✗ a tree cannot be `directed`"), "{out}");
        assert_eq!(domains["u v"].to_string(), "tree");
    }

    #[test]
    fn the_question_mark_explains_the_kinds_of_edges() {
        let (_, _, out) = graph_dialogue("?\ntree\n");
        assert!(out.contains("a simple graph: no loops, no repeated edges"), "{out}");
    }

    #[test]
    fn the_number_of_edges_is_left_alone_when_it_is_already_a_formula() {
        let schema = Schema::parse(&["n", "repeat n-1 {", "  u v", "}"]).unwrap();
        let defaults: Domains = [("n", "1..size"), ("u", "1..n"), ("v", "1..n")].into_iter().map(|(a, b)| (a.to_string(), Spec::parse(b).unwrap())).collect();
        let (_, domains, out) = relations(&schema, defaults, "\n\n\ntree\n");
        assert_eq!(domains["u v"].to_string(), "tree");
        assert!(!out.contains("→"), "nothing needed changing: {out}");
    }

    #[test]
    fn the_number_of_edges_cannot_be_fitted_without_a_number_of_vertices() {
        let defaults: Domains = [("n", "1..size"), ("m", "1..size"), ("u", "1..10"), ("v", "1..10")]
            .into_iter()
            .map(|(a, b)| (a.to_string(), Spec::parse(b).unwrap()))
            .collect();
        let (_, domains, out) = relations(&edges_schema(), defaults, "\n\n\n\ntree\n");
        assert!(out.contains("not bounded by one number"), "{out}");
        assert_eq!(domains["m"].to_string(), "1..size");
    }

    #[test]
    fn two_columns_can_be_asked_to_be_in_order() {
        // Not edges (three columns), just rows: a, b, c.
        let schema = Schema::parse(&["n", "repeat n {", "  a b c", "}"]).unwrap();
        let defaults: Domains = [("n", "1..size"), ("a", "1..9"), ("b", "1..9"), ("c", "1..9")]
            .into_iter()
            .map(|(x, y)| (x.to_string(), Spec::parse(y).unwrap()))
            .collect();
        let (outcome, domains, out) = relations(&schema, defaults, "\n\n\n\ny\nn\n");
        assert!(out.contains("a ≤ b in every row? [n]: ") && out.contains("b ≤ c in every row? [n]: "), "{out}");
        assert_eq!(domains["b"].to_string(), "a..9");
        assert_eq!(domains["c"].to_string(), "1..9", "answered no");
        assert!(outcome.changed);
    }

    #[test]
    fn an_order_can_be_taken_back() {
        let schema = Schema::parse(&["n", "repeat n {", "  a b c", "}"]).unwrap();
        let defaults: Domains = [("n", "1..size"), ("a", "2..9"), ("b", "a..9"), ("c", "1..9")]
            .into_iter()
            .map(|(x, y)| (x.to_string(), Spec::parse(y).unwrap()))
            .collect();
        let (_, domains, out) = relations(&schema, defaults, "\n\n\n\nn\n\n");
        assert!(out.contains("a ≤ b in every row? [y]: "), "{out}");
        assert_eq!(domains["b"].to_string(), "2..9", "back to where `a` starts");
    }

    #[test]
    fn a_wrong_order_answer_is_asked_again_and_a_closed_input_ends_the_dialogue() {
        let schema = Schema::parse(&["n", "repeat n {", "  a b c", "}"]).unwrap();
        let defaults: Domains = [("n", "1..size"), ("a", "1..9"), ("b", "1..9"), ("c", "1..9")]
            .into_iter()
            .map(|(x, y)| (x.to_string(), Spec::parse(y).unwrap()))
            .collect();
        let (outcome, _, out) = relations(&schema, defaults, "\n\n\n\nmaybe\ny\n");
        assert!(out.contains("✗ answer y or n"), "{out}");
        assert!(outcome.ended, "the input ran out at the second order question");
    }

    #[test]
    fn a_format_without_pairs_asks_nothing_about_relations() {
        let (_, _, out) = dialogue("\n\n\n");
        assert!(!out.contains("Relations"), "{out}");
    }

    // ---- saving --------------------------------------------------------------------------------

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cph_judge_constraints_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn saving_creates_the_file_with_the_format_and_the_constraints() {
        let dir = scratch("create");
        let path = dir.join("A.stress.json");
        let format = vec!["n".to_string(), "a[n]".to_string()];
        save(&path, Some(&format), &defaults()).unwrap();
        let saved = read(&path);
        assert_eq!(saved["format"], serde_json::json!(["n", "a[n]"]));
        assert_eq!(saved["vars"]["n"], "1..size");
        assert_eq!(saved["vars"]["s"], "chars a-c");
        assert!(!dir.join("A.stress.json.tmp").exists() && !dir.join("A.stress.json.json.tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_keeps_the_other_settings_and_merges_the_constraints() {
        let dir = scratch("merge");
        let path = dir.join("A.stress.json");
        fs::write(&path, r#"{ "iters": 50, "brute": "slow.rs", "vars": { "zzz": "1..2", "n": "1..99" } }"#).unwrap();
        save(&path, None, &defaults()).unwrap();
        let saved = read(&path);
        assert_eq!(saved["iters"], 50);
        assert_eq!(saved["brute"], "slow.rs");
        assert_eq!(saved["vars"]["n"], "1..size", "the new answer replaces the old one");
        assert_eq!(saved["vars"]["zzz"], "1..2", "constraints of other names are left alone");
        assert!(saved.get("format").is_none(), "no format was given, so none is added");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_a_format_replaces_the_old_one() {
        let dir = scratch("format");
        let path = dir.join("A.stress.json");
        fs::write(&path, r#"{ "format": ["old"] }"#).unwrap();
        save(&path, Some(&["n".to_string()]), &Domains::new()).unwrap();
        assert_eq!(read(&path)["format"], serde_json::json!(["n"]));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saved_constraints_are_read_back_by_the_hints() {
        let dir = scratch("roundtrip");
        let src = dir.join("A.rs");
        let mut domains = defaults();
        domains.insert("a".to_string(), Spec::parse("-5..5, distinct, sorted").unwrap());
        save(&crate::hints::sidecar_path(&src), Some(&["n".to_string(), "a[n]".to_string(), "str s[n]".to_string()]), &domains).unwrap();

        let hints = crate::hints::load(None, &src).unwrap();
        assert_eq!(hints.format, Some(vec!["n".to_string(), "a[n]".to_string(), "str s[n]".to_string()]));
        assert_eq!(hints.vars["a"], "-5..5, distinct, sorted");
        let built = crate::infer::build(&[], &hints).unwrap();
        assert_eq!(built.domains, domains, "the whole generator comes back from the file alone");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_a_json_object_is_not_overwritten() {
        let dir = scratch("bad");
        let path = dir.join("A.stress.json");
        for content in ["[1, 2]", "not json"] {
            fs::write(&path, content).unwrap();
            assert!(save(&path, None, &defaults()).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), content);
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}

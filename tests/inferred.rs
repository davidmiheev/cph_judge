//! End-to-end tests for stress testing without a generator file: the tests are made from the input
//! format inferred from the samples in the `.prob`, with constraints that can be set by hints or typed
//! at the `--ask-constraints` prompt. Each test builds a throwaway Cargo project and runs the real binary.
//!
//! The example is "sum of an array" (see `common`): the samples are `n` and then `n` numbers, and the
//! buggy solution is wrong exactly when `n >= 3`.

mod common;

use common::*;
use std::fs;

const SAMPLES: [(&str, &str); 2] = [("3\n1 2 3\n", "6\n"), ("2\n5 7\n", "12\n")];

/// A sum project with the buggy solution and a brute force, but no generator file.
fn project(name: &str) -> Project {
    let p = Project::new("inferred", name);
    p.solution(&sum_buggy()).brute(&sum_correct()).problem(&SAMPLES);
    p
}

fn counterexample_input(p: &Project) -> String {
    fs::read_to_string(p.root.join("target/judge/stress/counterexample.in")).expect("a counterexample was found")
}

// ---- stress testing from the samples ------------------------------------------------

#[test]
fn without_a_generator_file_the_tests_are_made_from_the_samples() {
    let p = project("from_samples");
    let r = p.run_with(&["--stress", "--seed", "1"]);
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    // The inferred format and its constraints are shown, so a wrong guess is visible.
    assert!(r.stdout.contains("Input format (inferred from 2 sample(s))"), "{}", r.stdout);
    assert!(r.stdout.contains("  n\n  a[n]\n  constraints:\n    n: 1..size\n    a: 1..10"), "{}", r.stdout);
    assert!(r.stdout.contains("The constraints are guesses from the samples"), "{}", r.stdout);
    // And the bug is found and shrunk like with a generator file: three numbers are needed to fail.
    assert!(r.stdout.contains("Counterexample found"), "{}", r.stdout);
    assert!(r.stdout.contains("size 3"), "{}", r.stdout);
    assert!(r.stdout.contains("The input was made by the inferred generator"), "{}", r.stdout);
    assert_eq!(counterexample_input(&p).lines().next(), Some("3"), "n = 3 is the smallest failing input");
}

#[test]
fn a_correct_solution_survives_stress_testing_with_the_inferred_generator() {
    let p = project("correct");
    p.solution(&sum_correct());
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "300"]);
    // Every generated input was valid for the brute force and the solution (both parse it with unwrap).
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("No counterexample in 300 tests"), "{}", r.stdout);
}

#[test]
fn the_same_seed_gives_the_same_counterexample() {
    let p = project("repeatable");
    let first = p.run_with(&["--stress", "--seed", "5"]);
    let second = p.run_with(&["--stress", "--seed", "5"]);
    assert_eq!(first.stdout, second.stdout);
}

#[test]
fn a_generator_file_is_used_in_preference_to_the_samples() {
    let p = project("gen_file_wins");
    p.generator("fn main() { println!(\"3\\n10 20 30\"); }\n");
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "5"]);
    assert!(!r.stdout.contains("Input format"), "{}", r.stdout);
    assert_eq!(counterexample_input(&p), "3\n10 20 30\n");
}

#[test]
fn samples_that_cannot_be_understood_are_explained_and_the_generator_file_is_suggested() {
    let p = project("floats");
    p.problem(&[("2\n1.5 2.5\n", "4\n")]);
    let r = p.run_with(&["--stress"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("could not make tests from the samples"), "{}", r.stderr);
    assert!(r.stderr.contains("non-integer"), "{}", r.stderr);
    assert!(r.stderr.contains("A_gen.rs"), "{}", r.stderr);
}

// ---- reducing the counterexample ----------------------------------------------------------

#[test]
fn the_counterexample_is_reduced_to_the_simplest_input_that_still_fails() {
    let p = project("reduced");
    let r = p.run_with(&["--stress", "--seed", "1"]);
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    // The solution is wrong from three numbers on, and any numbers show it: the least is three ones.
    assert_eq!(counterexample_input(&p), "3\n1 1 1\n", "{}", r.stdout);
    assert!(r.stdout.contains("then reduced in"), "{}", r.stdout);
    assert!(r.stdout.contains("--- Input ---\n3\n1 1 1\n"), "{}", r.stdout);
}

#[test]
fn reduction_stays_inside_the_constraints() {
    let p = project("reduced_constrained");
    p.problem_with_hints(&SAMPLES, r#"{"vars":{"a":"5..9"}}"#);
    p.run_with(&["--stress", "--seed", "1"]);
    // Not 1 1 1: the numbers must be 5 to 9.
    assert_eq!(counterexample_input(&p), "3\n5 5 5\n");
}

#[test]
fn the_reduced_input_is_the_one_that_is_saved() {
    let p = project("reduced_saved");
    let r = p.run_with(&["--stress", "--seed", "1", "--save"]);
    assert!(r.stdout.contains("Saved as test #3"), "{}", r.stdout);
    assert!(p.prob_json().contains(r#""input":"3\n1 1 1\n""#), "{}", p.prob_json());
}

#[test]
fn reduction_is_repeatable() {
    let p = project("reduced_repeatable");
    let first = p.run_with(&["--stress", "--seed", "7"]);
    let second = p.run_with(&["--stress", "--seed", "7"]);
    assert_eq!(first.stdout, second.stdout);
}

#[test]
fn a_generator_files_input_is_left_exactly_as_it_made_it() {
    let p = project("not_reduced");
    p.generator("fn main() { println!(\"3\\n10 20 30\"); }\n");
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "5"]);
    assert!(!r.stdout.contains("then reduced in"), "{}", r.stdout);
    assert_eq!(counterexample_input(&p), "3\n10 20 30\n");
}

// ---- trees, graphs and l <= r ------------------------------------------------------------------

/// "Print the largest degree of a vertex". Reads `n` (and `m` if `with_m`, else there are `n-1` edges) and the edges.
fn degree_program(with_m: bool, print: &str) -> String {
    let read_m = if with_m { "let m = it.next().unwrap();" } else { "let m = n - 1;" };
    format!(
        r#"
use std::io::Read;
fn main() {{
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    let mut it = s.split_whitespace().map(|t| t.parse::<usize>().unwrap());
    let n = it.next().unwrap();
    {read_m}
    let mut d = vec![0usize; n + 1];
    for _ in 0..m {{
        let u = it.next().unwrap();
        let v = it.next().unwrap();
        d[u] += 1;
        d[v] += 1;
    }}
    println!("{{}}", {print});
}}
"#
    )
}

/// The buggy solution only looks at vertex 1.
fn tree_project(name: &str, with_m: bool, samples: &[(&str, &str)]) -> Project {
    let p = Project::new("inferred", name);
    p.solution(&degree_program(with_m, "d[1]")).brute(&degree_program(with_m, "d.iter().max().unwrap()")).problem(samples);
    p
}

const TREE_SAMPLES: [(&str, &str); 2] = [("4\n1 2\n2 3\n3 4\n", "2\n"), ("3\n1 2\n1 3\n", "2\n")];

/// The degree of every vertex in a counterexample that starts with `n` and then lists `n-1` edges.
fn degrees(input: &str) -> Vec<usize> {
    let mut lines = input.lines();
    let n: usize = lines.next().unwrap().split_whitespace().next().unwrap().parse().unwrap();
    let mut d = vec![0; n + 1];
    for line in lines {
        for v in line.split_whitespace() {
            d[v.parse::<usize>().unwrap()] += 1;
        }
    }
    d
}

#[test]
fn a_tree_is_inferred_generated_and_reduced_to_a_small_tree() {
    let p = tree_project("tree", false, &TREE_SAMPLES);
    let r = p.run_with(&["--stress", "--seed", "1"]);
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("repeat n-1 {") && r.stdout.contains("u v: tree, ordered"), "{}", r.stdout);

    // Every generated test was a tree (the brute force would have crashed on a bad vertex), and the
    // reduction left the smallest tree that fools the solution: three vertices, vertex 1 not the centre.
    let input = counterexample_input(&p);
    assert_eq!(input.lines().next(), Some("3"), "{input}");
    assert_eq!(input.lines().count(), 3, "two edges: {input}");
    let d = degrees(&input);
    assert!(d[1] < *d.iter().max().unwrap(), "vertex 1 must not be the busiest: {input}");
    assert!(r.stdout.contains("then reduced in"), "{}", r.stdout);
}

#[test]
fn a_correct_solution_survives_stress_testing_on_trees() {
    let p = tree_project("tree_ok", false, &TREE_SAMPLES);
    p.solution(&degree_program(false, "d.iter().max().unwrap()"));
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "300"]);
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
}

#[test]
fn show_schema_prints_the_tree_constraint_as_a_hint() {
    let p = tree_project("tree_schema", false, &TREE_SAMPLES);
    let r = p.run_with(&["--stress", "--show-schema"]);
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains(r#""u v": "tree, ordered""#), "{}", r.stdout);
    assert!(r.stdout.contains(r#""repeat n-1 {""#), "{}", r.stdout);
}

#[test]
fn a_hint_can_ask_for_a_tree_without_the_samples_saying_so() {
    // Forests in the samples; the hint says the edges form a tree.
    let p = tree_project("tree_hint", true, &[("5 3\n1 2\n2 3\n4 5\n", "2\n"), ("4 2\n1 2\n3 4\n", "1\n")]);
    p.problem_with_hints(
        &[("5 3\n1 2\n2 3\n4 5\n", "2\n"), ("4 2\n1 2\n3 4\n", "1\n")],
        r#"{"vars":{"u v":"tree","m":"n-1..n-1"}}"#,
    );
    let r = p.run_with(&["--stress", "--seed", "1"]);
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    let first = counterexample_input(&p);
    let header: Vec<usize> = first.lines().next().unwrap().split(' ').map(|v| v.parse().unwrap()).collect();
    assert_eq!(header[1], header[0] - 1, "a tree: {first}");
    assert!(r.stdout.contains("do not connect") || r.stdout.contains("a tree on"), "the forest samples are warned about: {}", r.stdout);
}

#[test]
fn the_prompt_asks_about_edges_and_sets_the_edge_count_to_match() {
    let p = tree_project("tree_prompt", true, &[("5 3\n1 2\n2 3\n4 5\n", "2\n"), ("4 2\n1 2\n3 4\n", "1\n")]);
    // Enter for n, m, u, v; "tree" for the edges; yes, save.
    let r = p.run_typing(&["--stress", "--seed", "1", "--ask-constraints"], "\n\n\n\ntree\ny\n");
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("edges u v [graph, ordered]: "), "{}", r.stdout);
    assert!(r.stdout.contains("→ `m` is now n-1..n-1"), "{}", r.stdout);
    let saved = fs::read_to_string(p.root.join("sol/A.stress.json")).unwrap();
    assert!(saved.contains(r#""u v": "tree""#) && saved.contains(r#""m": "n-1..n-1""#), "{saved}");

    let input = counterexample_input(&p);
    let header: Vec<usize> = input.lines().next().unwrap().split(' ').map(|v| v.parse().unwrap()).collect();
    assert_eq!(header[1], header[0] - 1, "the tests are trees: {input}");

    // The saved answers are all a later run needs.
    let later = p.run_with(&["--stress", "--seed", "1"]);
    assert!(later.stdout.contains("u v: tree") && later.stdout.contains("from the `format` hint"), "{}", later.stdout);
}

/// Range sums, and a brute force that insists on `l <= r` so that a query the wrong way round shows up.
const RANGE_BRUTE: &str = r#"
use std::io::Read;
fn main() {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    let mut it = s.split_whitespace().map(|t| t.parse::<usize>().unwrap());
    let (n, q) = (it.next().unwrap(), it.next().unwrap());
    let a: Vec<usize> = (0..n).map(|_| it.next().unwrap()).collect();
    for _ in 0..q {
        let (l, r) = (it.next().unwrap(), it.next().unwrap());
        assert!(1 <= l && l <= r && r <= n, "bad query {l} {r}");
        println!("{}", a[l - 1..r].iter().sum::<usize>());
    }
}
"#;

/// Wrong: forgets the first element of the range.
const RANGE_BUGGY: &str = r#"
use std::io::Read;
fn main() {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    let mut it = s.split_whitespace().map(|t| t.parse::<usize>().unwrap());
    let (n, q) = (it.next().unwrap(), it.next().unwrap());
    let a: Vec<usize> = (0..n).map(|_| it.next().unwrap()).collect();
    let mut pre = vec![0usize; n + 1];
    for i in 0..n { pre[i + 1] = pre[i] + a[i]; }
    for _ in 0..q {
        let (l, r) = (it.next().unwrap(), it.next().unwrap());
        println!("{}", pre[r] - pre[l]);
    }
}
"#;

fn range_project(name: &str) -> Project {
    let p = Project::new("inferred", name);
    // Two samples with different numbers of queries, and a query on a single element in each (so the
    // queries are not a simple graph), tell the queries from arrays.
    p.solution(RANGE_BUGGY).brute(RANGE_BRUTE).problem(&[
        ("5 3\n3 1 4 1 5\n1 3\n2 5\n4 4\n", "8\n11\n1\n"),
        ("4 2\n2 2 9 1\n1 4\n2 2\n", "14\n2\n"),
    ]);
    p
}

#[test]
fn queries_never_have_their_ends_the_wrong_way_round() {
    let p = range_project("range");
    let r = p.run_with(&["--stress", "--seed", "1"]);
    // Not a setup problem ("the brute force failed": a query with l > r): the queries are in order.
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("v: u..n"), "{}", r.stdout);
    for line in counterexample_input(&p).lines().skip(2) {
        let q: Vec<usize> = line.split(' ').map(|v| v.parse().unwrap()).collect();
        assert!(q[0] <= q[1], "{line}");
    }
}

#[test]
fn declining_the_order_at_the_prompt_lets_bad_queries_through() {
    let p = range_project("range_declined");
    // A correct solution, so the run goes on until a query the wrong way round turns up.
    p.solution(RANGE_BRUTE);
    // Enter for n, m, a, u, v; Enter for the edges; "n" to `u ≤ v`.
    let r = p.run_typing(&["--stress", "--seed", "1", "--ask-constraints"], "\n\n\n\n\n\nn\n");
    assert!(r.stdout.contains("u ≤ v in every row? [y]: "), "{}", r.stdout);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("The brute force failed") && r.stdout.contains("bad query"), "{}", r.stdout);
}

// ---- hints -------------------------------------------------------------------------------

#[test]
fn the_vars_hint_sets_constraints() {
    let p = project("vars_hint");
    p.problem_with_hints(&SAMPLES, r#"{"vars":{"n":"3..3","a":"7..7"}}"#);
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "3"]);
    assert_eq!(counterexample_input(&p), "3\n7 7 7\n", "{}", r.stdout);
}

#[test]
fn the_format_hint_replaces_inference() {
    let p = project("format_hint");
    p.problem_with_hints(&SAMPLES, r#"{"format":["n","a[n]"],"vars":{"n":"3..3","a":"1..2"}}"#);
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "3"]);
    assert!(r.stdout.contains("Input format (from the `format` hint)"), "{}", r.stdout);
    assert!(!r.stdout.contains("guesses"), "every constraint was given: {}", r.stdout);
    assert_eq!(counterexample_input(&p).lines().next(), Some("3"));
}

#[test]
fn a_format_that_does_not_fit_the_samples_is_an_error() {
    let p = project("format_mismatch");
    p.problem_with_hints(&SAMPLES, r#"{"format":["n","a[n]","b[n]"]}"#);
    let r = p.run_with(&["--stress"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("do not fit the `format` hint"), "{}", r.stderr);
}

#[test]
fn constraints_the_samples_break_are_warned_about() {
    let p = project("warns");
    p.problem_with_hints(&SAMPLES, r#"{"vars":{"n":"1..2"}}"#);
    let r = p.run_with(&["--stress", "--iters", "1"]);
    assert!(r.stdout.contains("`n` is 3, outside 1..2"), "{}", r.stdout);
}

#[test]
fn a_multitest_input_is_recognised() {
    let p = project("multitest");
    p.problem(&[("2\n3\n1 2 3\n2\n4 5\n", "6\n9\n")]);
    let r = p.run_with(&["--stress", "--show-schema"]);
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("  t\n  repeat t {\n    n\n    a[n]\n  }"), "{}", r.stdout);
    assert!(r.stdout.contains("t: 1..5"), "{}", r.stdout);
}

#[test]
fn an_uncertain_reading_is_reported_with_the_choice_made() {
    let p = project("uncertain");
    // Two numbers, then two lines of two numbers: two arrays of 2, or 2 rows of 2.
    p.problem(&[("3 2\n1 2\n2 3\n", "x\n")]);
    let r = p.run_with(&["--stress", "--show-schema"]);
    assert!(r.stdout.contains("could also be rows"), "{}", r.stdout);
}

// ---- --show-schema --------------------------------------------------------------------------

#[test]
fn show_schema_prints_pasteable_hints_and_runs_nothing() {
    let p = project("show_schema");
    // Neither the brute force nor the solution is needed (or compiled) to look at the schema.
    fs::remove_file(p.root.join("sol/A_brute.rs")).unwrap();
    let r = p.run_with(&["--stress", "--show-schema"]);
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("Compiling"), "{}", r.stdout);
    assert!(!p.root.join("target").exists(), "nothing was built");

    let json = &r.stdout[r.stdout.find('{').expect("a JSON object")..];
    let hints: serde_like::Hints = serde_like::parse(json);
    assert_eq!(hints.format, ["n", "a[n]"]);
    assert_eq!(hints.vars, [("a".to_string(), "1..10".to_string()), ("n".to_string(), "1..size".to_string())]);
}

/// Just enough JSON handling for the test above, without adding a dependency.
mod serde_like {
    pub struct Hints {
        pub format: Vec<String>,
        pub vars: Vec<(String, String)>,
    }

    /// Reads the `"format": [...]` and `"vars": {...}` the judge prints (one entry per line).
    pub fn parse(json: &str) -> Hints {
        let quoted = |line: &str| -> Vec<String> { line.split('"').skip(1).step_by(2).map(str::to_string).collect() };
        let (mut format, mut vars) = (Vec::new(), Vec::new());
        let mut section = "";
        for line in json.lines() {
            let line = line.trim();
            if line.starts_with("\"format\"") {
                section = "format";
            } else if line.starts_with("\"vars\"") {
                section = "vars";
            } else if section == "format" && line.starts_with('"') {
                format.extend(quoted(line));
            } else if section == "vars" && line.starts_with('"') {
                let parts = quoted(line);
                vars.push((parts[0].clone(), parts[1].clone()));
            }
        }
        Hints { format, vars }
    }
}

#[test]
fn show_schema_output_can_be_saved_as_hints_and_gives_the_same_generator() {
    let p = project("show_schema_roundtrip");
    let shown = p.run_with(&["--stress", "--show-schema"]).stdout;
    let json = &shown[shown.find('{').unwrap()..];
    p.write("sol/A.stress.json", json);
    let r = p.run_with(&["--stress", "--seed", "1"]);
    assert!(r.stdout.contains("Input format (from the `format` hint)"), "{}", r.stdout);
    assert_eq!(r.code, Some(1), "{}", r.stdout);
}

// ---- --ask-constraints -------------------------------------------------------------------------

#[test]
fn typed_constraints_are_used_for_the_run_and_can_be_saved() {
    let p = project("ask_and_save");
    // n from 1 to 4; the numbers from 5 to 6; then yes, save.
    let r = p.run_typing(&["--stress", "--seed", "1", "--ask-constraints"], "1..4\n5..6\ny\n");
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("n (number) [1..size]: "), "{}", r.stdout);
    assert!(r.stdout.contains("a (array) [1..10]: "), "{}", r.stdout);
    assert!(r.stdout.contains("Constraints now:") && r.stdout.contains("n: 1..4") && r.stdout.contains("a: 5..6"), "{}", r.stdout);
    assert!(r.stdout.contains("Saved to"), "{}", r.stdout);

    // The tests of this run obey what was typed.
    let input = counterexample_input(&p);
    let mut lines = input.lines();
    let n: usize = lines.next().unwrap().parse().unwrap();
    let values: Vec<i64> = lines.next().unwrap().split(' ').map(|v| v.parse().unwrap()).collect();
    assert!((1..=4).contains(&n) && values.len() == n && values.iter().all(|v| (5..=6).contains(v)), "{input}");

    // The answers were saved next to the solution, with the format.
    let saved = fs::read_to_string(p.root.join("sol/A.stress.json")).expect("a sidecar file");
    assert!(saved.contains("\"n\": \"1..4\"") && saved.contains("\"a\": \"5..6\""), "{saved}");
    assert!(saved.contains("\"a[n]\""), "the format is saved too: {saved}");

    // A later run needs no typing: it reads the saved hints.
    let later = p.run_with(&["--stress", "--seed", "1"]);
    assert!(later.stdout.contains("Input format (from the `format` hint)"), "{}", later.stdout);
    assert!(later.stdout.contains("n: 1..4") && later.stdout.contains("a: 5..6"), "{}", later.stdout);
    let again = counterexample_input(&p);
    assert_eq!(again, input, "the same seed and the same saved constraints give the same counterexample");
}

#[test]
fn constraints_are_not_saved_unless_the_answer_is_yes() {
    let p = project("ask_decline");
    let r = p.run_typing(&["--stress", "--seed", "1", "--ask-constraints"], "1..4\n\nn\n");
    assert!(r.stdout.contains("Constraints now:") && r.stdout.contains("[y/N]"), "{}", r.stdout);
    assert!(!r.stdout.contains("Saved to"), "{}", r.stdout);
    assert!(!p.root.join("sol/A.stress.json").exists());
}

#[test]
fn enter_keeps_the_defaults_and_nothing_is_offered_for_saving() {
    let p = project("ask_enter");
    let r = p.run_typing(&["--stress", "--seed", "1", "--ask-constraints"], "\n\n");
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("[y/N]") && !r.stdout.contains("Constraints now"), "{}", r.stdout);
}

#[test]
fn a_wrong_answer_is_explained_and_asked_again() {
    let p = project("ask_wrong");
    let r = p.run_typing(&["--stress", "--seed", "1", "--ask-constraints"], "oops\n1..m\n1..4, distinct\n1..4\n\nn\n");
    assert!(r.stdout.contains("✗ expected `lo..hi`"), "{}", r.stdout);
    assert!(r.stdout.contains("✗ the constraint of `n` uses `m`"), "{}", r.stdout);
    assert!(r.stdout.contains("✗ `n` is a single number: `distinct` and `sorted` are for arrays"), "{}", r.stdout);
    assert!(r.stdout.contains("n: 1..4"), "the fourth answer was accepted: {}", r.stdout);
}

#[test]
fn the_question_mark_explains_the_syntax() {
    let p = project("ask_help");
    let r = p.run_typing(&["--stress", "--seed", "1", "--ask-constraints"], "?\n\n\n");
    assert!(r.stdout.contains("an integer from lo to hi") && r.stdout.contains("here you may use: size"), "{}", r.stdout);
}

#[test]
fn a_closed_input_keeps_the_defaults_instead_of_hanging() {
    let p = project("ask_closed");
    // No input at all (it is closed immediately): the run goes on with the inferred constraints.
    let r = p.run_with(&["--stress", "--seed", "1", "--ask-constraints"]);
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Counterexample found"), "{}", r.stdout);
}

#[test]
fn the_constraints_asked_for_are_those_of_the_format_hint_too() {
    let p = project("ask_with_format_hint");
    p.problem_with_hints(&SAMPLES, r#"{"format":["n","a[n]"]}"#);
    let r = p.run_typing(&["--stress", "--seed", "1", "--ask-constraints"], "3..3\n1..9\ny\n");
    assert!(r.stdout.contains("Saved to"), "{}", r.stdout);
    // The format came from a hint, so it is not copied into the sidecar; only the constraints are.
    let saved = fs::read_to_string(p.root.join("sol/A.stress.json")).unwrap();
    assert!(saved.contains("\"n\": \"3..3\"") && !saved.contains("\"format\""), "{saved}");
}

#[test]
fn the_constraint_options_need_the_inferred_generator() {
    let p = project("ask_needs_inferred");
    p.generator("fn main() { println!(\"3\\n1 2 3\"); }\n");
    for flag in ["--ask-constraints", "--show-schema"] {
        let r = p.run_with(&["--stress", flag]);
        assert_eq!(r.code, Some(2), "{flag}: stdout: {}\nstderr: {}", r.stdout, r.stderr);
        assert!(r.stderr.contains("about the inferred generator") && r.stderr.contains("A_gen.rs"), "{}", r.stderr);
    }
}

#[test]
fn the_constraint_options_need_stress() {
    let p = project("ask_needs_stress");
    let r = p.run_with(&["--ask-constraints"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("--ask-constraints only makes sense together with --stress"), "{}", r.stderr);
}

// ---- constraints that cannot be met ------------------------------------------------------------------

#[test]
fn constraints_that_allow_no_input_are_a_setup_problem_not_a_counterexample() {
    let p = project("impossible");
    // Eight different numbers cannot come from 1..3.
    p.problem_with_hints(&SAMPLES, r#"{"vars":{"n":"8..8","a":"1..3, distinct"}}"#);
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "5"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("The generator failed"), "{}", r.stdout);
    assert!(r.stdout.contains("cannot have 8 different values in 1..3"), "{}", r.stdout);
    assert!(r.stdout.contains("--ask-constraints"), "the way out is named: {}", r.stdout);
    assert!(!r.stdout.contains("Counterexample found"), "{}", r.stdout);
}

//! End-to-end tests for `--stress`: each test builds a throwaway Cargo project with a solution,
//! a brute force, a generator and sometimes a checker or interactor, and runs the real binary.
//!
//! The generators are deterministic functions of `(seed, size)`, so with a fixed `--seed` a run
//! is repeatable and the assertions can be exact. The seeds used here were picked so that the
//! interesting behaviour (a failure that has to be shrunk) really happens; if a generator is
//! changed, check that the shrinking tests still exercise a shrink.
//!
//! The main example is "sum of the array": the buggy solution only adds the first two numbers, so
//! it is wrong exactly when `n >= 3`, which makes the smallest counterexample have size 3.

mod common;

use common::*;
use std::{fs, process::Command};

const SUM_GEN_MAIN: &str = r#"
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: u64 = args[1].parse().unwrap();
    let size: u64 = args[2].parse().unwrap();
    let mut next = rng(seed);
    let n = 1 + next() % size;
    println!("{n}");
    let a: Vec<String> = (0..n).map(|_| ((next() % 21) as i64 - 10).to_string()).collect();
    println!("{}", a.join(" "));
}
"#;

/// Sums the first `take` numbers (all of them for `usize::MAX`), crashing if `crash_from` is reached.
fn sum_program(take: &str, crash_from: Option<usize>) -> String {
    let crash = crash_from
        .map(|n| format!("if n >= {n} {{ panic!(\"solution exploded\"); }}"))
        .unwrap_or_default();
    format!(
        r#"
use std::io::Read;
fn main() {{
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    let mut it = s.split_whitespace().map(|t| t.parse::<i64>().unwrap());
    let n = it.next().unwrap() as usize;
    {crash}
    let a: Vec<i64> = it.take(n).collect();
    let sum: i64 = a.iter().take({take}).sum();
    println!("{{sum}}");
}}
"#
    )
}

fn sum_generator() -> String {
    format!("{RNG}\n{SUM_GEN_MAIN}")
}

fn sum_correct() -> String {
    sum_program("usize::MAX", None)
}

/// Wrong whenever there are three or more numbers.
fn sum_buggy() -> String {
    sum_program("2", None)
}

/// A project for the sum problem with the given solution; brute force and generator are in place.
fn sum_project(name: &str, solution: &str) -> Project {
    let p = Project::new("stress", name);
    p.solution(solution).brute(&sum_correct()).generator(&sum_generator()).problem(&[("2\n3 4\n", "7\n")]);
    p
}

/// `--seed 1 --iters 3 --max-size 30` tries sizes 1, 11 and 21, so any failure is found well above
/// the smallest failing size and has to be shrunk.
const SHRINKING_RUN: [&str; 7] = ["--stress", "--seed", "1", "--iters", "3", "--max-size", "30"];

// ---- finding and shrinking ----------------------------------------------------

#[test]
fn a_counterexample_is_found_and_shrunk_to_the_smallest_size() {
    let p = sum_project("shrinks", &sum_buggy());
    let r = p.run_with(&SHRINKING_RUN);
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("shrunk from size"), "a failure at a larger size should be shrunk: {}", r.stdout);
    assert!(r.stdout.contains("to size 3 "), "n >= 3 is needed to fail, so size 3 is the smallest: {}", r.stdout);
    assert!(r.stdout.contains("--- Input ---"), "{}", r.stdout);
    assert!(r.stdout.contains("--- Expected (brute force) ---"), "{}", r.stdout);
    assert!(r.stdout.contains("--- Received ---"), "{}", r.stdout);
    assert!(r.stdout.contains("Counterexample WA"), "{}", r.stdout);
}

#[test]
fn the_reproduce_hint_regenerates_the_shown_input() {
    let p = sum_project("reproduce", &sum_buggy());
    let r = p.run_with(&SHRINKING_RUN);
    let line = r.stdout.lines().find(|l| l.starts_with("Reproduce the input with: ")).expect("a reproduce line");
    let mut parts = line.trim_start_matches("Reproduce the input with: ").split(' ');
    let (generator, seed, size) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());

    let regenerated = Command::new(generator).args([seed, size]).output().expect("the printed generator can be run");
    let saved = fs::read(p.root.join("target/judge/stress/counterexample.in")).expect("counterexample.in");
    assert_eq!(regenerated.stdout, saved);
    // The brute force's answer and the solution's are kept too.
    assert!(p.root.join("target/judge/stress/counterexample.out").is_file());
    assert!(p.root.join("target/judge/stress/counterexample.ans").is_file());
}

#[test]
fn a_correct_solution_survives_every_test() {
    let p = sum_project("passes", &sum_correct());
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "300"]);
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("No counterexample in 300 tests"), "{}", r.stdout);
}

#[test]
fn the_same_seed_gives_the_same_result() {
    let p = sum_project("repeatable", &sum_buggy());
    let first = p.run_with(&SHRINKING_RUN);
    let second = p.run_with(&SHRINKING_RUN);
    assert_eq!(first.stdout, second.stdout);
}

#[test]
fn an_unseeded_run_prints_a_seed_that_repeats_it() {
    let p = sum_project("unseeded", &sum_buggy());
    let unseeded = p.run_with(&["--stress", "--iters", "200"]);
    assert_eq!(unseeded.code, Some(1), "stdout: {}\nstderr: {}", unseeded.stdout, unseeded.stderr);
    let seed = unseeded
        .stdout
        .split("--seed ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("the seed is printed")
        .to_string();

    let repeated = p.run_with(&["--stress", "--iters", "200", "--seed", &seed]);
    let result = |out: &str| out.lines().filter(|l| l.contains("Counterexample found")).collect::<Vec<_>>().join("\n");
    assert_eq!(result(&unseeded.stdout), result(&repeated.stdout));
    assert!(!result(&repeated.stdout).is_empty(), "{}", repeated.stdout);
}

#[test]
fn a_crash_is_a_counterexample_with_its_trace_and_is_shrunk_too() {
    let p = sum_project("crash", &sum_program("usize::MAX", Some(3)));
    let r = p.run_with(&SHRINKING_RUN);
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Counterexample RE"), "{}", r.stdout);
    assert!(r.stdout.contains("solution exploded"), "the crash trace is shown: {}", r.stdout);
    assert!(r.stdout.contains("to size 3 "), "{}", r.stdout);
}

// ---- --save ---------------------------------------------------------------------

#[test]
fn save_appends_the_counterexample_as_a_regression_test() {
    let p = sum_project("save", &sum_buggy());
    let r = p.run_with(&[SHRINKING_RUN.as_slice(), &["--save"]].concat());
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Saved as test #2"), "{}", r.stdout);

    let prob: serde_like::Prob = serde_like::parse(&p.prob_json());
    assert_eq!(prob.test_count, 2, "{}", p.prob_json());
    let saved_input = fs::read_to_string(p.root.join("target/judge/stress/counterexample.in")).unwrap();
    assert!(prob.last_input_matches(&saved_input), "{}", p.prob_json());

    // The normal judge now fails on it: the regression test works.
    let normal = p.run();
    assert_eq!(normal.code, Some(1), "stdout: {}\nstderr: {}", normal.stdout, normal.stderr);
    assert!(normal.stdout.contains("Test #1") && normal.stdout.contains("Final Score: 1/2"), "{}", normal.stdout);
}

/// Just enough JSON handling for the test above, without adding a dependency.
mod serde_like {
    pub struct Prob {
        pub test_count: usize,
        text: String,
    }

    pub fn parse(text: &str) -> Prob {
        Prob { test_count: text.matches("\"input\":").count(), text: text.to_string() }
    }

    impl Prob {
        /// Whether the text contains `input` as a JSON string (newlines escaped).
        pub fn last_input_matches(&self, input: &str) -> bool {
            self.text.contains(&format!("\"input\":\"{}\"", input.replace('\n', "\\n")))
        }
    }
}

#[test]
fn without_save_the_prob_file_is_left_alone() {
    let p = sum_project("no_save", &sum_buggy());
    let before = p.prob_json();
    p.run_with(&SHRINKING_RUN);
    assert_eq!(p.prob_json(), before);
}

// ---- a checker in stress mode -----------------------------------------------------

/// "Print two distinct positive integers that sum to n": many answers are right.
const SPLIT_GEN_MAIN: &str = r#"
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: u64 = args[1].parse().unwrap();
    let size: u64 = args[2].parse().unwrap();
    let mut next = rng(seed);
    println!("{}", 4 + next() % (size + 1));
}
"#;

const SPLIT_CHECKER: &str = r#"
use std::{env, fs, process::exit};
fn main() {
    let args: Vec<String> = env::args().collect();
    let n: i64 = fs::read_to_string(&args[1]).unwrap().trim().parse().unwrap();
    let out = fs::read_to_string(&args[2]).unwrap();
    let nums: Vec<i64> = out.split_whitespace().map(|t| t.parse().unwrap()).collect();
    if nums.len() != 2 { eprintln!("expected 2 numbers"); exit(2); }
    let (a, b) = (nums[0], nums[1]);
    if a <= 0 || b <= 0 || a == b || a + b != n { eprintln!("{} + {} is not a valid split of {}", a, b, n); exit(1); }
}
"#;

/// A program that reads one integer `n` and then runs `body`.
fn read_n_program(body: &str) -> String {
    format!(
        "use std::io::Read;\nfn main() {{ let mut s = String::new(); std::io::stdin().read_to_string(&mut s).unwrap(); \
         let n: i64 = s.trim().parse().unwrap(); {body} }}\n"
    )
}

fn split_project(name: &str) -> Project {
    let p = Project::new("stress", name);
    // The brute force prints 1 and n-1; the solution prints 2 and n-2, which is just as good
    // except for n = 4 (2 and 2 are not distinct).
    p.solution(&read_n_program("println!(\"2 {}\", n - 2);"))
        .brute(&read_n_program("println!(\"1 {}\", n - 1);"))
        .generator(&format!("{RNG}\n{SPLIT_GEN_MAIN}"))
        .problem(&[("5\n", "1 4")]);
    p
}

#[test]
fn without_a_checker_a_different_valid_answer_is_a_false_counterexample() {
    let p = split_project("split_no_checker");
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "20"]);
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Counterexample found at test #1"), "every answer differs from the brute force's: {}", r.stdout);
}

#[test]
fn a_checker_accepts_valid_answers_and_still_finds_the_real_bug() {
    let p = split_project("split_checker");
    p.checker(SPLIT_CHECKER);
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "60"]);
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("--- Checker ---"), "{}", r.stdout);
    assert!(r.stdout.contains("2 + 2 is not a valid split of 4"), "only n = 4 is really wrong: {}", r.stdout);
    assert!(r.stdout.contains("--- Brute force answer ---"), "{}", r.stdout);
    assert!(r.stdout.contains("--- Received ---"), "{}", r.stdout);
}

#[test]
fn a_checker_that_rejects_the_brute_forces_own_answer_is_a_setup_problem() {
    let p = split_project("split_bad_brute");
    // The brute force answers "1 1", which a sound checker must reject.
    p.checker(SPLIT_CHECKER).brute("use std::io::Read;\nfn main() { let mut s = String::new(); std::io::stdin().read_to_string(&mut s).unwrap(); println!(\"1 1\"); }\n");
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "20"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("rejected the brute force's own answer"), "{}", r.stdout);
    assert!(!r.stdout.contains("Counterexample found"), "{}", r.stdout);
}

// ---- problems with the setup are not counterexamples -------------------------------------

#[test]
fn a_crashing_brute_force_is_reported_as_a_setup_problem() {
    let p = sum_project("brute_crash", &sum_correct());
    p.brute("fn main() { panic!(\"brute exploded\"); }\n");
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "20"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("The brute force failed on a generated input"), "{}", r.stdout);
    assert!(r.stdout.contains("brute exploded"), "{}", r.stdout);
    assert!(r.stdout.contains("not a counterexample"), "{}", r.stdout);
    assert!(r.stdout.contains("--- Input ---"), "the input that broke it is shown: {}", r.stdout);
}

#[test]
fn a_failing_generator_is_reported_as_a_setup_problem() {
    let p = sum_project("gen_fails", &sum_correct());
    p.generator("fn main() { eprintln!(\"generator broke\"); std::process::exit(1); }\n");
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "20"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("The generator failed"), "{}", r.stdout);
    assert!(r.stdout.contains("generator broke"), "{}", r.stdout);
}

#[test]
fn a_generator_that_prints_nothing_is_reported() {
    let p = sum_project("gen_silent", &sum_correct());
    p.generator("fn main() {}\n");
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "5"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("printed nothing"), "{}", r.stdout);
}

#[test]
fn a_missing_generator_names_the_file_it_expected() {
    let p = sum_project("no_gen", &sum_correct());
    fs::remove_file(p.root.join("sol/A_gen.rs")).unwrap();
    let r = p.run_with(&["--stress"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("needs a generator") && r.stderr.contains("A_gen.rs"), "{}", r.stderr);
}

#[test]
fn a_missing_brute_force_names_the_file_it_expected() {
    let p = sum_project("no_brute", &sum_correct());
    fs::remove_file(p.root.join("sol/A_brute.rs")).unwrap();
    let r = p.run_with(&["--stress"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("needs a brute-force solution") && r.stderr.contains("A_brute.rs"), "{}", r.stderr);
}

// ---- hints ------------------------------------------------------------------------

#[test]
fn hints_can_put_the_files_anywhere_and_set_the_number_of_tests() {
    // Relative hint paths are resolved against the solution's directory (`sol/`).
    let p = Project::new("stress", "hints");
    p.solution(&sum_correct())
        .write("sol/tools/slow.rs", &sum_correct())
        .write("sol/tools/make.rs", &sum_generator())
        .problem_with_hints(&[("2\n3 4\n", "7\n")], r#"{"brute":"tools/slow.rs","gen":"tools/make.rs","iters":12,"maxSize":4}"#);
    let r = p.run_with(&["--stress", "--seed", "1"]);
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("up to 12 tests, sizes 1..=4"), "{}", r.stdout);
    assert!(r.stdout.contains("No counterexample in 12 tests"), "{}", r.stdout);
}

#[test]
fn command_line_options_win_over_hints() {
    let p = sum_project("cli_over_hints", &sum_correct());
    p.problem_with_hints(&[("2\n3 4\n", "7\n")], r#"{"iters":12,"maxSize":4}"#);
    let r = p.run_with(&["--stress", "--seed", "1", "--iters", "5", "--max-size", "9"]);
    assert!(r.stdout.contains("up to 5 tests, sizes 1..=9"), "{}", r.stdout);
}

#[test]
fn a_sidecar_file_overrides_the_prob_hints() {
    let p = sum_project("sidecar", &sum_correct());
    p.problem_with_hints(&[("2\n3 4\n", "7\n")], r#"{"iters":500}"#).write("sol/A.stress.json", r#"{"iters":7}"#);
    let r = p.run_with(&["--stress", "--seed", "1"]);
    assert!(r.stdout.contains("up to 7 tests"), "{}", r.stdout);
}

#[test]
fn a_hint_naming_a_missing_file_is_an_error_not_a_fallback() {
    let p = sum_project("hint_missing", &sum_correct());
    p.problem_with_hints(&[("2\n3 4\n", "7\n")], r#"{"brute":"nope.rs"}"#);
    let r = p.run_with(&["--stress"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("`brute` hint") && r.stderr.contains("nope.rs"), "{}", r.stderr);
}

#[test]
fn a_checker_hint_also_applies_to_ordinary_judging() {
    let p = Project::new("stress", "checker_hint");
    // The checker is not named after the solution; only the hint finds it.
    p.solution(&read_n_program("println!(\"1 {}\", n - 1);"))
        .write("sol/tools/judge_split.rs", SPLIT_CHECKER)
        .problem_with_hints(&[("5\n", "2 3")], r#"{"checker":"tools/judge_split.rs"}"#);
    let r = p.run();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Using checker judge_split.rs"), "{}", r.stdout);
}

#[test]
fn stress_hints_do_not_change_ordinary_judging() {
    let p = sum_project("hints_ignored", &sum_correct());
    p.problem_with_hints(&[("2\n3 4\n", "7\n")], r#"{"iters":3}"#);
    let r = p.run();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Final Score: 1/1"), "{}", r.stdout);
    assert!(!r.stdout.contains("Stress"), "{}", r.stdout);
}

// ---- interactive problems -----------------------------------------------------------

const GUESS_GEN_MAIN: &str = r#"
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: u64 = args[1].parse().unwrap();
    let size: u64 = args[2].parse().unwrap();
    let mut next = rng(seed);
    println!("{}", 1 + next() % size);
}
"#;

/// A solution that answers one too many whenever the secret is at least 10.
fn guess_buggy() -> String {
    let fixed = GUESS_SOLUTION.replace(
        "    writeln!(out, \"! {lo}\").unwrap();",
        "    let lo = if lo >= 10 { lo + 1 } else { lo };\n    writeln!(out, \"! {lo}\").unwrap();",
    );
    assert_ne!(fixed, GUESS_SOLUTION, "the guessing-game solution changed; update this replacement");
    fixed
}

fn guess_project(name: &str, solution: &str) -> Project {
    let p = Project::new("stress", name);
    p.solution(solution).interactor(GUESS_INTERACTOR).generator(&format!("{RNG}\n{GUESS_GEN_MAIN}")).interactive_problem(&["7\n"], 5000);
    p
}

#[test]
fn an_interactive_solution_is_stress_tested_against_the_interactor_without_a_brute_force() {
    let p = guess_project("interactive", &guess_buggy());
    let r = p.run_with(&["--stress", "--seed", "3", "--iters", "40"]);
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("--- Interactor ---"), "{}", r.stdout);
    assert!(r.stdout.contains("wrong answer: 11 is not the secret"), "{}", r.stdout);
    // Secrets of 10 and above fail; size 10 is the smallest size that can produce one.
    assert!(r.stdout.contains("size 10"), "{}", r.stdout);
    assert!(!r.stdout.contains("brute"), "no brute force is involved: {}", r.stdout);
}

#[test]
fn a_correct_interactive_solution_survives_stress_testing() {
    let p = guess_project("interactive_ok", GUESS_SOLUTION);
    let r = p.run_with(&["--stress", "--seed", "3", "--iters", "60"]);
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("No counterexample in 60 tests"), "{}", r.stdout);
}

#[test]
fn a_failing_interactor_is_a_setup_problem_in_stress_mode_too() {
    let p = guess_project("interactor_fails", GUESS_SOLUTION);
    p.interactor("fn main() { eprintln!(\"hidden data is corrupt\"); std::process::exit(3); }\n");
    let r = p.run_with(&["--stress", "--seed", "3", "--iters", "10"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("The interactor failed") && r.stdout.contains("hidden data is corrupt"), "{}", r.stdout);
}

// ---- the command line ------------------------------------------------------------------

#[test]
fn stress_only_options_are_rejected_without_stress() {
    let p = sum_project("flags", &sum_correct());
    let r = p.run_with(&["--iters", "5"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("only makes sense together with --stress"), "{}", r.stderr);
    assert!(r.stderr.contains("Usage:"), "{}", r.stderr);
}

#[test]
fn unknown_options_are_rejected() {
    let p = sum_project("unknown_flag", &sum_correct());
    let r = p.run_with(&["--stres"]);
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("unknown option `--stres`"), "{}", r.stderr);
}

#[test]
fn help_prints_the_usage_and_succeeds() {
    let p = sum_project("help", &sum_correct());
    let r = p.run_with(&["--help"]);
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Usage: judge") && r.stdout.contains("--stress"), "{}", r.stdout);
}

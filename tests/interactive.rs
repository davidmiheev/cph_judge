//! End-to-end tests for interactive problems: each test builds a throwaway Cargo project
//! with a solution and an `_interactor` file next to it and runs the real `cph_judge`
//! binary on it.
//!
//! The example problem is a guessing game. The interactor knows a secret number (the
//! test's input, handed to it as a file), answers `? x` queries with `<`, `>` or `=`
//! relative to the secret, and expects the final answer as `! x` within 25 queries.

mod common;

use common::*;
use std::fs;

/// Exit codes follow testlib: 0 OK, 1 WA, 2 PE.
const GUESS_INTERACTOR: &str = r#"
use std::io::{self, BufRead, Write};
use std::process::exit;

fn main() {
    let path = std::env::args().nth(1).expect("input file argument");
    let secret: i64 = std::fs::read_to_string(path).unwrap().trim().parse().unwrap();
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut out = io::stdout();
    for _ in 0..25 {
        let mut line = String::new();
        if input.read_line(&mut line).unwrap() == 0 {
            // Let the judge see which side stopped first.
            std::thread::sleep(std::time::Duration::from_millis(50));
            eprintln!("unexpected EOF");
            exit(1);
        }
        let line = line.trim();
        if let Some(x) = line.strip_prefix("? ") {
            let x: i64 = x.parse().unwrap();
            let reply = if x < secret { "<" } else if x > secret { ">" } else { "=" };
            writeln!(out, "{reply}").unwrap();
            out.flush().unwrap();
        } else if let Some(x) = line.strip_prefix("! ") {
            let x: i64 = x.parse().unwrap();
            if x == secret {
                exit(0);
            }
            eprintln!("wrong answer: {x} is not the secret");
            exit(1);
        } else {
            eprintln!("unknown command {line:?}");
            exit(2);
        }
    }
    eprintln!("too many queries");
    exit(1);
}
"#;

/// Binary search over 1..=1_000_000: about 20 queries.
const GUESS_SOLUTION: &str = r#"
use std::io::{self, BufRead, Write};

fn main() {
    let (mut lo, mut hi) = (1i64, 1_000_000i64);
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut out = io::stdout();
    while lo < hi {
        let mid = (lo + hi) / 2;
        writeln!(out, "? {mid}").unwrap();
        out.flush().unwrap();
        let mut reply = String::new();
        input.read_line(&mut reply).unwrap();
        if reply.trim() == "<" { lo = mid + 1 } else { hi = mid }
    }
    writeln!(out, "! {lo}").unwrap();
    out.flush().unwrap();
}
"#;

/// Three hidden numbers, including both ends of the range.
const SECRETS: [&str; 3] = ["1\n", "1000000\n", "424242\n"];

fn guessing_game(name: &str) -> Project {
    let p = Project::new("interactive", name);
    p.solution(GUESS_SOLUTION).interactor(GUESS_INTERACTOR);
    p
}

// ---- the happy path -------------------------------------------------------

#[test]
fn a_correct_solution_passes_every_test() {
    let p = guessing_game("passes");
    p.interactive_problem(&SECRETS, 5000);
    let r = p.run();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Using interactor A_interactor.rs"), "{}", r.stdout);
    assert!(r.stdout.contains("Final Score: 3/3"), "{}", r.stdout);
}

#[test]
fn many_round_trips_do_not_deadlock() {
    let p = Project::new("interactive", "round_trips");
    p.interactor(
        r#"
use std::io::{self, BufRead, Write};
use std::process::exit;
fn main() {
    let n: u32 = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap().trim().parse().unwrap();
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut out = io::stdout();
    for i in 1..=n {
        writeln!(out, "{i}").unwrap();
        out.flush().unwrap();
        let mut reply = String::new();
        input.read_line(&mut reply).unwrap();
        if reply.trim().parse::<u32>().ok() != Some(i + 1) {
            eprintln!("at {i}: expected {}, got {:?}", i + 1, reply.trim());
            exit(1);
        }
    }
}
"#,
    )
    .solution(
        r#"
use std::io::{self, BufRead, Write};
fn main() {
    let stdin = io::stdin();
    let mut out = io::stdout();
    for line in stdin.lock().lines() {
        let x: u32 = line.unwrap().trim().parse().unwrap();
        writeln!(out, "{}", x + 1).unwrap();
        out.flush().unwrap();
    }
}
"#,
    )
    .interactive_problem(&["20000\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Final Score: 1/1"), "{}", r.stdout);
}

// ---- the interactor rejects the solution ----------------------------------

#[test]
fn a_wrong_final_answer_is_rejected_with_the_interactors_message() {
    let p = Project::new("interactive", "wrong_answer");
    p.solution("fn main() { println!(\"! 1\"); }\n").interactor(GUESS_INTERACTOR);
    p.interactive_problem(&["424242\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("--- Interactor ---"), "{}", r.stdout);
    assert!(r.stdout.contains("wrong answer: 1 is not the secret"), "{}", r.stdout);
    assert!(r.stdout.contains("Final Score: 0/1"), "{}", r.stdout);
    // The interactor's input file is kept for debugging.
    assert_eq!(fs::read_to_string(p.root.join("target/judge/1.in")).unwrap(), "424242\n");
    assert!(r.stdout.contains("target/judge"), "{}", r.stdout);
}

#[test]
fn exceeding_the_query_limit_is_the_interactors_verdict_not_a_crash() {
    let p = Project::new("interactive", "too_many_queries");
    p.solution(
        r#"
use std::io::{self, BufRead, Write};
fn main() {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut out = io::stdout();
    loop {
        writeln!(out, "? 1").unwrap();
        out.flush().unwrap();
        let mut reply = String::new();
        input.read_line(&mut reply).unwrap();
    }
}
"#,
    )
    .interactor(GUESS_INTERACTOR)
    .interactive_problem(&["424242\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("too many queries"), "{}", r.stdout);
    // The solution then dies of the closed pipes, but that is a consequence, not the verdict.
    assert!(!r.stdout.contains("Test #1 RE"), "{}", r.stdout);
}

#[test]
fn a_solution_that_stops_early_is_rejected_by_the_interactor() {
    let p = Project::new("interactive", "stops_early");
    p.solution("fn main() {}\n").interactor(GUESS_INTERACTOR).interactive_problem(&["7\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("unexpected EOF"), "{}", r.stdout);
}

#[test]
fn a_rust_interactor_that_panics_writing_to_a_solution_that_left_blames_the_solution() {
    // The solution ends the conversation at once. The interactor, a Rust program, then fails
    // to write to it: Rust ignores SIGPIPE, so this is a panic on the broken pipe rather than a
    // signal. That is how the solution's early exit shows up, not a failure of the interactor.
    let p = Project::new("interactive", "writes_to_closed_pipe");
    p.solution("fn main() {}\n")
        .interactor(
            r#"
use std::io::Write;
fn main() {
    std::thread::sleep(std::time::Duration::from_millis(100));
    let mut out = std::io::stdout();
    writeln!(out, "hello").unwrap();
    out.flush().unwrap();
}
"#,
        )
        .interactive_problem(&["1\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("--- Interactor ---") && r.stdout.contains("stopped reading"), "{}", r.stdout);
    // It is a verdict on the solution, not the "interactor failed" label for a judge-side failure.
    assert!(!r.stdout.contains("Test #1 interactor failed"), "{}", r.stdout);
}

// ---- the solution fails first ----------------------------------------------

#[test]
fn a_crashing_solution_gets_a_runtime_error_with_its_trace() {
    let p = Project::new("interactive", "crash");
    p.solution(
        r#"
use std::io::{self, BufRead, Write};
fn main() {
    let mut out = io::stdout();
    writeln!(out, "? 5").unwrap();
    out.flush().unwrap();
    let mut reply = String::new();
    io::stdin().lock().read_line(&mut reply).unwrap();
    panic!("solution exploded");
}
"#,
    )
    .interactor(GUESS_INTERACTOR)
    .interactive_problem(&["424242\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Test #1 RE"), "{}", r.stdout);
    assert!(r.stdout.contains("solution exploded"), "the crash trace should be shown: {}", r.stdout);
    // What the interactor made of it is context, not the verdict.
    assert!(r.stdout.contains("unexpected EOF"), "{}", r.stdout);
    assert!(!r.stdout.contains("--- Interactor ---"), "{}", r.stdout);
}

#[test]
fn a_solution_that_spins_is_tle() {
    let p = Project::new("interactive", "spins");
    p.solution("fn main() { let mut x: u64 = 0; loop { x = std::hint::black_box(x.wrapping_add(1)); } }\n")
        .interactor(GUESS_INTERACTOR)
        .interactive_problem(&["424242\n"], 500);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Test #1 TLE"), "{}", r.stdout);
}

#[test]
fn a_solution_that_hangs_is_tle_and_the_note_points_at_the_wall_clock_cap() {
    let p = Project::new("interactive", "hangs");
    p.solution("fn main() { std::thread::sleep(std::time::Duration::from_secs(60)); }\n")
        .interactor(GUESS_INTERACTOR)
        .interactive_problem(&["424242\n"], 300);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Test #1 TLE"), "{}", r.stdout);
    assert!(r.stdout.contains("wall-clock cap"), "{}", r.stdout);
}

// ---- the interactor itself fails ----------------------------------------------

#[test]
fn an_interactor_failure_is_reported_separately_from_a_wrong_answer() {
    let p = Project::new("interactive", "interactor_fails");
    p.solution(GUESS_SOLUTION)
        .interactor("fn main() { eprintln!(\"hidden data is corrupt\"); std::process::exit(3); }\n")
        .interactive_problem(&["424242\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("interactor failed"), "{}", r.stdout);
    assert!(r.stdout.contains("hidden data is corrupt"), "{}", r.stdout);
    assert!(!r.stdout.contains("--- Interactor ---"), "a judge-side failure is not a WA: {}", r.stdout);
}

// ---- choosing the mode ---------------------------------------------------------

#[test]
fn an_interactive_problem_without_an_interactor_is_an_error() {
    let p = Project::new("interactive", "no_interactor");
    p.solution(GUESS_SOLUTION).interactive_problem(&["5\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("marked interactive"), "{}", r.stderr);
    assert!(r.stderr.contains("A_interactor.rs"), "the expected file name should be given: {}", r.stderr);
}

#[test]
fn an_interactor_is_ignored_when_the_problem_is_not_interactive() {
    let p = Project::new("interactive", "not_interactive");
    p.solution(&solution_printing("2 3")).interactor(GUESS_INTERACTOR).problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Ignoring interactor A_interactor.rs"), "{}", r.stdout);
    assert!(r.stdout.contains("not marked interactive"), "{}", r.stdout);
}

#[test]
fn a_checker_is_ignored_for_an_interactive_problem() {
    let p = guessing_game("checker_ignored");
    // A checker that rejects everything would fail the test if it were consulted.
    p.checker("fn main() { std::process::exit(1); }\n").interactive_problem(&["424242\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Ignoring checker A_checker.rs"), "{}", r.stdout);
}

#[test]
fn a_problem_with_no_tests_says_so() {
    let p = guessing_game("no_tests");
    p.interactive_problem(&[], 5000);
    let r = p.run();
    assert!(r.stdout.contains("has no tests"), "{}", r.stdout);
}

// ---- building the interactor ------------------------------------------------

#[test]
fn an_interactor_that_does_not_compile_fails_the_build() {
    let p = Project::new("interactive", "broken_interactor");
    p.solution(GUESS_SOLUTION).interactor("fn main( {").interactive_problem(&["5\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("Compilation failed"), "{}", r.stderr);
    assert!(r.stderr.contains("interactor"), "{}", r.stderr);
}

#[test]
fn the_installed_interactor_source_is_removed_after_the_build() {
    let p = guessing_game("cleanup");
    p.interactive_problem(&["424242\n"], 5000);
    assert_eq!(p.run().code, Some(0));
    assert!(!p.root.join("src/bin/__judge_interactor.rs").exists());
    assert!(!p.root.join("src/bin").exists(), "src/bin was created by the judge, so it is removed");
}

#[test]
fn autobins_false_with_an_interactor_is_a_clear_error() {
    let p = guessing_game("autobins_false");
    p.write("Cargo.toml", CARGO_TOML_NO_AUTOBINS).interactive_problem(&["424242\n"], 5000);
    let r = p.run();
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("autobins"), "{}", r.stderr);
    assert!(r.stderr.contains("interactor"), "{}", r.stderr);
}

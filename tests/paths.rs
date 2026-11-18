//! The search directory may be given as a relative path (`cph_judge . 1840A`). The
//! judge runs the compiled programs with the project as their working directory, so every
//! path it hands to a child process has to be absolute.

mod common;

use common::*;

#[test]
fn a_relative_search_directory_works_for_an_ordinary_problem() {
    let p = Project::new("paths", "plain");
    p.solution(&solution_printing("2 3")).problem(&[("5\n", "2 3")]);
    let r = p.run_with_relative_path();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Final Score: 1/1"), "{}", r.stdout);
}

#[test]
fn a_relative_search_directory_works_with_a_checker() {
    let p = Project::new("paths", "checker");
    p.solution(&solution_printing("1 4"))
        .checker("fn main() {}\n")
        .problem(&[("5\n", "2 3")]);
    let r = p.run_with_relative_path();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Final Score: 1/1"), "{}", r.stdout);
}

#[test]
fn a_relative_search_directory_works_for_an_interactive_problem() {
    let p = Project::new("paths", "interactive");
    p.solution("fn main() { println!(\"hello\"); }\n")
        .interactor(
            r#"
use std::io::BufRead;
fn main() {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    std::process::exit(if line.trim() == "hello" { 0 } else { 1 });
}
"#,
        )
        .interactive_problem(&["unused\n"], 5000);
    let r = p.run_with_relative_path();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Final Score: 1/1"), "{}", r.stdout);
}

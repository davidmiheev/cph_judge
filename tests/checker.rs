//! End-to-end tests for custom checkers: each test builds a throwaway Cargo project,
//! lays out a `.prob` file, a solution and (optionally) a `_checker` file next to it,
//! and runs the real `cph_judge` binary on it.
//!
//! The example problem: "print two distinct positive integers that sum to n". Many
//! answers are valid, but the `.prob` only stores one of them (`2 3` for n = 5).

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const CARGO_TOML: &str = "[package]\nname = \"proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// Prints `1 n-1`: valid, but different from the stored answer `2 3` for n = 5.
const SPLIT_SOLUTION: &str = r#"
use std::io::Read;
fn main() {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    let n: i64 = s.trim().parse().unwrap();
    println!("1 {}", n - 1);
}
"#;

/// Accepts any two distinct positive integers summing to n.
/// Exits 1 (WA) for a bad split and 2 (PE) for malformed output.
const SPLIT_CHECKER: &str = r#"
use std::{env, fs, process::exit};
fn main() {
    let args: Vec<String> = env::args().collect();
    let input = fs::read_to_string(&args[1]).unwrap();
    let output = fs::read_to_string(&args[2]).unwrap();
    let n: i64 = input.trim().parse().unwrap();
    let parsed: Result<Vec<i64>, _> = output.split_whitespace().map(str::parse::<i64>).collect();
    let nums = match parsed {
        Ok(v) => v,
        Err(_) => { eprintln!("output is not a list of integers"); exit(2); }
    };
    if nums.len() != 2 {
        eprintln!("expected 2 numbers, got {}", nums.len());
        exit(2);
    }
    let (a, b) = (nums[0], nums[1]);
    if a <= 0 || b <= 0 || a == b || a + b != n {
        eprintln!("{} + {} is not a valid split of {}", a, b, n);
        exit(1);
    }
}
"#;

fn solution_printing(line: &str) -> String {
    format!("fn main() {{ println!(\"{line}\"); }}\n")
}

struct Judged {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

struct Project {
    root: PathBuf,
}

impl Project {
    /// A fresh project laid out like a CPH workspace: `Cargo.toml`, `src/main.rs`,
    /// `.cph/` for the `.prob` file, and `sol/` for the solution and its checker.
    fn new(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("checker_e2e_{name}"));
        let _ = fs::remove_dir_all(&root);
        for dir in ["src", ".cph", "sol"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("Cargo.toml"), CARGO_TOML).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        Project { root }
    }

    fn write(&self, relative: &str, contents: &str) -> &Self {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        self
    }

    fn solution(&self, code: &str) -> &Self {
        self.write("sol/A.rs", code)
    }

    fn checker(&self, code: &str) -> &Self {
        self.write("sol/A_checker.rs", code)
    }

    /// Writes `.cph/p.prob` with the given `(input, expected output)` tests.
    fn problem(&self, tests: &[(&str, &str)]) -> &Self {
        let tests: Vec<_> = tests
            .iter()
            .map(|(input, output)| serde_json_value(input, output))
            .collect();
        let prob = format!(
            "{{\"name\":\"p\",\"interactive\":false,\"timeLimit\":5000,\"memoryLimit\":256,\
             \"srcPath\":{},\"tests\":[{}]}}",
            json_string(self.root.join("sol/A.rs").to_str().unwrap()),
            tests.join(",")
        );
        self.write(".cph/p.prob", &prob)
    }

    fn run(&self) -> Judged {
        let Output { status, stdout, stderr } = Command::new(env!("CARGO_BIN_EXE_cph_judge"))
            .arg(&self.root)
            .arg("p")
            .env("NO_COLOR", "1")
            // A shared target dir would move the binaries the judge looks for.
            .env_remove("CARGO_TARGET_DIR")
            .output()
            .expect("failed to run cph_judge");
        Judged {
            code: status.code(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        }
    }
}

/// Minimal JSON string escaping, so the tests need no extra dependency.
fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn serde_json_value(input: &str, output: &str) -> String {
    format!("{{\"input\":{},\"output\":{}}}", json_string(input), json_string(output))
}

// ---- without a checker: the existing comparison is unchanged --------------

#[test]
fn without_a_checker_a_different_valid_answer_is_wrong() {
    let p = Project::new("no_checker_wa");
    p.solution(SPLIT_SOLUTION).problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("--- Expected ---") && r.stdout.contains("--- Received ---"), "{}", r.stdout);
}

#[test]
fn without_a_checker_floats_are_still_compared_with_tolerance() {
    let p = Project::new("no_checker_float");
    p.solution(&solution_printing("0.5000001")).problem(&[("", "0.5")]);
    let r = p.run();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Final Score: 1/1"), "{}", r.stdout);
}

// ---- with a checker ------------------------------------------------------

#[test]
fn checker_accepts_a_valid_answer_that_differs_from_the_stored_one() {
    let p = Project::new("accepts_alternative");
    p.solution(SPLIT_SOLUTION).checker(SPLIT_CHECKER).problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Using checker A_checker.rs"), "{}", r.stdout);
    assert!(r.stdout.contains("Final Score: 1/1"), "{}", r.stdout);
}

#[test]
fn checker_rejects_an_invalid_answer_and_shows_its_message() {
    let p = Project::new("rejects_with_message");
    p.solution(&solution_printing("1 1000")).checker(SPLIT_CHECKER).problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("--- Checker ---"), "{}", r.stdout);
    assert!(r.stdout.contains("1 + 1000 is not a valid split of 5"), "{}", r.stdout);
    assert!(r.stdout.contains("Final Score: 0/1"), "{}", r.stdout);
    assert!(
        !r.stdout.contains("--- Expected ---"),
        "the checker's message replaces the expected/received dump: {}",
        r.stdout
    );
}

#[test]
fn checker_exit_code_2_is_reported_as_a_presentation_error() {
    let p = Project::new("presentation_error");
    p.solution(&solution_printing("7")).checker(SPLIT_CHECKER).problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Test #1 PE"), "{}", r.stdout);
    assert!(r.stdout.contains("expected 2 numbers, got 1"), "{}", r.stdout);
}

#[test]
fn checker_failure_is_reported_separately_from_a_wrong_answer() {
    let p = Project::new("checker_failure");
    p.solution(SPLIT_SOLUTION)
        .checker("fn main() { eprintln!(\"jury data is broken\"); std::process::exit(3); }\n")
        .problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("checker failed"), "{}", r.stdout);
    assert!(r.stdout.contains("jury data is broken"), "{}", r.stdout);
    assert!(!r.stdout.contains("--- Checker ---"), "a checker failure is not a WA: {}", r.stdout);
}

#[test]
fn checker_receives_the_input_the_output_and_the_jury_answer_as_files() {
    let p = Project::new("checker_args");
    p.solution(SPLIT_SOLUTION)
        .checker(
            r#"
use std::{env, fs, process::exit};
fn main() {
    let a: Vec<String> = env::args().collect();
    eprintln!("in={:?} out={:?} ans={:?}",
        fs::read_to_string(&a[1]).unwrap(),
        fs::read_to_string(&a[2]).unwrap(),
        fs::read_to_string(&a[3]).unwrap());
    exit(1);
}
"#,
        )
        .problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert!(r.stdout.contains(r#"in="5\n" out="1 4\n" ans="2 3""#), "{}", r.stdout);
    // The files are left in target/judge for debugging a failing test.
    assert!(r.stdout.contains("target/judge"), "{}", r.stdout);
    assert_eq!(fs::read_to_string(p.root.join("target/judge/1.ans")).unwrap(), "2 3");
}

#[test]
fn a_run_error_is_reported_without_consulting_the_checker() {
    let p = Project::new("re_skips_checker");
    p.solution("fn main() { let v: Vec<i32> = vec![]; println!(\"{}\", v[1]); }\n")
        .checker(SPLIT_CHECKER)
        .problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Test #1 RE"), "{}", r.stdout);
    assert!(!r.stdout.contains("checker failed"), "{}", r.stdout);
}

// ---- building the checker ------------------------------------------------

#[test]
fn a_checker_that_does_not_compile_fails_the_build() {
    let p = Project::new("broken_checker");
    p.solution(SPLIT_SOLUTION).checker("fn main( {").problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert_eq!(r.code, Some(1), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("Compilation failed"), "{}", r.stderr);
    assert!(r.stderr.contains("built together"), "{}", r.stderr);
}

#[test]
fn the_installed_checker_source_is_removed_after_the_build() {
    let p = Project::new("cleanup");
    p.solution(SPLIT_SOLUTION).checker(SPLIT_CHECKER).problem(&[("5\n", "2 3")]);
    assert_eq!(p.run().code, Some(0));
    assert!(!p.root.join("src/bin/__judge_checker.rs").exists());
    assert!(!p.root.join("src/bin").exists(), "src/bin was created by the judge, so it is removed");
}

#[test]
fn files_the_user_keeps_in_src_bin_are_left_alone() {
    let p = Project::new("keeps_user_bins");
    p.solution(SPLIT_SOLUTION).checker(SPLIT_CHECKER).problem(&[("5\n", "2 3")]);
    p.write("src/bin/mine.rs", "fn main() { /* mine */ }\n");
    let r = p.run();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(!p.root.join("src/bin/__judge_checker.rs").exists());
    assert_eq!(fs::read_to_string(p.root.join("src/bin/mine.rs")).unwrap(), "fn main() { /* mine */ }\n");
}

#[test]
fn autobins_false_with_a_checker_is_a_clear_error() {
    let p = Project::new("autobins_false");
    p.write(
        "Cargo.toml",
        "[package]\nname = \"proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautobins = false\n\
         [[bin]]\nname = \"proj\"\npath = \"src/main.rs\"\n",
    );
    p.solution(SPLIT_SOLUTION).checker(SPLIT_CHECKER).problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert_eq!(r.code, Some(2), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("autobins"), "{}", r.stderr);
}

#[test]
fn autobins_false_without_a_checker_still_works() {
    let p = Project::new("autobins_false_plain");
    p.write(
        "Cargo.toml",
        "[package]\nname = \"proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautobins = false\n\
         [[bin]]\nname = \"proj\"\npath = \"src/main.rs\"\n",
    );
    p.solution(&solution_printing("2 3")).problem(&[("5\n", "2 3")]);
    let r = p.run();
    assert_eq!(r.code, Some(0), "stdout: {}\nstderr: {}", r.stdout, r.stderr);
}

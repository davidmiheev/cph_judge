//! Helpers shared by the end-to-end test suites: build a throwaway Cargo project laid out
//! like a CPH workspace, write a `.prob`, a solution and auxiliary files into it, and run
//! the real `cph_judge` binary on it.
#![allow(dead_code)] // each suite uses a different subset

use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

pub const CARGO_TOML: &str = "[package]\nname = \"proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// `Cargo.toml` that turns off binary auto-discovery, which the judge cannot build around.
pub const CARGO_TOML_NO_AUTOBINS: &str = "[package]\nname = \"proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautobins = false\n\
     [[bin]]\nname = \"proj\"\npath = \"src/main.rs\"\n";

/// Exit codes follow testlib: 0 OK, 1 WA, 2 PE.
pub const GUESS_INTERACTOR: &str = r#"
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
pub const GUESS_SOLUTION: &str = r#"
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


/// A small xorshift generator for the example generators, so they need no crates.
pub const RNG: &str = r#"
fn rng(seed: u64) -> impl FnMut() -> u64 {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) | 1;
    move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    }
}
"#;

/// Sums the first `take` numbers (all of them for `usize::MAX`), crashing if `crash_from` is reached.
/// Input: `n` and then `n` numbers.
pub fn sum_program(take: &str, crash_from: Option<usize>) -> String {
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

pub fn sum_correct() -> String {
    sum_program("usize::MAX", None)
}

/// Wrong whenever there are three or more numbers.
pub fn sum_buggy() -> String {
    sum_program("2", None)
}

pub fn solution_printing(line: &str) -> String {
    format!("fn main() {{ println!(\"{line}\"); }}\n")
}

pub struct Judged {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub struct Project {
    pub root: PathBuf,
}

impl Project {
    /// A fresh project laid out like a CPH workspace: `Cargo.toml`, `src/main.rs`,
    /// `.cph/` for the `.prob` file, and `sol/` for the solution and its auxiliary files.
    pub fn new(suite: &str, name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{suite}_e2e_{name}"));
        let _ = fs::remove_dir_all(&root);
        for dir in ["src", ".cph", "sol"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("Cargo.toml"), CARGO_TOML).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        Project { root }
    }

    pub fn write(&self, relative: &str, contents: &str) -> &Self {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        self
    }

    pub fn solution(&self, code: &str) -> &Self {
        self.write("sol/A.rs", code)
    }

    pub fn checker(&self, code: &str) -> &Self {
        self.write("sol/A_checker.rs", code)
    }

    pub fn interactor(&self, code: &str) -> &Self {
        self.write("sol/A_interactor.rs", code)
    }

    pub fn brute(&self, code: &str) -> &Self {
        self.write("sol/A_brute.rs", code)
    }

    pub fn generator(&self, code: &str) -> &Self {
        self.write("sol/A_gen.rs", code)
    }

    /// Writes `.cph/p.prob` for an ordinary problem with the given `(input, expected output)` tests.
    pub fn problem(&self, tests: &[(&str, &str)]) -> &Self {
        self.prob(tests, false, 5000, None)
    }

    /// Like [`problem`](Self::problem), with a `"stress"` hints object (given as JSON) in the `.prob`.
    pub fn problem_with_hints(&self, tests: &[(&str, &str)], hints_json: &str) -> &Self {
        self.prob(tests, false, 5000, Some(hints_json))
    }

    /// Writes `.cph/p.prob` for an interactive problem. The tests' inputs are what the
    /// interactor reads; their stored outputs are unused.
    pub fn interactive_problem(&self, inputs: &[&str], time_limit_ms: u64) -> &Self {
        let tests: Vec<(&str, &str)> = inputs.iter().map(|input| (*input, "")).collect();
        self.prob(&tests, true, time_limit_ms, None)
    }

    fn prob(&self, tests: &[(&str, &str)], interactive: bool, time_limit_ms: u64, hints_json: Option<&str>) -> &Self {
        let tests: Vec<String> = tests
            .iter()
            .map(|(input, output)| format!("{{\"input\":{},\"output\":{}}}", json_string(input), json_string(output)))
            .collect();
        let hints = hints_json.map(|json| format!(",\"stress\":{json}")).unwrap_or_default();
        let prob = format!(
            "{{\"name\":\"p\",\"interactive\":{interactive},\"timeLimit\":{time_limit_ms},\"memoryLimit\":256,\
             \"srcPath\":{}{hints},\"tests\":[{}]}}",
            json_string(self.root.join("sol/A.rs").to_str().unwrap()),
            tests.join(",")
        );
        self.write(".cph/p.prob", &prob)
    }

    pub fn run(&self) -> Judged {
        self.run_with(&[])
    }

    /// Runs the judge with extra command-line arguments after the project and problem name.
    pub fn run_with(&self, extra: &[&str]) -> Judged {
        self.judge(Command::new(env!("CARGO_BIN_EXE_cph_judge")).arg(&self.root).arg("p").args(extra))
    }

    /// The `.prob` file as parsed JSON.
    pub fn prob_json(&self) -> String {
        fs::read_to_string(self.root.join(".cph/p.prob")).unwrap()
    }

    /// Like [`run`](Self::run), but names the project by a path relative to the judge's
    /// working directory, the way `cph_judge . 1840A` would.
    pub fn run_with_relative_path(&self) -> Judged {
        let parent = self.root.parent().unwrap();
        let name = self.root.file_name().unwrap();
        self.judge(Command::new(env!("CARGO_BIN_EXE_cph_judge")).current_dir(parent).arg(name).arg("p"))
    }

    /// Runs the judge with `typed` on its standard input, as an answer to its prompts.
    pub fn run_typing(&self, extra: &[&str], typed: &str) -> Judged {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cph_judge"));
        command.arg(&self.root).arg("p").args(extra);
        self.judge_with_stdin(&mut command, Some(typed))
    }

    fn judge(&self, command: &mut Command) -> Judged {
        self.judge_with_stdin(command, None)
    }

    fn judge_with_stdin(&self, command: &mut Command, typed: Option<&str>) -> Judged {
        command
            .env("NO_COLOR", "1")
            // A shared target dir would move the binaries the judge looks for.
            .env_remove("CARGO_TARGET_DIR");
        let Output { status, stdout, stderr } = match typed {
            None => command.output().expect("failed to run cph_judge"),
            Some(typed) => {
                let mut child = command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("failed to run cph_judge");
                // Dropping the handle closes the input, so the judge sees the end of what was typed.
                child.stdin.take().expect("piped").write_all(typed.as_bytes()).expect("write the typed answers");
                child.wait_with_output().expect("wait for cph_judge")
            }
        };
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

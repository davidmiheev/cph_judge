//! Helpers shared by the end-to-end test suites: build a throwaway Cargo project laid out
//! like a CPH workspace, write a `.prob`, a solution and auxiliary files into it, and run
//! the real `cph_judge` binary on it.
#![allow(dead_code)] // each suite uses a different subset

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

pub const CARGO_TOML: &str = "[package]\nname = \"proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// `Cargo.toml` that turns off binary auto-discovery, which the judge cannot build around.
pub const CARGO_TOML_NO_AUTOBINS: &str = "[package]\nname = \"proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautobins = false\n\
     [[bin]]\nname = \"proj\"\npath = \"src/main.rs\"\n";

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

    /// Writes `.cph/p.prob` for an ordinary problem with the given `(input, expected output)` tests.
    pub fn problem(&self, tests: &[(&str, &str)]) -> &Self {
        self.prob(tests, false, 5000)
    }

    /// Writes `.cph/p.prob` for an interactive problem. The tests' inputs are what the
    /// interactor reads; their stored outputs are unused.
    pub fn interactive_problem(&self, inputs: &[&str], time_limit_ms: u64) -> &Self {
        let tests: Vec<(&str, &str)> = inputs.iter().map(|input| (*input, "")).collect();
        self.prob(&tests, true, time_limit_ms)
    }

    fn prob(&self, tests: &[(&str, &str)], interactive: bool, time_limit_ms: u64) -> &Self {
        let tests: Vec<String> = tests
            .iter()
            .map(|(input, output)| format!("{{\"input\":{},\"output\":{}}}", json_string(input), json_string(output)))
            .collect();
        let prob = format!(
            "{{\"name\":\"p\",\"interactive\":{interactive},\"timeLimit\":{time_limit_ms},\"memoryLimit\":256,\
             \"srcPath\":{},\"tests\":[{}]}}",
            json_string(self.root.join("sol/A.rs").to_str().unwrap()),
            tests.join(",")
        );
        self.write(".cph/p.prob", &prob)
    }

    pub fn run(&self) -> Judged {
        self.judge(Command::new(env!("CARGO_BIN_EXE_cph_judge")).arg(&self.root).arg("p"))
    }

    /// Like [`run`](Self::run), but names the project by a path relative to the judge's
    /// working directory, the way `cph_judge . 1840A` would.
    pub fn run_with_relative_path(&self) -> Judged {
        let parent = self.root.parent().unwrap();
        let name = self.root.file_name().unwrap();
        self.judge(Command::new(env!("CARGO_BIN_EXE_cph_judge")).current_dir(parent).arg(name).arg("p"))
    }

    fn judge(&self, command: &mut Command) -> Judged {
        let Output { status, stdout, stderr } = command
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

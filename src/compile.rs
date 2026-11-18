//! Building the user's Cargo project.
//!
//! The solution is built from `src/main.rs`. Auxiliary programs (currently the
//! checker) are built alongside it by installing judge-owned files into the
//! project's `src/bin/` for the duration of the build, so they get the project's
//! edition, profile and dependencies.

use serde::Deserialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

/// Name of the Cargo binary built from the user's `<solution>_checker.rs`.
pub const CHECKER_BIN: &str = "__judge_checker";

#[derive(Deserialize)]
struct CargoToml {
    package: Package,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    autobins: Option<bool>,
}

pub struct PackageInfo {
    pub name: String,
    /// Whether Cargo auto-discovers binaries in `src/bin/` (the default).
    pub autobins: bool,
}

pub enum BuildOutcome {
    Success,
    /// Compilation failed; `log` is the compiler's stderr.
    Failed { log: String },
}

pub fn package_info(project_root: &Path) -> Result<PackageInfo, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(project_root.join("Cargo.toml"))?;
    let cargo_toml: CargoToml = toml::from_str(&contents)?;
    Ok(PackageInfo {
        name: cargo_toml.package.name,
        autobins: cargo_toml.package.autobins.unwrap_or(true),
    })
}

/// Path of an auxiliary source that sits next to the solution and is named after it
/// with a suffix: kind `checker` turns `A.rs` into `A_checker.rs`. Returns it only if
/// the file exists.
pub fn aux_source(src_path: &Path, kind: &str) -> Option<PathBuf> {
    let stem = src_path.file_stem()?.to_string_lossy();
    let name = match src_path.extension() {
        Some(ext) => format!("{stem}_{kind}.{}", ext.to_string_lossy()),
        None => format!("{stem}_{kind}"),
    };
    let path = src_path.with_file_name(name);
    path.is_file().then_some(path)
}

/// Judge-owned source files placed in the project's `src/bin/` while it is built.
///
/// Dropping the guard removes exactly what was installed (and `src/bin/` itself if
/// the judge had to create it), so a user's own files in `src/bin/` are never touched.
pub struct GeneratedBins {
    files: Vec<PathBuf>,
    created_dir: Option<PathBuf>,
}

impl GeneratedBins {
    /// Copies each `(binary name, source)` pair to `src/bin/<name>.rs`.
    pub fn install(project_root: &Path, bins: &[(&str, &Path)]) -> io::Result<Self> {
        let dir = project_root.join("src/bin");
        let mut created_dir = None;
        if !bins.is_empty() && !dir.exists() {
            fs::create_dir_all(&dir)?;
            created_dir = Some(dir.clone());
        }

        // Build the guard first so a failure part-way through still cleans up.
        let mut guard = GeneratedBins { files: Vec::new(), created_dir };
        for (name, source) in bins {
            let dest = dir.join(format!("{name}.rs"));
            guard.files.push(dest.clone());
            fs::copy(source, &dest)?;
        }
        Ok(guard)
    }
}

impl Drop for GeneratedBins {
    fn drop(&mut self) {
        for file in &self.files {
            let _ = fs::remove_file(file);
        }
        if let Some(dir) = &self.created_dir {
            let _ = fs::remove_dir(dir); // only succeeds if empty
        }
    }
}

/// Runs `cargo build --release` for the given binaries in `project_root`, keeping the
/// compiler's stderr in `compiler_log.txt` (and returning it on failure).
pub fn build_release(project_root: &Path, bins: &[&str]) -> io::Result<BuildOutcome> {
    let log_path = project_root.join("compiler_log.txt");
    let log_file = fs::File::create(&log_path)?;

    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release"]);
    for bin in bins {
        cmd.args(["--bin", bin]);
    }
    let status = cmd.current_dir(project_root).stderr(log_file).status()?;

    if status.success() {
        Ok(BuildOutcome::Success)
    } else {
        Ok(BuildOutcome::Failed { log: fs::read_to_string(&log_path)? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty directory unique to this test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cph_judge_unit_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn aux_source_is_named_after_the_solution_with_a_suffix() {
        let dir = scratch("aux_named");
        fs::write(dir.join("A.rs"), "").unwrap();
        fs::write(dir.join("A_checker.rs"), "").unwrap();
        assert_eq!(aux_source(&dir.join("A.rs"), "checker"), Some(dir.join("A_checker.rs")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn aux_source_is_none_when_the_file_is_missing() {
        let dir = scratch("aux_missing");
        fs::write(dir.join("A.rs"), "").unwrap();
        assert_eq!(aux_source(&dir.join("A.rs"), "checker"), None);
        // A file for a different kind does not count.
        fs::write(dir.join("A_brute.rs"), "").unwrap();
        assert_eq!(aux_source(&dir.join("A.rs"), "checker"), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn aux_source_keeps_dots_in_the_stem_and_handles_no_extension() {
        let dir = scratch("aux_dots");
        fs::write(dir.join("A.test_checker.rs"), "").unwrap();
        fs::write(dir.join("sol_checker"), "").unwrap();
        assert_eq!(aux_source(&dir.join("A.test.rs"), "checker"), Some(dir.join("A.test_checker.rs")));
        assert_eq!(aux_source(&dir.join("sol"), "checker"), Some(dir.join("sol_checker")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn generated_bins_are_installed_then_removed_with_the_dir_they_created() {
        let dir = scratch("gen_created");
        let source = dir.join("checker_src.rs");
        fs::write(&source, "fn main() {}").unwrap();

        let guard = GeneratedBins::install(&dir, &[(CHECKER_BIN, source.as_path())]).unwrap();
        assert_eq!(fs::read_to_string(dir.join("src/bin/__judge_checker.rs")).unwrap(), "fn main() {}");
        drop(guard);

        assert!(!dir.join("src/bin/__judge_checker.rs").exists());
        assert!(!dir.join("src/bin").exists(), "src/bin was created by the judge, so it should be removed");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn generated_bins_leave_the_users_own_files_alone() {
        let dir = scratch("gen_preexisting");
        fs::create_dir_all(dir.join("src/bin")).unwrap();
        fs::write(dir.join("src/bin/mine.rs"), "fn main() { /* mine */ }").unwrap();
        let source = dir.join("checker_src.rs");
        fs::write(&source, "fn main() {}").unwrap();

        let guard = GeneratedBins::install(&dir, &[(CHECKER_BIN, source.as_path())]).unwrap();
        assert!(dir.join("src/bin/__judge_checker.rs").exists());
        drop(guard);

        assert!(!dir.join("src/bin/__judge_checker.rs").exists());
        assert_eq!(fs::read_to_string(dir.join("src/bin/mine.rs")).unwrap(), "fn main() { /* mine */ }");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn installing_nothing_creates_nothing() {
        let dir = scratch("gen_empty");
        let guard = GeneratedBins::install(&dir, &[]).unwrap();
        assert!(!dir.join("src/bin").exists());
        drop(guard);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn package_info_reads_name_and_autobins() {
        let dir = scratch("pkg");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n").unwrap();
        let info = package_info(&dir).unwrap();
        assert_eq!(info.name, "foo");
        assert!(info.autobins, "autobins defaults to true");

        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nautobins = false\n").unwrap();
        assert!(!package_info(&dir).unwrap().autobins);
        fs::remove_dir_all(&dir).unwrap();
    }
}

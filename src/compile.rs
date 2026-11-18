//! Building the user's Cargo project.

use serde::Deserialize;
use std::{fs, io, path::Path, process::Command};

#[derive(Deserialize)]
struct CargoToml {
    package: Package,
}

#[derive(Deserialize)]
struct Package {
    name: String,
}

pub enum BuildOutcome {
    Success,
    /// Compilation failed; `log` is the compiler's stderr.
    Failed { log: String },
}

/// Name of the binary Cargo produces for the project at `project_root`.
pub fn package_name(project_root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(project_root.join("Cargo.toml"))?;
    let cargo_toml: CargoToml = toml::from_str(&contents)?;
    Ok(cargo_toml.package.name)
}

/// Runs `cargo build --release` in `project_root`, keeping the compiler's stderr
/// in `compiler_log.txt` (and returning it on failure).
pub fn build_release(project_root: &Path) -> io::Result<BuildOutcome> {
    let log_path = project_root.join("compiler_log.txt");
    let log_file = fs::File::create(&log_path)?;

    let status = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(project_root)
        .stderr(log_file)
        .status()?;

    if status.success() {
        Ok(BuildOutcome::Success)
    } else {
        Ok(BuildOutcome::Failed { log: fs::read_to_string(&log_path)? })
    }
}

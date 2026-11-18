//! Per-problem hints: where the auxiliary files are and how a stress run is tuned.
//!
//! Hints come from an optional `"stress"` object in the `.prob` file and, as a fallback in
//! case the editor that owns the `.prob` drops keys it does not know, from a sidecar file
//! next to the solution: `A.rs` is accompanied by `A.stress.json`. Where both set a field,
//! the sidecar wins. Unknown fields are ignored, so hints for later features do no harm.
//!
//! ```json
//! "stress": { "brute": "slow.rs", "gen": "tools/make.rs", "iters": 500, "maxSize": 12 }
//! ```
//!
//! Relative file names are resolved against the solution's directory.

use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct Hints {
    pub brute: Option<String>,
    #[serde(rename = "gen")]
    pub generator: Option<String>,
    pub checker: Option<String>,
    pub interactor: Option<String>,
    pub iters: Option<u32>,
    #[serde(rename = "maxSize")]
    pub max_size: Option<u32>,
    #[serde(rename = "shrinkSeeds")]
    pub shrink_seeds: Option<u32>,
}

impl Hints {
    /// `self` with every field it leaves unset taken from `fallback`.
    pub fn or(self, fallback: Hints) -> Hints {
        Hints {
            brute: self.brute.or(fallback.brute),
            generator: self.generator.or(fallback.generator),
            checker: self.checker.or(fallback.checker),
            interactor: self.interactor.or(fallback.interactor),
            iters: self.iters.or(fallback.iters),
            max_size: self.max_size.or(fallback.max_size),
            shrink_seeds: self.shrink_seeds.or(fallback.shrink_seeds),
        }
    }
}

/// `A.rs` -> `A.stress.json`, next to it.
pub fn sidecar_path(src_path: &Path) -> PathBuf {
    let stem = src_path.file_stem().unwrap_or_default().to_string_lossy();
    src_path.with_file_name(format!("{stem}.stress.json"))
}

/// The hints for a solution: the sidecar file if there is one, over the `.prob`'s own.
pub fn load(from_prob: Option<Hints>, src_path: &Path) -> Result<Hints, String> {
    let from_prob = from_prob.unwrap_or_default();
    let sidecar = sidecar_path(src_path);
    if !sidecar.is_file() {
        return Ok(from_prob);
    }
    let text = fs::read_to_string(&sidecar).map_err(|e| format!("could not read {}: {e}", sidecar.display()))?;
    let from_sidecar: Hints =
        serde_json::from_str(&text).map_err(|e| format!("{} is not valid hints JSON: {e}", sidecar.display()))?;
    Ok(from_sidecar.or(from_prob))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cph_judge_hints_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_every_field_and_ignores_unknown_ones() {
        let hints: Hints = serde_json::from_str(
            r#"{ "brute": "b.rs", "gen": "g.rs", "checker": "c.rs", "interactor": "i.rs",
                 "iters": 500, "maxSize": 12, "shrinkSeeds": 40,
                 "vars": { "n": "1..8" }, "multitest": true }"#,
        )
        .unwrap();
        assert_eq!(hints.brute.as_deref(), Some("b.rs"));
        assert_eq!(hints.generator.as_deref(), Some("g.rs"));
        assert_eq!(hints.checker.as_deref(), Some("c.rs"));
        assert_eq!(hints.interactor.as_deref(), Some("i.rs"));
        assert_eq!((hints.iters, hints.max_size, hints.shrink_seeds), (Some(500), Some(12), Some(40)));
    }

    #[test]
    fn an_empty_object_sets_nothing() {
        assert_eq!(serde_json::from_str::<Hints>("{}").unwrap(), Hints::default());
    }

    #[test]
    fn or_prefers_self_field_by_field() {
        let a = Hints { brute: Some("a".into()), iters: Some(1), ..Hints::default() };
        let b = Hints { brute: Some("b".into()), iters: Some(2), max_size: Some(3), ..Hints::default() };
        let merged = a.or(b);
        assert_eq!(merged.brute.as_deref(), Some("a"));
        assert_eq!(merged.iters, Some(1));
        assert_eq!(merged.max_size, Some(3), "fields self leaves unset come from the fallback");
    }

    #[test]
    fn the_sidecar_sits_next_to_the_solution() {
        assert_eq!(sidecar_path(Path::new("/p/A.rs")), PathBuf::from("/p/A.stress.json"));
        assert_eq!(sidecar_path(Path::new("/p/A.test.rs")), PathBuf::from("/p/A.test.stress.json"));
    }

    #[test]
    fn without_a_sidecar_the_prob_hints_are_used_as_they_are() {
        let dir = scratch("no_sidecar");
        let from_prob = Hints { iters: Some(9), ..Hints::default() };
        assert_eq!(load(Some(from_prob.clone()), &dir.join("A.rs")), Ok(from_prob));
        assert_eq!(load(None, &dir.join("A.rs")), Ok(Hints::default()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_sidecar_overrides_the_prob_where_both_set_a_field() {
        let dir = scratch("sidecar");
        fs::write(dir.join("A.stress.json"), r#"{ "iters": 5, "brute": "side.rs" }"#).unwrap();
        let from_prob = Hints { iters: Some(900), max_size: Some(7), ..Hints::default() };
        let hints = load(Some(from_prob), &dir.join("A.rs")).unwrap();
        assert_eq!(hints.iters, Some(5));
        assert_eq!(hints.brute.as_deref(), Some("side.rs"));
        assert_eq!(hints.max_size, Some(7), "the .prob still supplies what the sidecar leaves out");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_malformed_sidecar_is_an_error_naming_the_file() {
        let dir = scratch("bad_sidecar");
        fs::write(dir.join("A.stress.json"), "{ not json").unwrap();
        let error = load(None, &dir.join("A.rs")).unwrap_err();
        assert!(error.contains("A.stress.json") && error.contains("not valid"), "{error}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

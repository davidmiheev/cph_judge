//! Command-line parsing.
//!
//! ```text
//! judge <search_dir> <problem_name> [--stress [--iters N] [--seed S] [--max-size N] [--save]]
//! ```

pub const USAGE: &str = "\
Usage: judge <search_dir> <problem_name> [options]

Runs the problem's tests. With --stress, instead looks for an input on which the
solution disagrees with the brute force (or the interactor).

Stress options (only with --stress):
  --iters N      number of random tests (default 1000, or the `iters` hint)
  --seed S       seed of the first test; the same seed repeats a run
  --max-size N   largest size passed to the generator (default 20, or `maxSize`)
  --save         append the counterexample to the .prob file as a new test

Other options:
  -h, --help     show this message";

#[derive(Debug, PartialEq, Eq)]
pub struct Cli {
    pub search_dir: String,
    pub problem: String,
    pub stress: bool,
    pub iters: Option<u32>,
    pub seed: Option<u64>,
    pub max_size: Option<u32>,
    pub save: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Cli),
    Help,
}

/// Parses the arguments after the program name. Flags may come before, between or after
/// the two positional arguments, and take their value either as `--iters 5` or `--iters=5`.
pub fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut positional = Vec::new();
    let mut cli = Cli {
        search_dir: String::new(),
        problem: String::new(),
        stress: false,
        iters: None,
        seed: None,
        max_size: None,
        save: false,
    };

    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (flag, inline_value) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline_value
                .clone()
                .or_else(|| args.next().cloned())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--stress" => cli.stress = true,
            "--save" => cli.save = true,
            "--iters" => cli.iters = Some(positive(&value("--iters")?, "--iters")?),
            "--max-size" => cli.max_size = Some(positive(&value("--max-size")?, "--max-size")?),
            "--seed" => {
                let text = value("--seed")?;
                cli.seed = Some(text.parse().map_err(|_| format!("--seed needs a non-negative integer, got `{text}`"))?);
            }
            flag if flag.starts_with('-') && flag.len() > 1 => return Err(format!("unknown option `{flag}`")),
            _ => positional.push(arg.clone()),
        }
    }

    let [search_dir, problem] = <[String; 2]>::try_from(positional)
        .map_err(|found| format!("expected <search_dir> and <problem_name>, got {} argument(s)", found.len()))?;
    cli.search_dir = search_dir;
    cli.problem = problem;

    if !cli.stress {
        for (given, name) in [
            (cli.iters.is_some(), "--iters"),
            (cli.seed.is_some(), "--seed"),
            (cli.max_size.is_some(), "--max-size"),
            (cli.save, "--save"),
        ] {
            if given {
                return Err(format!("{name} only makes sense together with --stress"));
            }
        }
    }
    Ok(Parsed::Run(cli))
}

fn positive(text: &str, name: &str) -> Result<u32, String> {
    match text.parse::<u32>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{name} needs a positive integer, got `{text}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Result<Parsed, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn run(args: &[&str]) -> Cli {
        match parse_strs(args) {
            Ok(Parsed::Run(cli)) => cli,
            other => panic!("expected a run, got {other:?}"),
        }
    }

    #[test]
    fn the_original_two_argument_form_still_works() {
        let cli = run(&["problems", "1840A"]);
        assert_eq!((cli.search_dir.as_str(), cli.problem.as_str()), ("problems", "1840A"));
        assert!(!cli.stress && !cli.save);
        assert_eq!((cli.iters, cli.seed, cli.max_size), (None, None, None));
    }

    #[test]
    fn stress_flags_are_parsed_in_any_position() {
        let cli = run(&["--stress", "dir", "--iters", "50", "prob", "--seed=7", "--max-size", "9", "--save"]);
        assert_eq!((cli.search_dir.as_str(), cli.problem.as_str()), ("dir", "prob"));
        assert!(cli.stress && cli.save);
        assert_eq!((cli.iters, cli.seed, cli.max_size), (Some(50), Some(7), Some(9)));
    }

    #[test]
    fn help_is_recognised_anywhere() {
        assert_eq!(parse_strs(&["--help"]), Ok(Parsed::Help));
        assert_eq!(parse_strs(&["dir", "prob", "-h"]), Ok(Parsed::Help));
    }

    #[test]
    fn wrong_argument_counts_are_errors() {
        assert!(parse_strs(&[]).is_err());
        assert!(parse_strs(&["dir"]).is_err());
        let error = parse_strs(&["a", "b", "c"]).unwrap_err();
        assert!(error.contains("3 argument"), "{error}");
    }

    #[test]
    fn unknown_options_are_errors() {
        let error = parse_strs(&["dir", "prob", "--stres"]).unwrap_err();
        assert!(error.contains("unknown option `--stres`"), "{error}");
    }

    #[test]
    fn option_values_are_validated() {
        assert!(parse_strs(&["d", "p", "--stress", "--iters", "0"]).unwrap_err().contains("positive"));
        assert!(parse_strs(&["d", "p", "--stress", "--iters", "x"]).unwrap_err().contains("positive"));
        assert!(parse_strs(&["d", "p", "--stress", "--max-size=-3"]).unwrap_err().contains("positive"));
        assert!(parse_strs(&["d", "p", "--stress", "--seed", "-1"]).unwrap_err().contains("non-negative"));
        assert!(parse_strs(&["d", "p", "--stress", "--iters"]).unwrap_err().contains("needs a value"));
    }

    #[test]
    fn seed_zero_is_allowed() {
        assert_eq!(run(&["d", "p", "--stress", "--seed", "0"]).seed, Some(0));
    }

    #[test]
    fn stress_only_options_require_stress() {
        for flag in [&["--iters", "5"][..], &["--seed", "5"], &["--max-size", "5"], &["--save"]] {
            let mut args = vec!["d", "p"];
            args.extend_from_slice(flag);
            let error = parse_strs(&args).unwrap_err();
            assert!(error.contains("only makes sense together with --stress"), "{error}");
        }
    }
}

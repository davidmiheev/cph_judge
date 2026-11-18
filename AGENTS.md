# AGENTS.md

## Project Overview

`cph_judge` is a competitive programming judge tool written in Rust. It finds `.prob` problem files, compiles source code, and runs test cases in parallel against expected outputs with time and memory limit enforcement.

## Quick Commands

```bash
# Build
cargo build --release

# Run Tests
cargo test

# Run Judge (requires two args: search_dir and problem_name)
cargo run --release -- <search_dir> <problem_name>

# Stress test (needs <solution>_brute.rs, or an interactor; tests come from <solution>_gen.rs, or from the samples)
cargo run --release -- <search_dir> <problem_name> --stress [--iters N] [--seed S] [--max-size N] [--save] [--ask-constraints] [--show-schema]

# Example
cargo run --release -- /path/to/problemset bowling
```

## Project Structure

```
cph_judge/
├── Cargo.toml          # Rust project config
├── Cargo.lock          # Dependency lock file
├── src/
│   ├── main.rs         # CLI, problem discovery, orchestration, result printing
│   ├── compile.rs      # Package info, auxiliary-file discovery, `cargo build --release --bin ...`
│   ├── runner.rs       # run_once (process + pipes + wait4) and RunReport::check (verdicts)
│   ├── checker.rs      # Runs a custom _checker; exit_verdict maps testlib exit codes (shared with interactors)
│   ├── interactive.rs  # Solution + _interactor joined by pipes; decide() = first failure wins
│   ├── stress.rs       # --stress: generator + brute force/interactor, parallel chunks, shrinking, --save
│   ├── cli.rs          # Argument parsing and usage text
│   ├── hints.rs        # "stress" hints from the .prob or a sidecar <solution>.stress.json
│   ├── schema.rs       # Input format language + constraint syntax: expressions, parse/print/validate
│   ├── infer.rs        # Infers the format from the .prob samples (search with backtracking), default constraints
│   ├── generate.rs     # Makes a test from format + constraints + seed + size
│   ├── constraints.rs  # --ask-constraints dialogue and saving to <solution>.stress.json
│   ├── report.rs       # Terminal output shared by normal and stress mode
│   └── compare.rs      # Tokenized, float-aware output comparator
├── tests/
│   ├── common/mod.rs   # Shared helper: throwaway Cargo project + running the real binary
│   ├── checker.rs      # End-to-end tests for _checker
│   ├── interactive.rs  # End-to-end tests for _interactor
│   ├── stress.rs       # End-to-end tests for --stress, hints and --save
│   ├── inferred.rs     # End-to-end tests for the inferred generator and --ask-constraints
│   └── paths.rs        # Relative search directory regression tests
├── docs/
│   ├── arch.md         # System architecture and pipeline
│   ├── design-stress-interactive.md  # Plan: _checker, interactive, stress testing
│   ├── todo.md         # Project roadmap and backlog
│   └── analysis.md     # Code analysis
├── .gitignore          # Ignores /target and artifacts
├── LICENSE             # MIT License
└── README.md           # Project readme
```

## .prob File Format

The judge expects JSON config files with `.prob` extension (from Competitive Companion / CPH):

```json
{
  "name": "Problem Name",
  "interactive": false,
  "timeLimit": 2000,
  "memoryLimit": 256,
  "srcPath": "relative/or/absolute/path/to/source.rs",
  "tests": [
    { "input": "test input", "output": "expected output" }
  ]
}
```

## Verdicts & Test Evaluation

- **AC (Accepted)**: Output matches expected (tokenized comparison with $\epsilon = 10^{-6}$ for floating-point values).
- **WA (Wrong Answer)**: Output mismatches expected tokens. If `<solution>_checker.rs` exists next to the solution, it decides instead: it is built with the solution as `src/bin/__judge_checker.rs` (removed after the build), run as `checker <input> <output> <answer>` on files in `target/judge/`, and its exit code maps `0` AC, `1` WA (message shown), `2` PE, anything else `checker failed` (not counted as passed, never WA).
- **Interactive problems** (`"interactive": true`): the solution runs against `<solution>_interactor.rs` (`interactor <input_file>`, testlib exit codes, joined by two pipes) instead of an output comparison. The first failure wins; a solution that stops reading early gets WA, not an interactor failure; a checker is ignored. The interactor is required for interactive problems (exit code 2 without it).
- **Stress testing** (`--stress`): for each seed and a size that ramps up, `<solution>_gen.rs` (`gen <seed> <size>`) makes a test; `<solution>_brute.rs` gives the jury answer (compared, or given to a checker after the checker has accepted it itself), or the interactor decides. The lowest failing test is shrunk by searching smaller sizes for a failure of the same kind. A failing generator, brute force, checker or interactor is a setup problem (exit code 2), never a counterexample. Without a generator file the tests are made from the input format inferred from the `.prob` samples (lines of numbers, arrays, strings, `repeat` blocks; integers only), with constraints that are guesses from the samples: they are printed on every run, and set by the `format`/`vars`/`multitest` hints or typed with `--ask-constraints` (which can save them to the sidecar). A variable that sets a length or count, or bounds another, is limited to the test size so small tests come first. Hints in a `"stress"` object in the `.prob` or in `<solution>.stress.json` name these files and tune the run.
- **TLE (Time Limit Exceeded)**: User + system CPU time above `timeLimit`. A running process is polled every 10 ms (Linux `/proc/<pid>/stat`) and killed by process tree termination (`pkill -9 -P`, `kill -9`) once over the limit; the final check uses `wait4` `rusage`. A wall-clock cap (`2 × timeLimit + 500 ms`) also kills blocked or sleeping processes, and is the only live enforcement on platforms without `/proc`.
- **MLE (Memory Limit Exceeded)**: Checked via `wait4::Wait4` `rusage.maxrss` against configured `memoryLimit`.
- **RE (Runtime Error)**: Checked when process exits with non-zero status or termination signal.
- Checks run in the order TLE, RE, MLE, then output comparison.
- **Exit codes**: `0` all tests passed (stress: no counterexample), `1` compile failure, any non-AC test or (stress) a counterexample, `2` usage / judge error (stress: a setup problem).

Requires Rust 1.88 or newer (`rust-version` in `Cargo.toml`): `std::io::pipe` for interactive problems and let-chains.

## Dependencies

| Crate | Version | Purpose |
|---|---|---|
| serde | 1.0 | JSON serialization/deserialization |
| serde_json | 1.0 | JSON parsing for `.prob` files |
| ignore | 0.4 | Fast recursive directory walking |
| toml | 0.8 | Cargo.toml parsing |
| colored | 2.1 | Terminal color and formatting |
| tokio | 1.51 | Async runtime and timeout management |
| wait4 | 0.1 | Process status and resource usage (`rusage`) retrieval |

## Documentation Reference

- Architecture: [`docs/arch.md`](docs/arch.md)
- Roadmap: [`docs/todo.md`](docs/todo.md)
- Analysis: [`docs/analysis.md`](docs/analysis.md)
- Checker / interactive / stress plan: [`docs/design-stress-interactive.md`](docs/design-stress-interactive.md)

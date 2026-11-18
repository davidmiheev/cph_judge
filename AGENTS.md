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
│   ├── compile.rs      # Package name lookup and `cargo build --release`
│   ├── runner.rs       # run_once (process + pipes + wait4) and RunReport::check (verdicts)
│   └── compare.rs      # Tokenized, float-aware output comparator
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
- **WA (Wrong Answer)**: Output mismatches expected tokens.
- **TLE (Time Limit Exceeded)**: User + system CPU time above `timeLimit`. A running process is polled every 10 ms (Linux `/proc/<pid>/stat`) and killed by process tree termination (`pkill -9 -P`, `kill -9`) once over the limit; the final check uses `wait4` `rusage`. A wall-clock cap (`2 × timeLimit + 500 ms`) also kills blocked or sleeping processes, and is the only live enforcement on platforms without `/proc`.
- **MLE (Memory Limit Exceeded)**: Checked via `wait4::Wait4` `rusage.maxrss` against configured `memoryLimit`.
- **RE (Runtime Error)**: Checked when process exits with non-zero status or termination signal.
- Checks run in the order TLE, RE, MLE, then output comparison.
- **Exit codes**: `0` all tests passed, `1` compile failure or any non-AC test, `2` usage / judge error.

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

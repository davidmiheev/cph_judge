# Code Analysis: `cph_judge`

## Overview

A competitive programming judge tool written in Rust that discovers `.prob` problem files, compiles source code, and executes test cases with parallel async workers.

## Key Components

| File | Purpose |
|------|---------|
| `Cargo.toml` | Rust project configuration with `serde`, `ignore`, `toml`, `colored`, `tokio`, and `wait4` |
| `src/main.rs` | CLI parsing, compilation runner, async test execution, and tokenized output comparison |
| `docs/arch.md` | System architecture, pipeline, and verdict documentation |
| `docs/todo.md` | Project roadmap and backlog |

## Workflow

1. **Parse CLI arguments**: `cph_judge <search_dir> <problem_name>`.
2. **Locate metadata**: Recursively search `search_dir` for `<problem_name>.prob`.
3. **Parse CPH config**: Deserialize JSON metadata containing time/memory limits, source paths, and test cases.
4. **Prepare source code**: Copy problem solution file into target project `src/main.rs`.
5. **Compile release binary**: Run `cargo build --release` with stderr redirected to `compiler_log.txt`.
6. **Concurrent test execution**: Spawn Tokio async tasks using blocking workers and `wait4` to monitor CPU/memory.
7. **Verdict evaluation & output comparison**:
   - Compare tokens using whitespace insensitivity and floating-point tolerance ($\epsilon = 10^{-6}$).
   - Detect and report **AC**, **WA**, **TLE**, **MLE**, and **RE** verdicts.
8. **Summary**: Print colored per-test statistics and final test score.

## Analysis of Recent Fixes

1. **Floating-point output comparison**: Replaced raw string equality comparison with tokenized evaluation supporting absolute and relative tolerance ($\epsilon = 10^{-6}$), handling floating-point formatting differences across test outputs.
2. **Runtime Error detection**: Handled non-zero exit statuses and signals in `wait4` to report `RE` instead of treating crashed processes as empty outputs.

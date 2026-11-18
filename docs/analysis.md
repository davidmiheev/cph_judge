# Code Analysis: `cph_judge`

## Overview

A competitive programming judge tool written in Rust that discovers `.prob` problem files, compiles source code, and executes test cases with parallel async workers.

## Key Components

| File | Purpose |
|------|---------|
| `Cargo.toml` | Rust project configuration with `serde`, `ignore`, `toml`, `colored`, `tokio`, and `wait4` |
| `src/main.rs` | CLI parsing, problem discovery, test orchestration, and result printing |
| `src/compile.rs` | Reads the package name and runs `cargo build --release` |
| `src/runner.rs` | Runs one process under limits (`run_once`) and turns the report into a verdict (`RunReport::check`) |
| `src/compare.rs` | Tokenized, float-aware output comparison |
| `docs/arch.md` | System architecture, pipeline, and verdict documentation |
| `docs/design-stress-interactive.md` | Plan for `_checker`, interactive problems, and stress testing |
| `docs/todo.md` | Project roadmap and backlog |

## Workflow

1. **Parse CLI arguments**: `cph_judge <search_dir> <problem_name>`.
2. **Locate metadata**: Recursively search `search_dir` for `<problem_name>.prob`.
3. **Parse CPH config**: Deserialize JSON metadata containing time/memory limits, source paths, and test cases.
4. **Prepare source code**: Copy problem solution file into target project `src/main.rs`.
5. **Compile release binary**: Run `cargo build --release` with stderr redirected to `compiler_log.txt`.
6. **Concurrent test execution**: Spawn Tokio async tasks (at most one running per core) using blocking workers and `wait4` to monitor CPU time and memory.
7. **Verdict evaluation & output comparison**:
   - Compare tokens using whitespace insensitivity and floating-point tolerance ($\epsilon = 10^{-6}$).
   - Detect and report **AC**, **WA**, **TLE**, **MLE**, and **RE** verdicts with detailed panic/crash location traces.
8. **Summary**: Print colored per-test statistics and final test score.

## Analysis of Recent Fixes

1. **Floating-point output comparison**: Replaced raw string equality comparison with tokenized evaluation supporting absolute and relative tolerance ($\epsilon = 10^{-6}$), handling floating-point formatting differences across test outputs.
2. **Runtime Error detection & crash trace reporting**: Captured child process `stderr` alongside `stdout`, inspected `wait4` status and signals (`SIGABRT`, `SIGSEGV`, `SIGFPE`), and formatted crash/panic traces to pinpoint where runtime errors occur.
3. **Runner refactor and stdin deadlock fix**: The judge used to write all of a test's input before it started reading output, so a program that streams its output deadlocked on inputs larger than the pipe buffers and was reported as a false TLE (reproduced with a 2 MB input). Input is now written from a separate thread. At the same time the compile and run logic moved out of `main()`.
4. **CPU-time limits and bounded concurrency**: TLE is judged on user + system CPU time rather than wall-clock time, and tests no longer all start at once, so timings do not inflate with the number of tests. A wall-clock cap (`2 × timeLimit + 500 ms`) still catches blocked processes.
5. **Exit codes**: `0` all passed, `1` compile failure or failed tests, `2` usage / judge error.

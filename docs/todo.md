# Roadmap & TODO: `cph_judge`

Current state and planned improvements for `cph_judge`.

---

## Done

- [x] **Runner refactor**: `compile`, `run_once` and `RunReport::check` extracted from `main()` (phase 0 of [`design-stress-interactive.md`](design-stress-interactive.md)).
- [x] **Stdin deadlock fix**: input is written from its own thread, so programs that stream output no longer get a false TLE on large inputs.
- [x] **CPU-time limits**: TLE is judged on user + system CPU time, with a wall-clock cap for blocked processes; at most one test per core runs at a time.
- [x] **Clean Exit Codes**: `0` all passed, `1` compile failure or failed tests, `2` usage / judge error.
- [x] **Early Kill on CPU Limit**: a running process is polled and killed as soon as its CPU time passes `timeLimit` (Linux), so infinite loops are reported at the limit rather than after the wall-clock cap.

---

## High Priority

Checker, interactive and stress work is planned in [`design-stress-interactive.md`](design-stress-interactive.md) (phases 1-5).

- [ ] **Safe File Copy / Backup**: Before copying `src_path` to `src/main.rs`, create a backup of any existing `src/main.rs` and restore it after judge execution or on signal/panic. Note: copying a file onto itself (when `srcPath` is the project's own `src/main.rs`) truncates it.
- [ ] **Special Judge / Custom Checker** (phase 1): `A_checker.rs`, testlib argument order and exit codes.
- [ ] **Interactive Problems Support** (phase 2): Implement two-way process piping and interactor communication when `interactive: true`.
- [ ] **Stress Testing** (phases 3-5): `_brute`, `_gen`, inferred generator, shrinking.
- [ ] **Configurable Floating-Point Tolerance**: Allow per-problem or per-test configuration of $\epsilon$ tolerance (e.g., $10^{-9}$ for high precision problems).
- [ ] **Exact Problem Matching**: `find_prob_file` matches `name.contains(target)` on the `.prob` file name and takes the first hit, so a target like `re` matches `stream.prob`. Prefer an exact / best match and report ambiguity.

---

## Features & Improvements

- [ ] **CPU-Time Polling Outside Linux**: Early kill relies on `/proc`; on macOS an infinite loop is still only caught by the wall-clock cap (`2 × timeLimit + 500 ms`).
- [ ] **Multi-Language Support**: Extend beyond Rust to support compiling and executing C++, Python, Java, and Go solutions.
- [ ] **Diff Highlighting for WA**: Improve Wrong Answer visualization with colored diffs for multiline expected vs received mismatches.
- [ ] **Strict Memory Enforcement**: Enforce memory limits in real-time (e.g. via `setrlimit` or Linux cgroups) rather than post-execution `rusage`.

# Roadmap & TODO: `cph_judge`

Current state and planned improvements for `cph_judge`.

---

## Done

- [x] **Runner refactor**: `compile`, `run_once` and `RunReport::check` extracted from `main()` (phase 0 of [`design-stress-interactive.md`](design-stress-interactive.md)).
- [x] **Stdin deadlock fix**: input is written from its own thread, so programs that stream output no longer get a false TLE on large inputs.
- [x] **CPU-time limits**: TLE is judged on user + system CPU time, with a wall-clock cap for blocked processes; at most one test per core runs at a time.
- [x] **Clean Exit Codes**: `0` all passed, `1` compile failure or failed tests, `2` usage / judge error.
- [x] **Special Judge / Custom Checker** (phase 1): `<solution>_checker.rs` next to the solution, testlib argument order and exit codes; see [`design-stress-interactive.md`](design-stress-interactive.md).
- [x] **Stress Testing** (phase 3): `--stress` with `<solution>_gen.rs` and `<solution>_brute.rs` (or the interactor), size ramp, parallel batches, shrinking, `--save`; see [`design-stress-interactive.md`](design-stress-interactive.md).
- [x] **Inferred Generator** (phase 4): without a generator file, tests are made from the input format inferred from the `.prob` samples (`schema.rs`, `infer.rs`, `generate.rs`); `format`, `vars` and `multitest` hints; `--show-schema`.
- [x] **Trees, Graphs and `l <= r`**: a list of edges can be a `tree` or a `graph` (`connected`, `directed`, `ordered`), inferred from the samples, generated in varied shapes, reduced vertex by vertex, and asked about in `--ask-constraints` together with `l ≤ r`; the edge count is set to match (`graph.rs`).
- [x] **Schema-Aware Shrinking** (phase 5): a counterexample from the inferred generator is reduced to the least input that still fails in the same way: any repetition dropped, numbers lowered, arrays and strings simplified, all within the constraints (`reduce.rs`).
- [x] **Interactive Constraints**: `--ask-constraints` types the constraints of the generated input at a prompt, checked as typed, and offers to save them in `<solution>.stress.json`.
- [x] **Hints and Path Overrides**: a `"stress"` object in the `.prob` or a sidecar `<solution>.stress.json` names the checker, interactor, brute force and generator and tunes the stress run.
- [x] **Interactive Problems Support** (phase 2): `<solution>_interactor.rs` joined to the solution by two pipes, testlib exit codes, first-failure-wins verdicts.
- [x] **Relative Search Directories**: `cph_judge . 1840A` used to fail with "No such file or directory" because the project root stayed relative while child processes ran inside it. This predates the checker and interactive work (the original code behaves the same).
- [x] **Early Kill on CPU Limit**: a running process is polled and killed as soon as its CPU time passes `timeLimit` (Linux), so infinite loops are reported at the limit rather than after the wall-clock cap.

---

## High Priority

The plan in [`design-stress-interactive.md`](design-stress-interactive.md) is complete; what follows are improvements.

- [ ] **Safe File Copy / Backup**: Before copying `src_path` to `src/main.rs`, create a backup of any existing `src/main.rs` and restore it after judge execution or on signal/panic. Note: copying a file onto itself (when `srcPath` is the project's own `src/main.rs`) truncates it.
- [ ] **Reduce Hand-Written Generators' Output Too**: reduction needs the input format and constraints, so it only runs for the inferred generator. For an `_gen` file it could use a format inferred from the samples, but a guessed format cannot say what stays valid; offer it only when a `format` hint is given.
- [ ] **More Relations in the Inferred Generator**: bipartite graphs, a tree given as a list of parents, weights, an adjacency matrix, and `l < r` (strictly). Trees, simple graphs and `l <= r` are done.
- [ ] **Floats, Blank Lines and Mixed Lines in Inference**: refused today. Floats would need a number of decimals; blank lines need to be part of the format.
- [ ] **Offer to Save Without Typing**: `--show-schema` prints hints to paste; a flag that writes the inferred format and constraints to `<solution>.stress.json` directly would save the paste.
- [ ] **Configurable Floating-Point Tolerance**: Allow per-problem or per-test configuration of $\epsilon$ tolerance (e.g., $10^{-9}$ for high precision problems).
- [ ] **Exact Problem Matching**: `find_prob_file` matches `name.contains(target)` on the `.prob` file name and takes the first hit, so a target like `re` matches `stream.prob`. Prefer an exact / best match and report ambiguity.

---

## Features & Improvements

- [ ] **Interaction Transcript**: when an interactive test fails, show what the two sides said. The judge never sees the data today because it flows directly between the processes; it would need to sit in the middle of the pipes.
- [ ] **Cap Captured Output**: `run_once` reads a program's stdout to the end into memory with no limit, so a program that prints without stopping fills memory until it is killed. The same applies to generators and brute forces in stress mode.
- [ ] **Verify CPH Compatibility**: check that the CPH editor keeps unknown keys in a `.prob` (for the `"stress"` hints) and accepts the test `id` that `--save` writes.
- [ ] **CPU-Time Polling Outside Linux**: Early kill relies on `/proc`; on macOS an infinite loop is still only caught by the wall-clock cap (`2 × timeLimit + 500 ms`).
- [ ] **Multi-Language Support**: Extend beyond Rust to support compiling and executing C++, Python, Java, and Go solutions.
- [ ] **Diff Highlighting for WA**: Improve Wrong Answer visualization with colored diffs for multiline expected vs received mismatches.
- [ ] **Strict Memory Enforcement**: Enforce memory limits in real-time (e.g. via `setrlimit` or Linux cgroups) rather than post-execution `rusage`.

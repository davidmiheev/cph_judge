# Design: checker, interactive problems, and stress testing

Status: **phases 0-2 done** (runner, CPU-time limits, `_checker`, interactive problems); the rest is planned. See [Phases](#phases).

All three features are the same machine with different inputs and a different
oracle (the thing that decides pass/fail):

| Mode        | Test input from            | Runner                  | Oracle                                   |
|-------------|----------------------------|-------------------------|------------------------------------------|
| normal      | `.prob` tests              | batch (stdin → stdout)  | stored expected output, or `_checker`    |
| stress      | generator (`_gen`)         | batch                   | `_brute` output, compared or via `_checker` |
| interactive | `.prob` tests or generator | piped to `_interactor`  | interactor exit code                     |

## Auxiliary files

Found next to the solution (`srcPath`) by **suffix** on the solution's file stem, so they
sort beside it. For `A.rs`:

| File             | Role                                        |
|------------------|---------------------------------------------|
| `A_brute.rs`     | slow, obviously-correct solution (stress)   |
| `A_gen.rs`       | test generator (stress)                     |
| `A_checker.rs`   | answer checker (normal and stress)          |
| `A_interactor.rs`| interactor (interactive)                    |

Each is meant to be overridable by path in the hints (see [Hints](#hints)); until the hints exist (phase 3) only
the suffix convention is implemented.

## Building the auxiliary binaries

Copy each to `src/bin/__judge_<kind>.rs` in the project and build everything with one
`cargo build --release --bin <solution> --bin __judge_<kind> ...`; delete only the files the
judge created afterwards (the `__judge_` prefix keeps a user's own `src/bin/` safe).

Why cargo and not `rustc`: bare `rustc` differs from the way the solution is built in ways
that matter when brute force and solution must behave identically. Checked on this toolchain:

- **Edition** — `rustc` defaults to 2015; `async fn` is rejected (`E0670`) without `--edition`.
- **Profile** — with `[profile.release] overflow-checks = true`, the cargo-built binary
  panics on `i32::MAX + 1` (exit 101) while `rustc -O` silently wraps.
- **Dependencies** — a crate from `[dependencies]` (e.g. `proconio`) resolves under cargo and
  fails under `rustc` (`cannot find module or crate`).

Known cost: with `autobins = false` in `Cargo.toml`, `cargo build --bin __judge_*` fails
(`no bin target named ...`). The judge detects that (when an auxiliary file is present) and exits
with code 2 and a clear message rather than falling back to `rustc`. Without auxiliary files the
setting is irrelevant.

Implemented in phase 1: the build always names its targets (`--bin <package> [--bin __judge_checker]`)
instead of building every target, so unrelated files in the user's `src/bin/` no longer affect it.
The installed sources are removed by a drop guard (`GeneratedBins`) whatever the build outcome; a
user's own `src/bin/` files are never touched, and `src/bin/` itself is removed only if the judge
created it.

## Checker (`A_checker.rs`)

Used in normal and stress mode when present; otherwise the built-in token/epsilon
comparison is used. Not used for interactive problems (the interactor is the checker).

- Invocation (testlib argument order): `checker <input_file> <contestant_output_file> <jury_answer_file>`.
  Message goes to stderr.
- Jury answer: the `.prob` expected output in normal mode; the brute-force output in stress mode.

| Exit                              | Verdict                                              |
|-----------------------------------|------------------------------------------------------|
| 0                                 | AC                                                   |
| 1                                 | WA — checker message shown instead of expected/received |
| 2                                 | PE (reported as a WA variant)                        |
| 3, other codes, crash, timeout    | "checker failed" — judge-side, not counted as WA     |

Stress sanity check: run `checker(input, brute_out, brute_out)` first. A sound checker must
accept the jury's own answer, so a rejection means the brute force or the checker is wrong.

Implemented in phase 1 (normal mode):

- The input, the program's output and the stored answer are written to
  `<project>/target/judge/N.in`, `N.out`, `N.ans` for test N. The directory is recreated at the start
  of each run and kept afterwards, so a failing test can be re-checked by hand; the judge prints the
  location when any test did not pass.
- The checker only runs for a run that finished within its limits; TLE / MLE / RE are reported as before.
- Checker limits are 10 s CPU and 1 GiB, only to stop a broken checker from hanging the judge.
- A checker that fails to compile fails the whole build (the solution and checker are built together,
  and the compile log says which one failed).
- Checker failures are shown as `checker failed (...)` and count as not passed (exit code 1), but are
  never labelled WA.

Not yet implemented: the stress sanity check above, and path overrides.

Limitation: checkers are Rust for now (C++ testlib checkers wait for multi-language support).

## Interactive (`A_interactor.rs`)

Judges do not publish interactors, and the samples in a `.prob` for an interactive problem may show the
conversation instead of data an interactor can read (not verified against Competitive Companion), so
the user supplies the interactor, and test data comes from `.prob` entries (treated as hidden-input
files) or the generator.

- Invocation: `interactor <input_file>`. Interactor stdin = solution stdout; interactor stdout =
  solution stdin; diagnostics on stderr.
- Exit codes (testlib): 0 OK, 1 WA, 2 PE, 3 judge failure.
- Wiring: two `std::io::pipe()` pairs (stable since Rust 1.87). Drop the `Command` objects after
  spawning — they keep the pipe ends open. Prototyped: correct solution → exit 0/0 in ~4 ms;
  solution that quits early → interactor sees EOF and exits 1; hanging solution → caught by timeout.
- **Verdict arbitration — first failure wins.** The solution's own exit code is not enough: in the
  prototype a solution that quit early exited 0 while the interactor said WA; conversely an
  interactor that rejects early can kill a well-behaved solution with a broken pipe (looks like RE).
  Record each process's exit instant: if the solution crashed / timed out / exceeded memory first,
  report that; otherwise report the interactor's verdict.
- Time limit: the solution's CPU time (the interactor's thinking time must not count), plus a
  wall-clock cap to catch two processes waiting on each other.

Implemented in phase 2 (`src/interactive.rs`):

- **Mode selection.** Interactive mode is switched on by `"interactive": true` in the `.prob`, which
  Competitive Companion sets. The interactor is then required: without `A_interactor.rs` the judge
  exits with code 2 and names the file it expected. An interactor next to a problem that is not marked
  interactive, or a checker next to an interactive one, is ignored with a printed note.
- **Test data.** Each test's `input` is written to `<project>/target/judge/N.in` and passed as the
  interactor's only argument; the stored `output` is unused. The files are kept for debugging.
- **Pipes.** The judge's own pipe ends are closed right after both processes start. Mutation-checked:
  keeping them (by holding on to the `Command`s) makes EOF never arrive, and four tests hang until the
  wall-clock cap.
- **Killing.** The solution is killed at its CPU limit, both processes at the wall-clock cap. A solution
  still running after the interactor has rejected it is killed at once instead of being left to run to
  its time limit. An interactor still running 250 ms after the solution failed is killed too, which
  keeps a stuck interactor from turning a runtime error into a timeout.
- **First failure wins**, by the order in which the two processes were reaped; the other side's failure
  is shown as a note (`afterwards the interactor reported: ...`, `afterwards the solution failed too: ...`)
  so a misjudged order is visible. A process the judge itself killed is never counted as having failed.
  The order comes from two threads each waiting on one process, so exits closer together than a
  scheduler wake-up could in principle be misordered.
- **Broken pipes blame the solution.** A solution that exits before the conversation is over leaves the
  interactor writing to a pipe nobody reads. A C or C++ interactor is then killed by SIGPIPE; a Rust one
  panics with `Broken pipe`. If the solution left first, either is reported as a wrong answer ("the
  solution stopped reading ..."), not as an interactor failure. Found by a test that failed in about 7%
  of runs. Any other interactor failure after the solution has gone gets a note instead.
- **Interactor verdicts** use the testlib mapping shared with checkers (`checker::exit_verdict`): 1 is WA,
  2 is PE, and exit 3, any other code or a crash is `interactor failed`, never a WA.
- Memory is checked on the solution only, with the same `violation` classification as normal runs.

Not implemented: a transcript of the conversation on failure (the judge never sees the data, since it
flows between the processes directly), and driving the interactive run from the generator (phase 3).

## Stress (`--stress`)

`cph_judge <dir> <name> --stress [--iters N] [--seed S] [--save]`; the existing positional form is
unchanged.

1. Generate `(seed, size)`, with `size` ramping up over the iterations.
2. Run brute force, then the solution.
3. Compare (built-in comparison, or `_checker` when present).
4. Run iterations in batches in parallel, but report the **lowest** failing iteration.
5. Exit non-zero if a counterexample is found.

Verdicts: solution WA/RE/TLE/MLE as usual. A brute-force crash/timeout or a generator failure is
reported separately ("brute failed on generated input" — invalid input or a brute bug), so a wrong
format guess does not look like a bug in the solution.

**Shrinking (v1, generator-agnostic):** after the first failure at size `s`, rerun sizes `1..s` with a
few hundred seeds each and report the smallest failing case. `--save` appends it to the `.prob`
tests as a regression test. Schema-aware shrinking (reduce lengths, then values, keeping the input
valid) is a later step.

## Generator

Two tiers:

1. **`A_gen.rs`** if present: `A_gen <seed> <size>` prints one test to stdout. The escape hatch for
   graphs, queries, and "an answer exists" guarantees.
2. **Built-in, inferred** from the `.prob` samples plus hints.

Inference (all hypotheses must hold in *every* sample):

- Tokenize each sample into lines × tokens, typed int / float / word.
- Header integers on short leading lines are candidate parameters (`n`, `m`, `t`).
- A later line group is tied to a parameter when its token count (or line count) equals `n`, `n-1`,
  etc. A repeated block counted by a leading integer is a multitest.
- Observed per-column facts: string alphabet, permutation, distinct, sorted.

Rules that keep it safe:

- Infer **conservatively**: an over-tight schema only costs coverage; a too-loose one produces
  invalid inputs and false counterexamples.
- Print the inferred schema on every run (`--show-schema` prints it in hint syntax so it can be
  copied and edited).
- Every sample must round-trip through the schema before the run starts.
- With 1–3 samples ambiguity is common; hints resolve it.

What cannot be inferred: value ranges from the statement (`a ≤ 1e9`), global constraints
(`sum n ≤ 2e5`), connectivity/other guarantees. CPH provides no constraint text.

"Adaptive" behaviour: ramp the size over the run, keep default value ranges small (collisions find
bugs), and mix in boundary values from the hints.

### Hints

An optional `"stress"` object in the `.prob` (serde already ignores unknown fields):

```json
"stress": { "multitest": true, "vars": { "t": "1..5", "n": "1..8", "a": "1..n" } }
```

Assumption to verify: that the CPH editor extension preserves unknown keys when it rewrites the
file. If it strips them, fall back to a sidecar `A.stress.json`.

## Phases

| # | Scope | Status |
|---|-------|--------|
| 0 | Extract `compile` / `run_once` / `check`; fix stdin deadlock; CPU-time limits (killed live at the limit on Linux); bounded concurrency; exit codes | **done** |
| 1 | `_checker` in normal mode (introduces the extra-binary build, temp files, exit-code mapping) | **done** |
| 2 | Interactive mode, reusing phase 1 machinery | **done** |
| 3 | Stress with `_gen` / `_brute`, ramping, size-based shrinking, `--save` | planned |
| 4 | Schema inference and built-in generator, `--show-schema` | planned |
| 5 | Schema-aware shrinking | planned |

# Design: checker, interactive problems, and stress testing

Status: **all phases done** (runner and CPU-time limits, `_checker`, interactive problems, stress testing with a hand-written or an inferred generator, `--ask-constraints`, schema-aware shrinking). What is left is in `todo.md`. See [Phases](#phases).

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

Each can be overridden by path in the hints (see [Hints](#hints)).

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

The stress sanity check is implemented in phase 3 (see below), and so are path overrides.

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
flows between the processes directly). Driving an interactive run from the generator is done in phase 3.

## Stress (`--stress`)

`cph_judge <dir> <name> --stress [--iters N] [--seed S] [--max-size N] [--save] [--ask-constraints]
[--show-schema]`; the existing positional form is unchanged.

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

Implemented in phase 3 (`src/stress.rs`, `src/cli.rs`, `src/hints.rs`):

- **Defaults.** 1000 tests, sizes `1..=20` (`iters` / `maxSize` hints, `--iters` / `--max-size`; the
  command line wins). The size of test `i` is `1 + i * max_size / iters`, so every size gets an equal
  share of the run and small sizes come first. Test `i` uses seed `base_seed + i`. Without `--seed`
  the base seed is fresh and printed ("repeat the run with --seed N"); with it a run is repeatable
  down to the report, which the end-to-end tests rely on.
- **The generator** is run as `gen <seed> <size>` and must print one test to stdout (10 s CPU, 1 GiB).
  Printing only whitespace counts as a failure.
- **The brute force** gets `max(10 × the solution's time limit, 10 s)` and at least 1 GiB.
- **Batches.** Tests run in chunks of 4 × cores, each chunk in parallel with at most one evaluation
  per core; the lowest failing index of the first chunk with a failure is reported. Progress is
  printed every 250 tests.
- **Shrinking** tries sizes `1..found_size` in turn, `shrinkSeeds` (default 200) seeds each, and
  accepts only a failure of the **same kind** (wrong answer, PE, TLE, MLE, RE), so it does not swap
  the bug for a different one. A judge-side problem at a smaller size is not a shrink. Seeds for
  shrinking are derived from the base seed so shrinking is repeatable too (an early version mapped
  seeds `k` and `k^1` to the same value, which a unit test caught).
- **Oracles.** Ordinary problem: the brute force's output is compared with the built-in comparison,
  or, if there is a checker, handed to it as the jury answer after `checker(input, brute, brute)` has
  been run to make sure the checker accepts it. Interactive problem: the interactor decides, no brute
  force is involved (and a `brute` file is ignored).
- **Setup problems are not counterexamples** and exit with code 2: the generator crashing, timing out
  or printing nothing; the brute force failing on a generated input (shown, with a hint that the input
  may be invalid or the size too big for it); the checker rejecting the brute force's own answer; the
  checker or interactor failing. A solution that cannot be started is reported but not shrunk.
- **Exit codes:** 0 no counterexample, 1 a counterexample, 2 a setup problem or usage error.
- **Output.** The counterexample is shown with the input (long ones are cut to 40 lines / 2000
  characters), what the brute force and the solution printed or the checker / interactor said, and the
  command that regenerates the input. The full files are kept in
  `<project>/target/judge/stress/counterexample.in` (and `.out` / `.ans`).
- **`--save`** appends `{id, input, output}` to the `.prob`'s `tests`, where `output` is the brute
  force's answer (empty for interactive problems) and `id` is one above the highest existing id (or a
  timestamp). Everything else in the file is kept, but it is rewritten as compact JSON with the keys in
  alphabetical order, replacing the file atomically. That `id` is a number is an assumption about the
  CPH editor, not checked here.

## Generator

Two tiers:

1. **`A_gen.rs`** if present (or named by the `gen` hint): `A_gen <seed> <size>` prints one test to
   stdout. The escape hatch for graphs, trees, queries, and "an answer exists" guarantees.
2. **Built-in, inferred** from the `.prob` samples plus hints (phase 4). Used when there is no generator
   file; it runs inside the judge, with no process per test.

### The format language (`src/schema.rs`)

Inference, the `format` hint and `--show-schema` all use one small text language, so what is printed
can be pasted back and edited:

```text
t                    a line of integers, each named
repeat t {           the lines inside are repeated `t` times
  n m
  a[n]               one line of n integers
  str s[m]           one line holding a string of m characters
  repeat m {         m lines ...
    u v              ... each with two integers
  }
}
```

A grid is `repeat n { str s[m] }`, a matrix `repeat n { a[m] }`, a list of edges `repeat m { u v }`, and a
multitest `t` followed by `repeat t { ... }`: everything is one of four items (a line of numbers, an array,
a string, a repeated block). Lengths and counts are expressions over earlier numbers and `size`
(`n`, `n-1`, `2*n`, `10^5`, `1e5`). Names are unique across the whole format, so a constraint can name a
variable without ambiguity.

Expressions also divide (`n*(n-1)/2`, rounding towards zero).

### Trees, graphs and order (`src/graph.rs`)

Three relations between columns, added after the first version of the language:

- **A list of edges.** A `repeat` block whose body is one line of two numbers (`repeat m { u v }`, or
  `repeat n-1 { u v }`) can carry one constraint on the *pair*, keyed by both names (`"u v": "tree"`): `tree`,
  `graph`, `graph, connected`, `graph, directed`, with `ordered` (smaller vertex first) on any but `directed`.
  The vertices are the range of the first column. The generator makes the whole list at once: a tree is built
  from a random labelled tree whose shape varies (random parents, a path, a star, the last few vertices), a graph
  by drawing distinct pairs or, when most pairs are wanted, shuffling the unused ones; a connected graph starts
  from a tree. A request that cannot be met (more edges than pairs, a tree with the wrong number of edges) is a
  setup problem with the numbers in it.
- **The number of edges** is an ordinary variable with an ordinary range, so it is *set to match* when a kind of
  list is chosen: `n-1..n-1` for a tree, `0..n*(n-1)/2` for a graph, `n-1..n*(n-1)/2` if connected,
  `0..n*(n-1)` if directed. The generator's size limit then makes the edge count grow with the size.
- **`l <= r`** is not a new constraint: it is `r: l..n`, the lower bound of a column being the column before it
  in the same line.

*Inference.* A block of rows may be counted by `n-1` as well as by `n`, only when the rows are two or more
separate numbers (for arrays it let a matrix read as a row plus `n-1` more). With the vertices bounded by one
number in every sample, a list is a tree if every one is a tree, else a connected graph (only if some sample
has four or more vertices, so connectivity is not luck), else a simple graph, else a directed one; at least
three edges in total are needed, and `ordered` is added if every edge is written smaller vertex first. A list
that has a loop or a repeated edge in any sample gets no relation. Two columns of other rows, never decreasing in
at least three rows, get `l..hi`.

*The prompt* asks after the variables: for each list of edges (`tree`, `graph` with flags, or `none`, with `?` for
the syntax), and for each pair of columns of other rows (`l ≤ r in every row?`). Choosing a kind of list also sets
the number of edges and says so; answering no to an order goes back to the lower bound of the row's first column.

*Reduction* gains one edit: remove a vertex. It gets the highest label (swapping with it), the edges that touch it
go, and the number of vertices and of edges are lowered. For a tree that is only valid for a leaf, which the
validity check enforces, so trees shrink leaf by leaf; dropping one edge or lowering `n` alone cannot.

Each variable has one constraint, in the syntax of the `vars` hint and of the `--ask-constraints` prompt:
`lo..hi` (bounds are expressions), `lo..hi, distinct`, `lo..hi, sorted`, `perm`, `chars a-z`.

### How the format is inferred (`src/infer.rs`)

All samples are read at once, line by line. Each line is read as the first possibility that fits **every**
sample: an array whose length is an earlier number (or one less), a block of rows counted by an earlier
number that nothing has used yet, a string, or a line of new numbers. When a choice leads to a dead end
later, the search goes back and tries the next one (a matrix needs this once the samples have different
row counts). A multitest (first number `t`, then `t` blocks) is tried too; when both readings fit, the one
that leaves fewer numbers unused, then fewer guessed lengths, then fewer lines, wins.

Defaults for the constraints stay inside what the samples show:

- A value that never exceeds some earlier number in any sample is bounded by it (`a: 1..n`): an index into
  a list of n things must not exceed n, and that number is then a size variable too. The vertices of an edge
  list are bounded by the vertex count this way.
- Other values get the range they were seen in, widened to start at 1 (or 0, or at -10 when negatives
  occur) and to reach at least 10.
- Lengths and counts default to `1..size`; the number of tests `t` to `1..5`.
- Properties shared by every sample are kept if they were shown by enough elements: `perm` for three or
  more, `distinct` and `sorted` for four or more.
- Strings use the characters that were seen.

### Reducing the counterexample (`src/reduce.rs`, phase 5)

Searching smaller sizes finds a small test, not a clean one: its numbers are whatever the generator drew
(`3` / `6 5 6`). With the format and constraints known, the failing input is parsed into a tree of values and
reduced one edit at a time, each edit kept only if the result is still a valid input and the solution still
fails **in the same way** (the same kind check as shrinking by size):

- drop one repetition of a block counted by a number (any one, not only the last: the failing test of a
  multitest can be the third), lowering that number to match;
- lower a number to its constraint's lower bound, half way, or by one;
- set a whole array to its lower bound, then single elements to it or half way;
- set a character of a string to the first character of its alphabet.

After each edit lengths and counts are evaluated again and arrays, strings and repetitions cut to them, values
are pulled back into their constraints, and a permutation that was shortened keeps only the values that still
fit. A candidate must then read back under the format with no constraint broken, so `distinct`, `sorted` and
`perm` survive every step. Candidates are tried in parallel chunks and the lowest one that still fails is taken
(so the result is repeatable); this repeats until nothing simpler fails or 5000 candidates have been tried.
The result is the least input that fails: for the sum example, `3` and `1 1 1`; with `a: 5..9`, `3` and `5 5 5`.

It applies only to the inferred generator, which is what supplies the format and the constraints that make
"still valid" decidable; the input of a hand-written generator is shown as it made it. `--save` saves the
reduced input.

### Staying safe

- **Refuse instead of guessing.** Non-integer numbers, blank lines inside the input, lines mixing numbers
  and words, several words on one line, and samples that do not follow one format are all rejected, with
  the line and a pointer to writing `A_gen.rs`.
- **Say what was guessed.** The format and constraints are printed on every run, with a note for each
  ambiguity that was resolved (`its length could be n or m; chose n`; `these lines could also be rows`;
  `could also be a multitest`), a warning for every place a sample breaks the final constraints, and a
  reminder that the constraints are guesses.
- **Offsets are limited.** Only `n` and `n-1` are tried as lengths. `n+1` was tried at first and removed:
  with small samples it explains nearly anything, which made a multitest input parse as five unrelated arrays.
- **Constraints that cannot be met** (`8` different values from `1..3`) are a setup problem, never a
  counterexample, and the message points at `--ask-constraints`.
- Samples cannot show what the problem *allows*, only what *occurs*. "An answer exists", `sum of n <= ...`,
  bipartite or planar graphs, trees given as parent arrays, and relations beyond those below cannot be
  expressed; use `A_gen.rs` for those.

### Size

The size of a test works through **size variables**: a variable that sets a length or count, or that another
variable's constraint uses as a bound. While generating a test of size `s` such a variable is limited to the
first `s` values of its range (counting from at least 1), so `n: 1..8` still gives small tests first and `t`
grows with the size too. Other values do not depend on the size. An empty array leaves no line at all.

### `--ask-constraints` and `--show-schema`

`--ask-constraints` (with `--stress`, for the inferred generator) shows the inferred format, then asks about each
variable in turn: the prompt shows the current constraint, Enter keeps it, `?` explains the syntax and what
the constraint may use, and a wrong answer is explained and asked again. If anything changed, the
constraints are shown again with any samples they contradict, and the judge offers to save the format and the
constraints in `<solution>.stress.json`, so later runs use them without typing (the format is saved only if it
was inferred, not if it came from a hint). A closed input keeps what is left unchanged, so the flag never
hangs a script. It is an error when a generator file is in use.

`--show-schema` prints the format and constraints as a hints object ready to paste, and stops without
compiling or running anything.

### Hints

An optional `"stress"` object in the `.prob`, or the same object in a sidecar `A.stress.json` next to
the solution (the sidecar wins field by field; `vars` merge by name). Unknown fields are ignored.

```json
"stress": {
  "brute": "tools/slow.rs", "gen": "tools/make.rs", "checker": "my_checker.rs", "interactor": "talk.rs",
  "iters": 500, "maxSize": 12, "shrinkSeeds": 100,
  "format": ["t", "repeat t {", "  n", "  a[n]", "}"],
  "vars": { "t": "1..3", "n": "1..8", "a": "-5..5, sorted" },
  "multitest": true
}
```

- File hints are paths, relative to the solution's directory, and win over the `_suffix` convention.
  A hint naming a file that does not exist is an error, not a fallback.
- `checker` and `interactor` also apply to ordinary judging; the rest are only read with `--stress`.
- `format` (a list of lines, or one string with newlines) replaces inference; the samples are then checked
  against it and an error says where one does not fit. `vars` replaces single constraints. `multitest` decides
  between the readings when the samples cannot.

Whether the CPH editor keeps unknown keys when it rewrites the `.prob` is not verified; the sidecar
exists so the hints survive if it does not.

## Phases

| # | Scope | Status |
|---|-------|--------|
| 0 | Extract `compile` / `run_once` / `check`; fix stdin deadlock; CPU-time limits (killed live at the limit on Linux); bounded concurrency; exit codes | **done** |
| 1 | `_checker` in normal mode (introduces the extra-binary build, temp files, exit-code mapping) | **done** |
| 2 | Interactive mode, reusing phase 1 machinery | **done** |
| 3 | Stress with `_gen` / `_brute`, ramping, size-based shrinking, `--save`, hints and sidecar | **done** |
| 4 | Schema inference and built-in generator, `--show-schema`, `--ask-constraints` | **done** |
| 5 | Schema-aware shrinking | **done** |

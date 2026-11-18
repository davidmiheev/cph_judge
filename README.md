# cph_judge

A fast, concurrent competitive programming judge written in Rust, designed to work seamlessly with Competitive Companion / CPH `.prob` files.

---

## Features

- **Automatic Problem Discovery**: Recursively finds `.prob` configuration files matching problem names using `ignore::WalkBuilder`.
- **Parallel Test Execution**: Runs test cases concurrently with `tokio` async workers (at most one per CPU core).
- **Resource Limit Enforcement**:
  - **Time Limits (TLE)**: Judged on CPU time and enforced live (Linux), with a wall-clock cap and process-tree cleanup for hung processes.
  - **Memory Limits (MLE)**: Peak resident set size tracking using `wait4` / `rusage`.
  - **Runtime Errors (RE)**: Robust exit code and signal failure detection.
- **Stress Testing**: `--stress` hunts for a counterexample with a brute force (or interactor) and a generator, shrinks it, and can save it as a test. With no generator file, tests are made from the input format inferred from the samples, and `--ask-constraints` lets you type the constraints.
- **Interactive Problems**: Put a `<solution>_interactor.rs` next to your solution; the judge connects the two with pipes and takes the verdict from the interactor.
- **Custom Checkers**: Drop a `<solution>_checker.rs` next to your solution to judge problems with several valid answers (testlib-style protocol).
- **Tokenized & Float-Aware Comparison**: Token-by-token comparison with automatic floating-point epsilon tolerance ($\epsilon = 10^{-6}$) and whitespace insensitivity.
- **Colored Visual Feedback**: Clean, colored terminal output with execution time and memory stats.

---

## Installation & Build

```bash
# Clone the repository
git clone git@github.com:davidmiheev/cph_judge.git
cd cph_judge

# Build release binary
cargo build --release

# Run unit tests
cargo test
```

---

## Usage

```bash
cargo run --release -- <search_dir> <problem_name> [--stress [--iters N] [--seed S] [--max-size N] [--save] [--ask-constraints] [--show-schema]]
```

### Example

```bash
cargo run --release -- ~/cp/problemset 1840A
```

The process exit code is `0` when every test passes, `1` when compilation fails or a test does not pass, and `2` on a usage error or when the judge itself cannot run.

### Custom checkers

For problems where many outputs are valid, put a checker next to the solution, named after it with a
`_checker` suffix: `A.rs` is checked by `A_checker.rs`. The judge builds it together with the solution
(through the same `Cargo.toml`, so edition, profile and dependencies match) and, for every test whose
program finished within its limits, runs:

```
checker <input_file> <output_file> <answer_file>
```

`<answer_file>` holds the expected output stored in the `.prob`. The checker reports through its exit
code and may explain on stderr:

| Exit code | Verdict |
|---|---|
| 0 | AC |
| 1 | WA (the checker's message is shown) |
| 2 | PE |
| 3, any other code, crash or timeout | `checker failed`: counted as not passed, never reported as WA |

The three files for test N are kept in `<project>/target/judge/N.in`, `N.out` and `N.ans`, so a failing test
can be re-checked by hand. Checkers are written in Rust for now. With `autobins = false` in `Cargo.toml` the
judge cannot build a checker and says so.

### Interactive problems

For a problem marked `"interactive": true` in its `.prob` file (Competitive Companion sets this), write the
judge's side of the conversation as an interactor next to the solution: `A.rs` is judged against
`A_interactor.rs`. It is built with the solution and started once per test as:

```
interactor <input_file>
```

Its stdin is the solution's stdout and its stdout is the solution's stdin. `<input_file>` holds the test's
`input` from the `.prob`, so put what the interactor needs there (a hidden number, a graph); the stored
`output` is not used. The samples the `.prob` starts with may show the conversation rather than data
your interactor can read, so check them and edit the tests' inputs to match your interactor. The
interactor reports through its exit code, with a message on stderr:

| Exit code | Verdict |
|---|---|
| 0 | AC |
| 1 | WA (the interactor's message is shown) |
| 2 | PE |
| 3, any other code or a crash | `interactor failed`: counted as not passed, never reported as WA |

The solution's time and memory limits apply to the solution alone. If both programs fail, the one that
failed first gets the blame and the other side's failure is shown as a note; a solution that stops reading
too early gets a wrong answer rather than blaming the interactor. If the problem is marked interactive but
there is no interactor, the judge stops and says which file it expected. A checker is ignored for
interactive problems, and an interactor is ignored for problems that are not marked interactive. Like
checkers, interactors are written in Rust for now and cannot be built if `Cargo.toml` sets `autobins = false`.

The interactor's input file for test N is kept in `<project>/target/judge/N.in`.

### Stress testing

To look for an input on which your solution is wrong, add a brute-force solution next to it: `A.rs` is
stressed with `A_brute.rs` (slow but obviously right). Tests come from `A_gen.rs` if you write one, and are
otherwise [inferred from the samples](#generating-tests-from-the-samples). Then:

```bash
cargo run --release -- <search_dir> <problem_name> --stress
```

If you write a generator, each test is made by running `A_gen <seed> <size>`, which must print one test to
stdout and make it larger as `size` grows. The judge runs the brute force and the solution on it and compares the outputs (or hands them to
your `_checker`, if you have one, which is the right choice when several answers are valid). Sizes grow from 1
up to `--max-size`, so small inputs, which make the clearest counterexamples, are tried first.

When a test fails, the judge searches smaller sizes, with many seeds each, for a failure of the same kind and
shows the smallest one it finds (with the inferred generator it then also simplifies the values, see below): the input, what the brute force and your solution printed, and the command that
regenerates the input. For an interactive problem the interactor is the oracle and no brute force is needed.

| Option | Meaning |
|---|---|
| `--iters N` | number of tests (default 1000) |
| `--seed S` | seed of the first test; the same seed repeats the run exactly (a fresh seed is printed otherwise) |
| `--max-size N` | largest size given to the generator (default 20) |
| `--save` | append the counterexample to the `.prob` as a new test, so the normal judge runs it from then on |
| `--ask-constraints` | type the constraints of the generated input at a prompt (inferred generator only) |
| `--show-schema` | print the inferred input format and constraints as hints, and stop (inferred generator only) |

The exit code is `0` if no counterexample was found, `1` if one was, and `2` if something in the setup is wrong. A
generator that crashes or prints nothing, a brute force that fails on a generated input (the input may be invalid,
or the size too big for the brute force), and a checker that rejects the brute force's own answer are reported as
problems with the setup, never as counterexamples. The counterexample is also kept in
`<project>/target/judge/stress/counterexample.in` (and `.out` / `.ans`).

`--save` rewrites the `.prob` as compact JSON with its keys in alphabetical order; nothing else about it changes.

#### Generating tests from the samples

With no `A_gen.rs`, the judge reads the samples in the `.prob` and works out the shape of the input: lines of
numbers, arrays, strings, and repeated blocks (a list of edges, a grid, a matrix, a number of tests followed by
that many cases). It prints what it found, and what it assumed, on every run:

```
🧬 Input format (inferred from 2 sample(s)), used to generate the tests:
  n
  a[n]
  constraints:
    n: 1..size
    a: 1..10
   The constraints are guesses from the samples. Set the real ones with --ask-constraints or the `vars` hint.
```

Samples show what *occurs*, never what the problem *allows*, so the shape is reliable but the constraints are
guesses: they stay inside what the samples show, values that never exceed some number (`n`) are bounded by it,
and a number that sets a length or count grows with the size of the test (`1..size`), so small tests come
first. If the samples fit more than one shape, the choice made is reported, and more samples settle it. Things
it cannot understand (non-integer numbers, blank lines inside the input, lines mixing numbers and words) are
refused with the line number, and so are samples that do not follow one format.

**Reducing the counterexample.** The input of a small failing test still holds whatever numbers the generator drew
(`3` / `6 5 6`). With the format and constraints known, the judge then simplifies it one step at a time (drops a
repetition, lowers a number, sets an array or a character to its simplest value) and keeps a step only if the
input stays valid and the solution still fails in the same way, so you get the least input that fails (`3` /
`1 1 1`), always within the constraints. This runs only for the inferred generator; the input of an `A_gen.rs` is
shown as it made it.

**Setting the constraints.** `--ask-constraints` asks about each variable in turn, showing the current constraint;
Enter keeps it, `?` explains the syntax, and a wrong answer is explained and asked again:

```
Constraints of the generated input (Enter keeps the value in brackets, `?` shows the syntax):
  n (number) [1..size]: 1..8
  a (array) [1..10]: -5..5, sorted
Save the format and constraints in .../A.stress.json so later runs use them? [y/N] y
```

| Constraint | Meaning |
|---|---|
| `lo..hi` | an integer from `lo` to `hi`; the bounds may use earlier numbers and `size`: `1..n`, `0..n-1`, `1..10^9`, `1e5` |
| `lo..hi, distinct` | an array whose elements are all different (`sorted` too: non-decreasing) |
| `perm` | an array that is a permutation of `1..length` |
| `chars a-z` | a string over these characters (`chars 01`, `chars .#`) |
| `tree` | for a list of edges (the rows `u v` of a repeated block): the edges of a tree on the vertices, `n-1` of them, all connected |
| `graph` | ... a simple graph: no loops, no repeated edges; add `, connected`, `, directed` (an edge and its reverse differ) |
| `..., ordered` | ... every edge lists its smaller vertex first |

A constraint on a list of edges is written under both column names, `"u v": "tree"` in the hints. Bounds may also
divide, so the number of edges of a graph can be `0..n*(n-1)/2`. `l ≤ r` is just a range that starts at the
earlier column: `r: l..n`.

**Trees, graphs and ranges.** From the samples the judge recognises a list of edges that is a tree (a block of `n-1`
rows), a simple graph, a connected graph, a directed graph, and edges always written smaller vertex first, but only
when *every* sample is one and there are enough edges to mean something. A second column that is never below the
first in any row (at least three rows) gets a range that starts at the first. The number of edges is then set to
match (`n-1..n-1` for a tree, `0..n*(n-1)/2` for a graph), and the tests it generates are trees of different
shapes (random, paths, stars) or graphs of that kind. A graph that cannot exist (five edges on three vertices) is a
problem with the setup, not a counterexample. A counterexample reduces by removing whole vertices: a leaf of a
tree, or a vertex and its edges.

With `--ask-constraints`, after the variables the prompt asks about each list of edges and each pair of columns:

```
Relations between columns (Enter keeps the value in brackets, `?` shows the syntax):
  edges u v [graph, ordered]: tree
    → `m` is now n-1..n-1
  l ≤ r in every row? [y]:
```

Saving writes the format and the constraints to `A.stress.json`, so the next run needs no typing. The same thing
can be written by hand in the `vars` (constraints by name), `format` (the input's shape, one line per entry) and
`multitest` (whether the first number counts the tests) hints; `--show-schema` prints them ready to paste.

What the constraint language cannot say: that a graph is bipartite or planar, a tree given as a list of parents, a
weight on each edge that depends on the edge, "an answer exists", a limit on the sum of the lengths. For those, write
`A_gen.rs`. Constraints that allow no input (eight different
numbers from `1..3`) are reported as a problem with the setup, not as a counterexample.

#### Hints

File names and defaults can be set in a `"stress"` object in the `.prob` file, or in a `A.stress.json` next to the
solution, which wins where both set a field. Relative paths are resolved against the solution's directory, and a
file that is named but missing is an error.

```json
"stress": { "brute": "tools/slow.rs", "gen": "tools/make.rs", "checker": "my_checker.rs",
            "interactor": "talk.rs", "iters": 500, "maxSize": 12, "shrinkSeeds": 100,
            "format": ["n", "a[n]"], "vars": { "n": "1..8", "a": "-5..5" }, "multitest": false }
```

`checker` and `interactor` apply to ordinary judging too. Command-line options win over hints. Whether the CPH
editor keeps unknown keys when it rewrites the `.prob` has not been checked, which is what the sidecar file is for.

---

## Verdicts

| Icon | Verdict | Description |
|---|---|---|
| ✅ | **AC** | Accepted (Output matches expected within tolerance) |
| ❌ | **WA** | Wrong Answer (Output token mismatch) |
| ⏱️ | **TLE** | Time Limit Exceeded (Killed after exceeding `timeLimit`) |
| 💾 | **MLE** | Memory Limit Exceeded (Exceeded `memoryLimit`) |
| 💥 | **RE** | Runtime Error (Panic, crash, or non-zero exit) |

---

## Documentation

- **[Architecture](docs/arch.md)**: Detailed system design, execution pipeline, and comparison engine.
- **[Checker, Interactive & Stress Design](docs/design-stress-interactive.md)**: Design of `_checker`, interactive problems, and stress testing, and the plan for the inferred generator.
- **[Roadmap & Backlog](docs/todo.md)**: Current development state and upcoming features.
- **[Code Analysis](docs/analysis.md)**: Breakdown of components and evolution.

---

## License

MIT License. See [LICENSE](LICENSE) for details.

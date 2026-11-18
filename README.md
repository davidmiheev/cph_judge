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
- **Stress Testing**: `--stress` hunts for a counterexample with your own generator and brute force (or interactor), shrinks it, and can save it as a test.
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
cargo run --release -- <search_dir> <problem_name> [--stress [--iters N] [--seed S] [--max-size N] [--save]]
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

To look for an input on which your solution is wrong, add a brute-force solution and a generator next to it:
`A.rs` is stressed with `A_brute.rs` (slow but obviously right) and `A_gen.rs`. Then:

```bash
cargo run --release -- <search_dir> <problem_name> --stress
```

Each test is made by running `A_gen <seed> <size>`, which must print one test to stdout and make it larger as
`size` grows. The judge runs the brute force and the solution on it and compares the outputs (or hands them to
your `_checker`, if you have one, which is the right choice when several answers are valid). Sizes grow from 1
up to `--max-size`, so small inputs, which make the clearest counterexamples, are tried first.

When a test fails, the judge searches smaller sizes, with many seeds each, for a failure of the same kind and
shows the smallest one it finds: the input, what the brute force and your solution printed, and the command that
regenerates the input. For an interactive problem the interactor is the oracle and no brute force is needed.

| Option | Meaning |
|---|---|
| `--iters N` | number of tests (default 1000) |
| `--seed S` | seed of the first test; the same seed repeats the run exactly (a fresh seed is printed otherwise) |
| `--max-size N` | largest size given to the generator (default 20) |
| `--save` | append the counterexample to the `.prob` as a new test, so the normal judge runs it from then on |

The exit code is `0` if no counterexample was found, `1` if one was, and `2` if something in the setup is wrong. A
generator that crashes or prints nothing, a brute force that fails on a generated input (the input may be invalid,
or the size too big for the brute force), and a checker that rejects the brute force's own answer are reported as
problems with the setup, never as counterexamples. The counterexample is also kept in
`<project>/target/judge/stress/counterexample.in` (and `.out` / `.ans`).

`--save` rewrites the `.prob` as compact JSON with its keys in alphabetical order; nothing else about it changes.

#### Hints

File names and defaults can be set in a `"stress"` object in the `.prob` file, or in a `A.stress.json` next to the
solution, which wins where both set a field. Relative paths are resolved against the solution's directory, and a
file that is named but missing is an error.

```json
"stress": { "brute": "tools/slow.rs", "gen": "tools/make.rs", "checker": "my_checker.rs",
            "interactor": "talk.rs", "iters": 500, "maxSize": 12, "shrinkSeeds": 100 }
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

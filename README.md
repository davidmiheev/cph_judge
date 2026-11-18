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
cargo run --release -- <search_dir> <problem_name>
```

### Example

```bash
cargo run --release -- ~/cp/problemset 1840A
```

---

The process exit code is `0` when every test passes, `1` when compilation fails or a test does not pass, and `2` on a usage error or when the judge itself cannot run.

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
- **[Checker, Interactive & Stress Design](docs/design-stress-interactive.md)**: Plan for `_checker`, interactive problems, and stress testing.
- **[Roadmap & Backlog](docs/todo.md)**: Current development state and upcoming features.
- **[Code Analysis](docs/analysis.md)**: Breakdown of components and evolution.

---

## License

MIT License. See [LICENSE](LICENSE) for details.

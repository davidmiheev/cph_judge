# Roadmap & TODO: `cph_judge`

Current state and planned improvements for `cph_judge`.

---

## High Priority

- [ ] **Safe File Copy / Backup**: Before copying `src_path` to `src/main.rs`, create a backup of any existing `src/main.rs` and restore it after judge execution or on signal/panic.
- [ ] **Interactive Problems Support**: Implement two-way process piping and interactor communication when `interactive: true`.
- [ ] **Configurable Floating-Point Tolerance**: Allow per-problem or per-test configuration of $\epsilon$ tolerance (e.g., $10^{-9}$ for high precision problems).

---

## Features & Improvements

- [ ] **Special Judge / Custom Checker**: Support `testlib`-style custom checker scripts for problems with multiple valid outputs.
- [ ] **Multi-Language Support**: Extend beyond Rust to support compiling and executing C++, Python, Java, and Go solutions.
- [ ] **Diff Highlighting for WA**: Improve Wrong Answer visualization with colored diffs for multiline expected vs received mismatches.
- [ ] **Strict Memory Enforcement**: Enforce memory limits in real-time (e.g. via `setrlimit` or Linux cgroups) rather than post-execution `rusage`.
- [ ] **Clean Exit Code on Compilation Failure**: Ensure the judge returns a non-zero exit code (`std::process::exit(1)`) on compilation failures to integrate cleanly with CI/scripts.

//! Custom answer checkers (`<solution>_checker.rs`), testlib-style.
//!
//! A checker is invoked as `checker <input_file> <contestant_output_file> <jury_answer_file>`
//! and reports through its exit code, with an optional message on stderr:
//!
//! | Exit | Meaning                                       |
//! |------|-----------------------------------------------|
//! | 0    | OK                                            |
//! | 1    | Wrong answer                                  |
//! | 2    | Presentation error                            |
//! | 3    | The judge side failed (bad checker or data)   |
//!
//! Anything else (another exit code, a crash, a timeout) is also a checker failure,
//! reported separately so it is never mistaken for a wrong answer by the solution.

use std::{os::unix::process::ExitStatusExt, path::Path, process::ExitStatus, time::Duration};

use crate::runner::{Limits, RunError, RunReport, run_once};

/// Checkers are expected to be fast; this only stops a broken one from hanging the judge.
const TIME_LIMIT: Duration = Duration::from_secs(10);
const MEMORY_BYTES: u64 = 1 << 30;
/// Longest checker message kept, in characters.
const MAX_MESSAGE_CHARS: usize = 2000;

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Wrong(String),
    Presentation(String),
    /// The checker itself failed; says nothing about the solution.
    Failed(String),
}

/// Runs the checker binary on one test. The three files must already exist.
pub async fn run_checker(bin: &Path, cwd: &Path, input: &Path, output: &Path, answer: &Path) -> Verdict {
    let limits = Limits { time: TIME_LIMIT, memory_bytes: MEMORY_BYTES };
    match run_once(bin, [input, output, answer], cwd, b"", &limits).await {
        Ok(report) => interpret(&report),
        Err(RunError::Tle) => Verdict::Failed(format!("checker timed out after {}s", TIME_LIMIT.as_secs())),
        Err(RunError::System(e)) => Verdict::Failed(format!("could not run the checker: {e}")),
        Err(other) => Verdict::Failed(format!("checker failed: {other:?}")),
    }
}

/// Maps a finished checker process to a verdict.
pub fn interpret(report: &RunReport) -> Verdict {
    if report.cpu_time > TIME_LIMIT {
        return Verdict::Failed(format!("checker timed out after {}s", TIME_LIMIT.as_secs()));
    }
    exit_verdict("checker", report.status, &report.stderr)
}

/// The testlib exit-code convention, shared by checkers and interactors: 0 OK, 1 WA,
/// 2 PE, 3 judge failure; anything else (another code, a crash) is also a failure of
/// the program `who` names, never a verdict about the solution.
pub fn exit_verdict(who: &str, status: ExitStatus, stderr: &[u8]) -> Verdict {
    let message = message(stderr);
    match status.code() {
        Some(0) => Verdict::Ok,
        Some(1) => Verdict::Wrong(message),
        Some(2) => Verdict::Presentation(message),
        Some(3) => Verdict::Failed(with_message(&format!("{who} reported a judge-side failure"), &message)),
        Some(code) => Verdict::Failed(with_message(&format!("{who} exited with unexpected code {code}"), &message)),
        None => {
            let how = match status.signal() {
                Some(sig) => format!("{who} was killed by signal {sig}"),
                None => format!("{who} terminated unexpectedly"),
            };
            Verdict::Failed(with_message(&how, &message))
        }
    }
}

fn with_message(prefix: &str, message: &str) -> String {
    if message.is_empty() { prefix.to_string() } else { format!("{prefix}: {message}") }
}

/// The checker's stderr as a trimmed, length-capped message.
fn message(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.chars().count() <= MAX_MESSAGE_CHARS {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(MAX_MESSAGE_CHARS).collect();
        cut.push('…');
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(status_raw: i32, stderr: &str, cpu: Duration) -> RunReport {
        RunReport {
            status: ExitStatus::from_raw(status_raw),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            cpu_time: cpu,
            max_rss: 0,
        }
    }

    fn exited(code: i32, stderr: &str) -> Verdict {
        interpret(&report(code << 8, stderr, Duration::ZERO))
    }

    #[test]
    fn exit_0_is_ok_whatever_the_message() {
        assert_eq!(exited(0, ""), Verdict::Ok);
        assert_eq!(exited(0, "ok 3 numbers"), Verdict::Ok);
    }

    #[test]
    fn exit_1_is_wrong_answer_with_the_message() {
        assert_eq!(exited(1, "1 + 1000 != 5\n"), Verdict::Wrong("1 + 1000 != 5".into()));
        assert_eq!(exited(1, ""), Verdict::Wrong(String::new()));
    }

    #[test]
    fn exit_2_is_presentation_error() {
        assert_eq!(exited(2, "expected 2 numbers"), Verdict::Presentation("expected 2 numbers".into()));
    }

    #[test]
    fn exit_3_is_a_checker_failure_not_a_wrong_answer() {
        match exited(3, "jury answer is malformed") {
            Verdict::Failed(m) => assert!(m.contains("jury answer is malformed"), "{m}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn unknown_exit_codes_are_checker_failures() {
        for code in [4, 7, 42, 101] {
            match exited(code, "") {
                Verdict::Failed(m) => assert!(m.contains(&code.to_string()), "{m}"),
                other => panic!("exit {code}: expected Failed, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_crashing_checker_is_a_checker_failure() {
        // Raw wait status 11 = killed by SIGSEGV.
        match interpret(&report(11, "", Duration::ZERO)) {
            Verdict::Failed(m) => assert!(m.contains("signal 11"), "{m}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn a_checker_that_ran_too_long_is_a_checker_failure() {
        // Even an exit-0 checker is not trusted if it overran its own limit.
        match interpret(&report(0, "", TIME_LIMIT + Duration::from_millis(1))) {
            Verdict::Failed(m) => assert!(m.contains("timed out"), "{m}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn exit_verdict_names_the_program_that_failed() {
        let status = ExitStatus::from_raw(42 << 8);
        match exit_verdict("interactor", status, b"") {
            Verdict::Failed(m) => assert!(m.starts_with("interactor exited"), "{m}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn long_messages_are_truncated() {
        let long = "x".repeat(MAX_MESSAGE_CHARS * 3);
        match exited(1, &long) {
            Verdict::Wrong(m) => {
                assert_eq!(m.chars().count(), MAX_MESSAGE_CHARS + 1);
                assert!(m.ends_with('…'));
            }
            other => panic!("expected Wrong, got {other:?}"),
        }
    }

    #[test]
    fn messages_survive_invalid_utf8() {
        let verdict = interpret(&RunReport {
            status: ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: vec![b'o', b'k', 0xff, 0xfe],
            cpu_time: Duration::ZERO,
            max_rss: 0,
        });
        assert!(matches!(verdict, Verdict::Wrong(m) if m.starts_with("ok")));
    }
}

//! Interactive problems: the solution and an interactor run connected back to back.
//!
//! ```text
//!   interactor stdout ──▶ solution stdin
//!   interactor stdin  ◀── solution stdout
//! ```
//!
//! The interactor is a program the user supplies (`<solution>_interactor.rs`). It is
//! started as `interactor <input_file>`, plays the judge's side of the conversation, and
//! reports through the testlib exit-code convention (see [`crate::checker::exit_verdict`]).
//!
//! Two steps, as in [`crate::runner`]: [`run_interaction`] runs both processes and
//! reports what each did; [`decide`] turns that into a verdict. [`interact`] does both.
//!
//! # Who gets the blame
//!
//! When the conversation breaks down, both sides usually end up failing, and the second
//! failure is a consequence of the first: a solution that crashes closes its pipes, so the
//! interactor reads EOF and rejects it; an interactor that rejects the solution closes
//! its pipes, so a well-behaved solution dies of a broken pipe. So **the first failure
//! wins**, decided by when each process was reaped, and the other side's failure is
//! reported as a note instead of as the verdict.
//!
//! Exit order is observed from two threads that each wait on one process, so two exits
//! within a scheduler's wake-up latency of each other could be misordered. The note keeps
//! such a mistake visible instead of hiding the other side's story.

use std::{
    ffi::OsString,
    io::{self, Read},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use wait4::Wait4;

use crate::checker::{self, Verdict as ExitVerdict};
use crate::runner::{Limits, POLL_INTERVAL, RunError, kill_process_tree, process_cpu_time, violation};

/// How long the interactor may keep running after the solution has failed. It normally
/// notices the closed pipes and exits at once, and what it says is shown as a note.
const INTERACTOR_GRACE: Duration = Duration::from_millis(250);

/// An executable and its arguments.
pub struct Program {
    pub path: PathBuf,
    pub args: Vec<OsString>,
}

impl Program {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Program { path: path.into(), args: Vec::new() }
    }

    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }
}

/// What one of the two processes did.
pub struct ProcessReport {
    pub status: ExitStatus,
    pub stderr: Vec<u8>,
    /// User + system CPU time.
    pub cpu_time: Duration,
    /// Peak resident set size in bytes.
    pub max_rss: u64,
    /// Time from the start of the run until the process was reaped.
    pub exited_after: Duration,
    /// The judge killed this process because the other side had already failed, so its
    /// own exit status says nothing about it.
    pub killed_by_judge: bool,
}

pub struct InteractionReport {
    pub solution: ProcessReport,
    pub interactor: ProcessReport,
}

#[derive(Debug)]
pub enum InteractionError {
    /// The solution used more CPU time than allowed, or the run hit the wall-clock cap.
    TimedOut { by_wall_cap: bool },
    System(String),
}

/// How an interaction ended, as the judge reports it.
#[derive(Debug)]
pub enum Outcome {
    Accepted { cpu_time: Duration, max_rss: u64 },
    /// The interactor rejected the solution (exit code 1).
    Wrong(String),
    /// The interactor found the solution's output malformed (exit code 2).
    Presentation(String),
    /// The solution broke a limit or crashed first.
    SolutionFailed(RunError),
    /// The interactor itself failed, which says nothing about the solution.
    InteractorFailed(String),
}

#[derive(Debug)]
pub struct Verdict {
    pub outcome: Outcome,
    /// What the other side did, when it failed as well.
    pub note: Option<String>,
}

/// Runs the interaction and decides the verdict.
pub async fn interact(solution: &Program, interactor: &Program, cwd: &Path, limits: &Limits) -> Verdict {
    match run_interaction(solution, interactor, cwd, limits).await {
        Ok(report) => decide(&report, limits),
        Err(InteractionError::TimedOut { by_wall_cap }) => Verdict {
            outcome: Outcome::SolutionFailed(RunError::Tle),
            note: by_wall_cap.then(|| {
                "both programs were still running when the wall-clock cap was reached: the solution may be \
                 too slow or waiting for input that never comes, or the interactor may be stuck"
                    .to_string()
            }),
        },
        Err(InteractionError::System(e)) => Verdict { outcome: Outcome::SolutionFailed(RunError::System(e)), note: None },
    }
}

/// Applies the limits and the interactor's verdict; the first failure wins (see the module docs).
pub fn decide(report: &InteractionReport, limits: &Limits) -> Verdict {
    let solution = &report.solution;
    let interactor = &report.interactor;

    // A process the judge had to kill did not fail by itself.
    let solution_failure = if solution.killed_by_judge {
        None
    } else {
        violation(solution.status, &solution.stderr, solution.cpu_time, solution.max_rss, limits)
    };
    let interactor_verdict = if interactor.killed_by_judge {
        ExitVerdict::Ok
    } else {
        checker::exit_verdict("interactor", interactor.status, &interactor.stderr)
    };

    match (solution_failure, interactor_verdict) {
        (None, ExitVerdict::Ok) => Verdict {
            outcome: Outcome::Accepted { cpu_time: solution.cpu_time, max_rss: solution.max_rss },
            note: None,
        },
        (None, rejected) => {
            let solution_left_first = solution.exited_after < interactor.exited_after;
            if solution_left_first && failed_writing_to_closed_pipe(interactor) {
                // The pipe can only break once the reader is gone, so the solution ended the
                // conversation early; the interactor's crash is just how that showed up.
                return Verdict { outcome: Outcome::Wrong(SOLUTION_STOPPED_READING.to_string()), note: None };
            }
            let outcome = interactor_outcome(rejected);
            let note = (solution_left_first && matches!(outcome, Outcome::InteractorFailed(_))).then(|| {
                "the solution had already exited when the interactor failed, so this may be a consequence of \
                 the solution ending the conversation early"
                    .to_string()
            });
            Verdict { outcome, note }
        }
        (Some(failure), ExitVerdict::Ok) => Verdict { outcome: Outcome::SolutionFailed(failure), note: None },
        (Some(failure), rejected) => {
            if solution.exited_after <= interactor.exited_after {
                Verdict {
                    note: Some(format!("afterwards the interactor reported: {}", describe_rejection(&rejected))),
                    outcome: Outcome::SolutionFailed(failure),
                }
            } else {
                Verdict {
                    note: Some(format!("afterwards the solution failed too: {}", failure.describe())),
                    outcome: interactor_outcome(rejected),
                }
            }
        }
    }
}

const SOLUTION_STOPPED_READING: &str =
    "the solution stopped reading before the conversation was over (the interactor failed writing to it: broken pipe)";

const SIGPIPE: i32 = 13;

/// Whether the interactor died because it wrote to a pipe nobody reads any more: killed by
/// SIGPIPE (the default for C and C++ programs), or, where the language ignores SIGPIPE,
/// crashed on the resulting `EPIPE` error (Rust reports it as "Broken pipe").
fn failed_writing_to_closed_pipe(interactor: &ProcessReport) -> bool {
    interactor.status.signal() == Some(SIGPIPE)
        || (!interactor.status.success() && String::from_utf8_lossy(&interactor.stderr).contains("Broken pipe"))
}

fn interactor_outcome(verdict: ExitVerdict) -> Outcome {
    match verdict {
        ExitVerdict::Ok => unreachable!("an accepting interactor is not an outcome of its own"),
        ExitVerdict::Wrong(message) => Outcome::Wrong(message),
        ExitVerdict::Presentation(message) => Outcome::Presentation(message),
        ExitVerdict::Failed(message) => Outcome::InteractorFailed(message),
    }
}

fn describe_rejection(verdict: &ExitVerdict) -> String {
    let (what, message) = match verdict {
        ExitVerdict::Ok => return "OK".to_string(),
        ExitVerdict::Wrong(message) => ("wrong answer", message),
        ExitVerdict::Presentation(message) => ("presentation error", message),
        ExitVerdict::Failed(message) => return message.clone(),
    };
    if message.is_empty() { what.to_string() } else { format!("{what}: {message}") }
}

fn command(program: &Program, cwd: &Path) -> Command {
    let mut command = Command::new(&program.path);
    command.args(&program.args).current_dir(cwd).stderr(Stdio::piped());
    command
}

/// Waits for one process and gathers its report. `reaped` is set the moment the process
/// has been reaped: from then on its pid may be reused, so nothing may signal it.
fn supervise(mut child: Child, start: Instant, reaped: Arc<AtomicBool>) -> Result<ProcessReport, String> {
    let mut stderr_pipe = child.stderr.take().ok_or("failed to capture stderr")?;
    let read_stderr = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let waited = child.wait4();
    reaped.store(true, Ordering::Release);
    let exited_after = start.elapsed();
    let res_use = waited.map_err(|e| e.to_string())?;

    Ok(ProcessReport {
        status: res_use.status,
        stderr: read_stderr.join().unwrap_or_default(),
        cpu_time: res_use.rusage.utime + res_use.rusage.stime,
        max_rss: res_use.rusage.maxrss,
        exited_after,
        killed_by_judge: false,
    })
}

fn joined(result: Result<Result<ProcessReport, String>, tokio::task::JoinError>) -> Result<ProcessReport, InteractionError> {
    match result {
        Ok(Ok(report)) => Ok(report),
        Ok(Err(e)) => Err(InteractionError::System(e)),
        Err(e) => Err(InteractionError::System(e.to_string())),
    }
}

/// Runs the solution and the interactor connected to each other and waits for both.
///
/// The judge's own copies of the pipe ends are closed right after both processes have
/// started: if it kept them, a process closing its end would never be seen as EOF by the
/// other side.
///
/// While they run, the judge enforces the limits and keeps the pair from waiting on each
/// other: the solution is killed at its CPU limit, both at the wall-clock cap; a solution
/// still running after the interactor has rejected it is killed at once, and an interactor
/// still running shortly after the solution has failed is killed too.
pub async fn run_interaction(
    solution: &Program,
    interactor: &Program,
    cwd: &Path,
    limits: &Limits,
) -> Result<InteractionReport, InteractionError> {
    let system = |e: io::Error| InteractionError::System(e.to_string());
    let (solution_stdin, interactor_stdout) = io::pipe().map_err(system)?;
    let (interactor_stdin, solution_stdout) = io::pipe().map_err(system)?;

    let mut interactor_cmd = command(interactor, cwd);
    interactor_cmd.stdin(interactor_stdin).stdout(interactor_stdout);
    let mut solution_cmd = command(solution, cwd);
    solution_cmd.stdin(solution_stdin).stdout(solution_stdout);

    let mut interactor_child = interactor_cmd
        .spawn()
        .map_err(|e| InteractionError::System(format!("could not start the interactor: {e}")))?;
    let solution_child = match solution_cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = interactor_child.kill();
            let _ = interactor_child.wait();
            return Err(InteractionError::System(format!("could not start the solution: {e}")));
        }
    };
    // A Command keeps the pipe ends it was given until it is dropped.
    drop(interactor_cmd);
    drop(solution_cmd);

    let start = Instant::now();
    let deadline = start + limits.wall_cap();
    let (solution_pid, interactor_pid) = (solution_child.id(), interactor_child.id());
    let solution_reaped = Arc::new(AtomicBool::new(false));
    let interactor_reaped = Arc::new(AtomicBool::new(false));

    let mut solution_task = {
        let reaped = Arc::clone(&solution_reaped);
        tokio::task::spawn_blocking(move || supervise(solution_child, start, reaped))
    };
    let mut interactor_task = {
        let reaped = Arc::clone(&interactor_reaped);
        tokio::task::spawn_blocking(move || supervise(interactor_child, start, reaped))
    };

    // Returns whether the process was still running, i.e. whether a kill was really sent.
    let kill_if_running = |pid: u32, reaped: &AtomicBool| {
        let running = !reaped.load(Ordering::Acquire);
        if running {
            kill_process_tree(pid);
        }
        running
    };

    let mut solution_report: Option<ProcessReport> = None;
    let mut interactor_report: Option<ProcessReport> = None;
    let mut solution_killed = false;
    let mut interactor_killed = false;
    let mut interactor_deadline: Option<Instant> = None;

    loop {
        tokio::select! {
            result = &mut solution_task, if solution_report.is_none() => {
                let report = joined(result)?;
                // A solution that failed on its own leaves the interactor little time to
                // say why; one that merely finished may still be answered.
                let failed = !solution_killed
                    && violation(report.status, &report.stderr, report.cpu_time, report.max_rss, limits).is_some();
                if failed && interactor_report.is_none() {
                    interactor_deadline = Some(Instant::now() + INTERACTOR_GRACE);
                }
                solution_report = Some(report);
            }
            result = &mut interactor_task, if interactor_report.is_none() => {
                let report = joined(result)?;
                // The interactor has made up its mind. A solution still running can only
                // be waiting on it, so there is no point letting it run to the time limit.
                if report.status.code() != Some(0) && solution_report.is_none() {
                    solution_killed = kill_if_running(solution_pid, &solution_reaped);
                }
                interactor_report = Some(report);
            }
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                let now = Instant::now();
                let over_wall_cap = now >= deadline;
                let over_cpu_limit = solution_report.is_none()
                    && !solution_reaped.load(Ordering::Acquire)
                    && process_cpu_time(solution_pid).is_some_and(|t| t > limits.time);
                if over_wall_cap || over_cpu_limit {
                    kill_if_running(solution_pid, &solution_reaped);
                    kill_if_running(interactor_pid, &interactor_reaped);
                    return Err(InteractionError::TimedOut { by_wall_cap: !over_cpu_limit });
                }
                if interactor_report.is_none() && interactor_deadline.is_some_and(|d| now >= d) {
                    interactor_killed = kill_if_running(interactor_pid, &interactor_reaped);
                    interactor_deadline = None;
                }
            }
        }

        if solution_report.is_some() && interactor_report.is_some() {
            let mut solution = solution_report.take().expect("checked above");
            let mut interactor = interactor_report.take().expect("checked above");
            solution.killed_by_judge = solution_killed;
            interactor.killed_by_judge = interactor_killed;
            return Ok(InteractionReport { solution, interactor });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits { time: Duration::from_millis(1000), memory_bytes: 1 << 20 }
    }

    fn process(status_raw: i32, stderr: &str, exited_after_ms: u64) -> ProcessReport {
        ProcessReport {
            status: ExitStatus::from_raw(status_raw),
            stderr: stderr.as_bytes().to_vec(),
            cpu_time: Duration::from_millis(10),
            max_rss: 1000,
            exited_after: Duration::from_millis(exited_after_ms),
            killed_by_judge: false,
        }
    }

    fn exited(code: i32) -> i32 {
        code << 8
    }

    fn report(solution: ProcessReport, interactor: ProcessReport) -> InteractionReport {
        InteractionReport { solution, interactor }
    }

    // ---- decide: no failure on the solution's side ---------------------------

    #[test]
    fn both_sides_ok_is_accepted() {
        let v = decide(&report(process(0, "", 5), process(0, "", 6)), &limits());
        assert!(matches!(v.outcome, Outcome::Accepted { .. }), "{v:?}");
        assert!(v.note.is_none());
    }

    #[test]
    fn interactor_exit_codes_are_the_testlib_verdicts() {
        let outcome = |code, msg: &str| decide(&report(process(0, "", 5), process(exited(code), msg, 6)), &limits()).outcome;
        assert!(matches!(outcome(1, "bad"), Outcome::Wrong(m) if m == "bad"));
        assert!(matches!(outcome(2, "format"), Outcome::Presentation(m) if m == "format"));
        assert!(matches!(outcome(3, "oops"), Outcome::InteractorFailed(m) if m.contains("oops")));
        assert!(matches!(outcome(7, ""), Outcome::InteractorFailed(m) if m.contains("code 7")));
    }

    #[test]
    fn an_interactor_killed_by_sigpipe_after_the_solution_left_blames_the_solution() {
        // The solution exited cleanly at 5 ms; the interactor then wrote to it and was killed.
        let v = decide(&report(process(0, "", 5), process(SIGPIPE, "", 9)), &limits());
        assert!(matches!(&v.outcome, Outcome::Wrong(m) if m.contains("stopped reading")), "{v:?}");
        assert!(v.note.is_none(), "{v:?}");
    }

    #[test]
    fn an_interactor_panicking_on_a_broken_pipe_after_the_solution_left_blames_the_solution() {
        let stderr = "thread 'main' panicked: called `Result::unwrap()` on an `Err` value: Os { code: 32, kind: BrokenPipe, message: \"Broken pipe\" }";
        let v = decide(&report(process(0, "", 5), process(exited(101), stderr, 9)), &limits());
        assert!(matches!(&v.outcome, Outcome::Wrong(m) if m.contains("stopped reading")), "{v:?}");
    }

    #[test]
    fn a_broken_pipe_message_does_not_excuse_an_interactor_that_failed_before_the_solution_left() {
        // The interactor died first, so the pipe cannot have been what broke it.
        let v = decide(&report(process(0, "", 9), process(exited(101), "Broken pipe", 5)), &limits());
        assert!(matches!(v.outcome, Outcome::InteractorFailed(_)), "{v:?}");
        assert!(v.note.is_none(), "{v:?}");
    }

    #[test]
    fn an_interactor_that_fails_after_the_solution_left_gets_a_note() {
        // Unrelated to pipes as far as the judge can tell, but worth a hint.
        let v = decide(&report(process(0, "", 5), process(exited(101), "index out of bounds", 9)), &limits());
        assert!(matches!(v.outcome, Outcome::InteractorFailed(_)), "{v:?}");
        assert!(v.note.as_deref().is_some_and(|n| n.contains("already exited")), "{v:?}");
    }

    #[test]
    fn a_crashing_interactor_is_an_interactor_failure_not_a_wrong_answer() {
        // Both exited at about the same moment, so nothing suggests the solution caused it.
        let v = decide(&report(process(0, "", 6), process(11, "", 5)), &limits());
        assert!(matches!(&v.outcome, Outcome::InteractorFailed(m) if m.contains("signal 11")), "{v:?}");
    }

    // ---- decide: the solution failed ------------------------------------------

    #[test]
    fn solution_failure_with_an_ok_interactor_is_the_solutions_fault() {
        let v = decide(&report(process(exited(101), "boom", 5), process(0, "", 6)), &limits());
        assert!(matches!(v.outcome, Outcome::SolutionFailed(RunError::Re { .. })), "{v:?}");
        assert!(v.note.is_none());
    }

    #[test]
    fn solution_cpu_time_over_the_limit_is_tle_even_if_the_interactor_is_happy() {
        let mut slow = process(0, "", 5);
        slow.cpu_time = Duration::from_millis(1500);
        let v = decide(&report(slow, process(0, "", 6)), &limits());
        assert!(matches!(v.outcome, Outcome::SolutionFailed(RunError::Tle)), "{v:?}");
    }

    #[test]
    fn solution_memory_over_the_limit_is_mle() {
        let mut big = process(0, "", 5);
        big.max_rss = (1 << 20) + 1;
        let v = decide(&report(big, process(0, "", 6)), &limits());
        assert!(matches!(v.outcome, Outcome::SolutionFailed(RunError::Mle)), "{v:?}");
    }

    // ---- decide: both failed, so the first failure wins ------------------------

    #[test]
    fn solution_crashed_first_so_the_interactors_complaint_is_only_a_note() {
        // The solution died at 5 ms; the interactor then read EOF and rejected it at 9 ms.
        let v = decide(&report(process(exited(101), "boom", 5), process(exited(1), "unexpected EOF", 9)), &limits());
        assert!(matches!(v.outcome, Outcome::SolutionFailed(RunError::Re { .. })), "{v:?}");
        let note = v.note.expect("the interactor's side should be reported");
        assert!(note.contains("unexpected EOF"), "{note}");
    }

    #[test]
    fn interactor_rejected_first_so_the_solutions_crash_is_only_a_note() {
        // The interactor rejected the solution at 5 ms; the solution then died of a broken pipe.
        let v = decide(&report(process(exited(101), "Broken pipe", 9), process(exited(1), "too many queries", 5)), &limits());
        assert!(matches!(&v.outcome, Outcome::Wrong(m) if m == "too many queries"), "{v:?}");
        let note = v.note.expect("the solution's side should be reported");
        assert!(note.contains("exit code 101"), "{note}");
    }

    #[test]
    fn a_simultaneous_exit_is_blamed_on_the_solution() {
        let v = decide(&report(process(exited(101), "", 5), process(exited(1), "x", 5)), &limits());
        assert!(matches!(v.outcome, Outcome::SolutionFailed(_)), "{v:?}");
    }

    #[test]
    fn a_solution_the_judge_killed_does_not_count_as_failed() {
        // The interactor rejected the solution, and the judge killed the solution because of it.
        let mut killed = process(9, "", 6);
        killed.killed_by_judge = true;
        let v = decide(&report(killed, process(exited(1), "wrong", 5)), &limits());
        assert!(matches!(&v.outcome, Outcome::Wrong(m) if m == "wrong"), "{v:?}");
        assert!(v.note.is_none(), "the judge's own kill is not news: {v:?}");
    }

    #[test]
    fn an_interactor_the_judge_killed_does_not_override_a_solution_failure() {
        let mut killed = process(9, "", 300);
        killed.killed_by_judge = true;
        let v = decide(&report(process(exited(3), "", 5), killed), &limits());
        assert!(matches!(v.outcome, Outcome::SolutionFailed(RunError::Re { .. })), "{v:?}");
        assert!(v.note.is_none(), "{v:?}");
    }

    // ---- run_interaction with real processes ------------------------------------

    fn sh(script: &str) -> Program {
        Program::new("sh").with_args(["-c", script])
    }

    async fn run(solution: &str, interactor: &str, time_ms: u64) -> (Verdict, Duration) {
        let limits = Limits { time: Duration::from_millis(time_ms), memory_bytes: 1 << 40 };
        let started = Instant::now();
        let verdict = interact(&sh(solution), &sh(interactor), Path::new("."), &limits).await;
        (verdict, started.elapsed())
    }

    /// The interactor sends 41 and expects 42 back; the solution adds `add` to what it reads.
    fn ping_pong(add: i32) -> (String, &'static str) {
        (
            format!("read x; echo $((x+{add}))"),
            "echo 41; read r; [ \"$r\" = 42 ] || { echo \"got '$r'\" >&2; exit 1; }",
        )
    }

    #[tokio::test]
    async fn a_correct_conversation_is_accepted() {
        let (solution, interactor) = ping_pong(1);
        let (v, _) = run(&solution, interactor, 5000).await;
        assert!(matches!(v.outcome, Outcome::Accepted { .. }), "{v:?}");
    }

    #[tokio::test]
    async fn a_wrong_reply_is_rejected_with_the_interactors_message() {
        let (solution, interactor) = ping_pong(2);
        let (v, _) = run(&solution, interactor, 5000).await;
        assert!(matches!(&v.outcome, Outcome::Wrong(m) if m == "got '43'"), "{v:?}");
    }

    #[tokio::test]
    async fn a_solution_that_quits_silently_is_rejected_by_the_interactor() {
        // The solution exits 0 without answering: not a runtime error, but the interactor sees
        // EOF. `trap` and the redirect keep the interactor's own `echo` from dying of SIGPIPE
        // if it runs after the solution has gone, which would be a different test.
        let interactor = "trap '' PIPE; echo 41 2>/dev/null; read r; [ \"$r\" = 42 ] || { echo \"got '$r'\" >&2; exit 1; }";
        let (v, _) = run("exit 0", interactor, 5000).await;
        assert!(matches!(&v.outcome, Outcome::Wrong(m) if m == "got ''"), "{v:?}");
    }

    /// A C or C++ interactor writing to a solution that has already gone is killed by
    /// SIGPIPE. That is the solution's doing, not an interactor failure.
    #[tokio::test]
    async fn an_interactor_killed_writing_to_an_exited_solution_blames_the_solution() {
        // The sleep guarantees the solution is gone before the interactor writes.
        let (v, _) = run("exit 0", "sleep 0.1; echo 41; read r", 5000).await;
        assert!(matches!(&v.outcome, Outcome::Wrong(m) if m.contains("stopped reading")), "{v:?}");
    }

    #[tokio::test]
    async fn a_solution_that_crashes_first_gets_the_runtime_error() {
        // `trap '' PIPE` and the sleep keep the interactor's reaction well behind the crash.
        let interactor = "trap '' PIPE; echo 41 2>/dev/null; read r || { sleep 0.05; echo 'eof from solution' >&2; exit 1; }";
        let (v, _) = run("exit 7", interactor, 5000).await;
        match &v.outcome {
            Outcome::SolutionFailed(RunError::Re { status, .. }) => assert_eq!(status.code(), Some(7)),
            other => panic!("expected RE, got {other:?}"),
        }
        assert!(v.note.as_deref().is_some_and(|n| n.contains("eof from solution")), "{v:?}");
    }

    #[tokio::test]
    async fn a_rejected_solution_is_not_left_running_until_its_time_limit() {
        // The solution spins forever; the interactor rejects it at once. The verdict must
        // come right away rather than after the 5 s time limit.
        let (v, elapsed) = run("while :; do :; done", "echo 'bad start' >&2; exit 1", 5000).await;
        assert!(matches!(&v.outcome, Outcome::Wrong(m) if m == "bad start"), "{v:?}");
        assert!(v.note.is_none(), "{v:?}");
        assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_spinning_solution_is_killed_at_its_cpu_limit() {
        let limits = Limits { time: Duration::from_millis(300), memory_bytes: 1 << 40 };
        let (v, elapsed) = run("while :; do :; done", "read r", 300).await;
        assert!(matches!(v.outcome, Outcome::SolutionFailed(RunError::Tle)), "{v:?}");
        assert!(v.note.is_none(), "killed by the CPU limit, not the wall-clock cap: {v:?}");
        assert!(elapsed >= Duration::from_millis(250), "killed too early: {elapsed:?}");
        assert!(elapsed < limits.wall_cap(), "was not killed by the CPU limit: {elapsed:?}");
    }

    #[tokio::test]
    async fn two_programs_waiting_on_each_other_hit_the_wall_cap() {
        let (v, _) = run("read x; echo $x", "read x; echo $x", 100).await;
        assert!(matches!(v.outcome, Outcome::SolutionFailed(RunError::Tle)), "{v:?}");
        assert!(v.note.as_deref().is_some_and(|n| n.contains("wall-clock cap")), "{v:?}");
    }

    #[tokio::test]
    async fn a_solution_that_never_exits_after_an_ok_interactor_is_tle() {
        let (v, _) = run("sleep 30", "exit 0", 100).await;
        assert!(matches!(v.outcome, Outcome::SolutionFailed(RunError::Tle)), "{v:?}");
    }

    #[tokio::test]
    async fn a_missing_solution_is_a_system_error() {
        let limits = Limits { time: Duration::from_secs(5), memory_bytes: 1 << 40 };
        let v = interact(&Program::new("/nonexistent/solution"), &sh("exit 0"), Path::new("."), &limits).await;
        assert!(matches!(&v.outcome, Outcome::SolutionFailed(RunError::System(m)) if m.contains("solution")), "{v:?}");
    }

    #[tokio::test]
    async fn a_missing_interactor_is_a_system_error() {
        let limits = Limits { time: Duration::from_secs(5), memory_bytes: 1 << 40 };
        let v = interact(&sh("exit 0"), &Program::new("/nonexistent/interactor"), Path::new("."), &limits).await;
        assert!(matches!(&v.outcome, Outcome::SolutionFailed(RunError::System(m)) if m.contains("interactor")), "{v:?}");
    }

    /// Both directions carry a lot of data without either side deadlocking: the interactor
    /// sends 50k lines, one at a time, and checks the solution's echo of each.
    #[tokio::test]
    async fn heavy_traffic_in_both_directions_does_not_deadlock() {
        let interactor = "for n in $(seq 1 50000); do echo $n; read r; [ \"$r\" = \"$n\" ] || exit 1; done";
        let (v, _) = run("cat", interactor, 20_000).await;
        assert!(matches!(v.outcome, Outcome::Accepted { .. }), "{v:?}");
    }
}

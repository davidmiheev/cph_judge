//! Runs one executable under resource limits and classifies the result.
//!
//! Two steps, deliberately separate so later features (checker, brute force,
//! interactor) can reuse the raw run and apply their own interpretation:
//!
//! 1. [`run_once`] spawns the process, feeds stdin, drains stdout/stderr, and
//!    reports exit status plus resource usage as a [`RunReport`]. It only fails
//!    when the process cannot be run or exceeds the hard wall-clock cap.
//! 2. [`RunReport::check`] turns a report into a verdict using [`Limits`].

use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    time::Duration,
};
use tokio::time::timeout;
use wait4::Wait4;

#[derive(Debug)]
pub enum RunError {
    Tle,
    Mle,
    Re { status: ExitStatus, stderr: String },
    System(String),
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Limit on CPU time (user + system) consumed by the process.
    pub time: Duration,
    pub memory_bytes: u64,
}

impl Limits {
    /// Hard wall-clock cap after which the process is killed and reported as TLE.
    ///
    /// The time limit itself is enforced on CPU time, so a process that is merely
    /// descheduled is not penalised. This cap only catches processes that are
    /// blocked or sleeping (and so never accumulate CPU time).
    pub fn wall_cap(&self) -> Duration {
        self.time * 2 + Duration::from_millis(500)
    }
}

/// Raw result of a finished process, before any limit is applied.
pub struct RunReport {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// User + system CPU time.
    pub cpu_time: Duration,
    /// Peak resident set size in bytes.
    pub max_rss: u64,
}

/// A run that stayed within its limits.
pub struct RunOutput {
    pub stdout: String,
    pub cpu_time: Duration,
    pub max_rss: u64,
}

impl RunReport {
    /// Applies the limits. Precedence: TLE, then RE, then MLE.
    pub fn check(self, limits: &Limits) -> Result<RunOutput, RunError> {
        if self.cpu_time > limits.time {
            return Err(RunError::Tle);
        }
        if !self.status.success() {
            return Err(RunError::Re {
                status: self.status,
                stderr: String::from_utf8_lossy(&self.stderr).into_owned(),
            });
        }
        if self.max_rss > limits.memory_bytes {
            return Err(RunError::Mle);
        }
        Ok(RunOutput {
            stdout: String::from_utf8_lossy(&self.stdout).into_owned(),
            cpu_time: self.cpu_time,
            max_rss: self.max_rss,
        })
    }
}

/// Runs `program` with `args` in `cwd`, feeding it `input` on stdin.
///
/// Returns `Err(RunError::Tle)` if the process is still running after `wall_cap`
/// (it is killed), and `Err(RunError::System)` if it cannot be spawned or waited on.
/// A non-zero exit status is *not* an error here; see [`RunReport::check`].
pub async fn run_once<I, S>(
    program: &Path,
    args: I,
    cwd: &Path,
    input: &[u8],
    wall_cap: Duration,
) -> Result<RunReport, RunError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RunError::System(e.to_string()))?;

    let pid = child.id();
    let input_owned = input.to_vec();

    let run_task = tokio::task::spawn_blocking(move || -> Result<RunReport, RunError> {
        let stdin = child.stdin.take();
        let mut stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| RunError::System("Failed to capture stdout".into()))?;
        let mut stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| RunError::System("Failed to capture stderr".into()))?;

        // stdin is written from its own thread so that a program which streams its
        // output while still reading input cannot deadlock against us: all three
        // pipes are serviced concurrently. The thread is detached on purpose: it
        // ends on its own once the child has consumed the input or exited (EPIPE).
        std::thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(&input_owned);
            }
        });

        let read_stdout = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout_pipe.read_to_end(&mut buf);
            buf
        });
        let read_stderr = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut buf);
            buf
        });

        let res_use = child.wait4().map_err(|e| RunError::System(e.to_string()))?;
        let stdout = read_stdout.join().unwrap_or_default();
        let stderr = read_stderr.join().unwrap_or_default();

        Ok(RunReport {
            status: res_use.status,
            stdout,
            stderr,
            cpu_time: res_use.rusage.utime + res_use.rusage.stime,
            max_rss: res_use.rusage.maxrss,
        })
    });

    match timeout(wall_cap, run_task).await {
        Ok(Ok(report)) => report,
        Ok(Err(join_err)) => Err(RunError::System(join_err.to_string())),
        Err(_) => {
            kill_process_tree(pid);
            Err(RunError::Tle)
        }
    }
}

fn kill_process_tree(pid: u32) {
    let _ = Command::new("pkill")
        .arg("-9")
        .arg("-P")
        .arg(pid.to_string())
        .output();
    let _ = Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::time::Instant;

    const NO_ARGS: [&str; 0] = [];

    fn report(exit_raw: i32, cpu_ms: u64, max_rss: u64) -> RunReport {
        RunReport {
            status: ExitStatus::from_raw(exit_raw),
            stdout: b"out".to_vec(),
            stderr: b"err".to_vec(),
            cpu_time: Duration::from_millis(cpu_ms),
            max_rss,
        }
    }

    fn limits() -> Limits {
        Limits { time: Duration::from_millis(1000), memory_bytes: 1000 }
    }

    // ---- RunReport::check -------------------------------------------------

    #[test]
    fn check_accepts_run_within_limits() {
        let out = report(0, 10, 10).check(&limits()).expect("should pass");
        assert_eq!(out.stdout, "out");
    }

    #[test]
    fn check_tle_is_based_on_cpu_time() {
        assert!(matches!(report(0, 1001, 10).check(&limits()), Err(RunError::Tle)));
        assert!(report(0, 1000, 10).check(&limits()).is_ok());
    }

    #[test]
    fn check_tle_wins_over_crash() {
        // A process that burned more CPU than allowed and then crashed was
        // too slow first; the crash is not what a judge would have seen.
        assert!(matches!(report(1 << 8, 1500, 10).check(&limits()), Err(RunError::Tle)));
    }

    #[test]
    fn check_nonzero_exit_is_re() {
        match report(3 << 8, 10, 10).check(&limits()) {
            Err(RunError::Re { status, stderr }) => {
                assert_eq!(status.code(), Some(3));
                assert_eq!(stderr, "err");
            }
            _ => panic!("expected RE"),
        }
    }

    #[test]
    fn check_signal_is_re() {
        match report(11, 10, 10).check(&limits()) {
            Err(RunError::Re { status, .. }) => assert_eq!(status.signal(), Some(11)),
            _ => panic!("expected RE"),
        }
    }

    #[test]
    fn check_memory_over_limit_is_mle() {
        assert!(matches!(report(0, 10, 1001).check(&limits()), Err(RunError::Mle)));
        assert!(report(0, 10, 1000).check(&limits()).is_ok());
    }

    #[test]
    fn wall_cap_exceeds_time_limit() {
        let l = limits();
        assert!(l.wall_cap() > l.time);
    }

    // ---- run_once (spawns real processes) ----------------------------------

    #[tokio::test]
    async fn captures_stdout_stderr_and_status() {
        let r = run_once(
            Path::new("sh"),
            ["-c", "echo hello; echo oops >&2; exit 3"],
            Path::new("."),
            b"",
            Duration::from_secs(10),
        )
        .await
        .expect("run");
        assert_eq!(r.stdout, b"hello\n");
        assert_eq!(r.stderr, b"oops\n");
        assert_eq!(r.status.code(), Some(3));
    }

    #[tokio::test]
    async fn passes_stdin_through() {
        let r = run_once(Path::new("cat"), NO_ARGS, Path::new("."), b"abc 123\n", Duration::from_secs(10))
            .await
            .expect("run");
        assert_eq!(r.stdout, b"abc 123\n");
    }

    /// Regression: the judge used to write all of stdin before it started draining
    /// stdout, so a program that streams its output (like `cat`) deadlocked once
    /// input and output both exceeded the pipe buffer and was reported as TLE.
    #[tokio::test]
    async fn large_streaming_io_does_not_deadlock() {
        let input: Vec<u8> = (0..300_000).flat_map(|i| format!("{i}\n").into_bytes()).collect();
        assert!(input.len() > 1_000_000);
        let r = run_once(Path::new("cat"), NO_ARGS, Path::new("."), &input, Duration::from_secs(20))
            .await
            .expect("cat must not deadlock or time out");
        assert_eq!(r.stdout, input);
    }

    #[tokio::test]
    async fn program_that_ignores_stdin_is_fine() {
        // Child exits without reading a large input: the writer thread gets EPIPE.
        let input = vec![b'x'; 4 * 1024 * 1024];
        let r = run_once(Path::new("sh"), ["-c", "echo done"], Path::new("."), &input, Duration::from_secs(10))
            .await
            .expect("run");
        assert_eq!(r.stdout, b"done\n");
        assert!(r.status.success());
    }

    #[tokio::test]
    async fn wall_cap_kills_sleeping_process() {
        let started = Instant::now();
        let r = run_once(Path::new("sleep"), ["30"], Path::new("."), b"", Duration::from_millis(300)).await;
        assert!(matches!(r, Err(RunError::Tle)));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn wall_cap_kills_busy_loop() {
        let r = run_once(Path::new("sh"), ["-c", "while :; do :; done"], Path::new("."), b"", Duration::from_millis(300)).await;
        assert!(matches!(r, Err(RunError::Tle)));
    }

    #[tokio::test]
    async fn reports_cpu_time_for_busy_process() {
        let r = run_once(
            Path::new("sh"),
            ["-c", "i=0; while [ $i -lt 300000 ]; do i=$((i+1)); done"],
            Path::new("."),
            b"",
            Duration::from_secs(30),
        )
        .await
        .expect("run");
        assert!(r.cpu_time > Duration::ZERO, "cpu_time should be measured, got {:?}", r.cpu_time);
    }

    #[tokio::test]
    async fn missing_binary_is_system_error() {
        let r = run_once(Path::new("/nonexistent/binary"), NO_ARGS, Path::new("."), b"", Duration::from_secs(5)).await;
        assert!(matches!(r, Err(RunError::System(_))));
    }
}

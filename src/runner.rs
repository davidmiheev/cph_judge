//! Runs one executable under resource limits and classifies the result.
//!
//! Two steps, deliberately separate so later features (checker, brute force,
//! interactor) can reuse the raw run and apply their own interpretation:
//!
//! 1. [`run_once`] spawns the process, feeds stdin, drains stdout/stderr, and
//!    reports exit status plus resource usage as a [`RunReport`]. While it runs, the
//!    process is killed as soon as its CPU time passes the limit (Linux: polled from
//!    `/proc`) or the hard wall-clock cap is hit; both are reported as TLE.
//! 2. [`RunReport::check`] turns a report into a verdict using [`Limits`].

use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::Path,
    os::unix::process::ExitStatusExt,
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use wait4::Wait4;

/// How often a running process is checked against its CPU and wall-clock limits.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(10);

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
    /// descheduled is not penalised. This cap catches processes that are blocked or
    /// sleeping (and so never accumulate CPU time), and is the only enforcement on
    /// platforms where CPU time cannot be polled while a process runs.
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

impl RunError {
    /// A short description, for messages that are not the per-test verdict line.
    pub fn describe(&self) -> String {
        match self {
            RunError::Tle => "time limit exceeded".to_string(),
            RunError::Mle => "memory limit exceeded".to_string(),
            RunError::Re { status, .. } => format!("runtime error ({})", format_exit_status(*status)),
            RunError::System(e) => e.clone(),
        }
    }
}

/// Which limit, if any, a finished process broke. Precedence: TLE, then RE, then MLE.
pub fn violation(
    status: ExitStatus,
    stderr: &[u8],
    cpu_time: Duration,
    max_rss: u64,
    limits: &Limits,
) -> Option<RunError> {
    if cpu_time > limits.time {
        return Some(RunError::Tle);
    }
    if !status.success() {
        return Some(RunError::Re { status, stderr: String::from_utf8_lossy(stderr).into_owned() });
    }
    if max_rss > limits.memory_bytes {
        return Some(RunError::Mle);
    }
    None
}

/// Human-readable description of how a process ended.
pub fn format_exit_status(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        format!("exit code {}", code)
    } else if let Some(sig) = status.signal() {
        match sig {
            6 => "SIGABRT (abort / panic)".to_string(),
            8 => "SIGFPE (div by zero / arithmetic error)".to_string(),
            11 => "SIGSEGV (segmentation fault / memory error)".to_string(),
            sig => format!("signal {}", sig),
        }
    } else {
        "terminated unexpectedly".to_string()
    }
}

impl RunReport {
    /// Applies the limits. Precedence: TLE, then RE, then MLE.
    pub fn check(self, limits: &Limits) -> Result<RunOutput, RunError> {
        if let Some(error) = violation(self.status, &self.stderr, self.cpu_time, self.max_rss, limits) {
            return Err(error);
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
/// Returns `Err(RunError::Tle)` if the process is killed for using more than
/// `limits.time` of CPU time or running past `limits.wall_cap()`, and
/// `Err(RunError::System)` if it cannot be spawned or waited on. A non-zero exit
/// status is *not* an error here; see [`RunReport::check`].
pub async fn run_once<I, S>(
    program: &Path,
    args: I,
    cwd: &Path,
    input: &[u8],
    limits: &Limits,
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
    let deadline = Instant::now() + limits.wall_cap();

    // Set as soon as the child has been reaped: from then on its pid may be reused,
    // so the watcher below must neither read /proc/<pid> nor signal it.
    let reaped = Arc::new(AtomicBool::new(false));
    let reaped_by_runner = Arc::clone(&reaped);

    let mut run_task = tokio::task::spawn_blocking(move || -> Result<RunReport, RunError> {
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

        let waited = child.wait4();
        reaped_by_runner.store(true, Ordering::Release);
        let res_use = waited.map_err(|e| RunError::System(e.to_string()))?;
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

    loop {
        tokio::select! {
            joined = &mut run_task => {
                return match joined {
                    Ok(report) => report,
                    Err(join_err) => Err(RunError::System(join_err.to_string())),
                };
            }
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                if reaped.load(Ordering::Acquire) {
                    continue; // already finished; the task result is about to arrive
                }
                let over_wall_cap = Instant::now() >= deadline;
                let over_cpu_limit = process_cpu_time(pid).is_some_and(|t| t > limits.time);
                if over_wall_cap || over_cpu_limit {
                    kill_process_tree(pid);
                    return Err(RunError::Tle);
                }
            }
        }
    }
}

/// Linux `/proc/<pid>/stat` reports CPU time in clock ticks of `USER_HZ`, which is a
/// fixed 100 Hz in the Linux user-space ABI (the value `getconf CLK_TCK` prints).
#[cfg(target_os = "linux")]
const CLOCK_TICKS_PER_SEC: u64 = 100;

/// CPU time (user + system) a running process has used so far, or `None` where it
/// cannot be read (non-Linux platforms, or the process is gone).
#[cfg(target_os = "linux")]
pub(crate) fn process_cpu_time(pid: u32) -> Option<Duration> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat_cpu_time(&stat)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn process_cpu_time(_pid: u32) -> Option<Duration> {
    None
}

/// Extracts utime + stime (fields 14 and 15) from the contents of `/proc/<pid>/stat`.
///
/// The second field is the command name in parentheses and may itself contain spaces
/// and parentheses, so parsing starts after the *last* `)`.
#[cfg(target_os = "linux")]
fn parse_stat_cpu_time(stat: &str) -> Option<Duration> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_comm.split_whitespace(); // first field here is field 3 (state)
    let utime: u64 = fields.nth(11)?.parse().ok()?; // field 14
    let stime: u64 = fields.next()?.parse().ok()?; // field 15
    Some(Duration::from_millis((utime + stime) * 1000 / CLOCK_TICKS_PER_SEC))
}

pub(crate) fn kill_process_tree(pid: u32) {
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

    /// Limits that no well-behaved test process comes near.
    fn lenient() -> Limits {
        Limits { time: Duration::from_secs(30), memory_bytes: 1 << 40 }
    }

    fn with_time_ms(ms: u64) -> Limits {
        Limits { time: Duration::from_millis(ms), memory_bytes: 1 << 40 }
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

    // ---- /proc/<pid>/stat parsing -------------------------------------------

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_cpu_time_from_proc_stat() {
        // utime = 150 ticks, stime = 50 ticks at 100 Hz -> 2 s.
        let stat = "4242 (prog) S 1 4242 4242 0 -1 4194304 100 0 0 0 150 50 0 0 20 0 1 0 12345 1000 100";
        assert_eq!(parse_stat_cpu_time(stat), Some(Duration::from_secs(2)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_stat_command_name_may_contain_spaces_and_parens() {
        let stat = "4242 (a b) c) S 1 4242 4242 0 -1 4194304 100 0 0 0 150 50 0 0 20 0 1 0 12345 1000 100";
        assert_eq!(parse_stat_cpu_time(stat), Some(Duration::from_secs(2)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn malformed_proc_stat_is_none() {
        assert_eq!(parse_stat_cpu_time("garbage"), None);
        assert_eq!(parse_stat_cpu_time("1 (x) S 1 2 3"), None);
        assert_eq!(parse_stat_cpu_time("1 (x) S 1 4242 4242 0 -1 4194304 100 0 0 0 abc 50"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_cpu_time_of_live_process_and_none_for_missing_pid() {
        assert!(process_cpu_time(std::process::id()).is_some());
        assert!(process_cpu_time(u32::MAX).is_none());
    }

    // ---- run_once (spawns real processes) ----------------------------------

    #[tokio::test]
    async fn captures_stdout_stderr_and_status() {
        let r = run_once(
            Path::new("sh"),
            ["-c", "echo hello; echo oops >&2; exit 3"],
            Path::new("."),
            b"",
            &lenient(),
        )
        .await
        .expect("run");
        assert_eq!(r.stdout, b"hello\n");
        assert_eq!(r.stderr, b"oops\n");
        assert_eq!(r.status.code(), Some(3));
    }

    #[tokio::test]
    async fn passes_stdin_through() {
        let r = run_once(Path::new("cat"), NO_ARGS, Path::new("."), b"abc 123\n", &lenient())
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
        let r = run_once(Path::new("cat"), NO_ARGS, Path::new("."), &input, &with_time_ms(10_000))
            .await
            .expect("cat must not deadlock or time out");
        assert_eq!(r.stdout, input);
    }

    #[tokio::test]
    async fn program_that_ignores_stdin_is_fine() {
        // Child exits without reading a large input: the writer thread gets EPIPE.
        let input = vec![b'x'; 4 * 1024 * 1024];
        let r = run_once(Path::new("sh"), ["-c", "echo done"], Path::new("."), &input, &lenient())
            .await
            .expect("run");
        assert_eq!(r.stdout, b"done\n");
        assert!(r.status.success());
    }

    #[tokio::test]
    async fn wall_cap_kills_sleeping_process() {
        // A sleeping process uses no CPU, so only the wall-clock cap can stop it.
        let l = with_time_ms(100);
        let started = Instant::now();
        let r = run_once(Path::new("sleep"), ["30"], Path::new("."), b"", &l).await;
        assert!(matches!(r, Err(RunError::Tle)));
        assert!(started.elapsed() >= l.wall_cap(), "killed before the wall cap: {:?}", started.elapsed());
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// The point of polling: an infinite loop is reported at the CPU limit, not
    /// only after the (much longer) wall-clock cap.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cpu_limit_kills_busy_loop_before_wall_cap() {
        let l = with_time_ms(300);
        let started = Instant::now();
        let r = run_once(Path::new("sh"), ["-c", "while :; do :; done"], Path::new("."), b"", &l).await;
        let elapsed = started.elapsed();
        assert!(matches!(r, Err(RunError::Tle)));
        // A single-threaded loop cannot use 300 ms of CPU in less wall time than that.
        assert!(elapsed >= Duration::from_millis(250), "killed too early: {elapsed:?}");
        assert!(elapsed < l.wall_cap(), "was not killed by the CPU limit: {elapsed:?}");
    }

    #[tokio::test]
    async fn reports_cpu_time_for_busy_process() {
        let r = run_once(
            Path::new("sh"),
            ["-c", "i=0; while [ $i -lt 300000 ]; do i=$((i+1)); done"],
            Path::new("."),
            b"",
            &lenient(),
        )
        .await
        .expect("run");
        assert!(r.cpu_time > Duration::ZERO, "cpu_time should be measured, got {:?}", r.cpu_time);
    }

    #[tokio::test]
    async fn missing_binary_is_system_error() {
        let r = run_once(Path::new("/nonexistent/binary"), NO_ARGS, Path::new("."), b"", &lenient()).await;
        assert!(matches!(r, Err(RunError::System(_))));
    }
}

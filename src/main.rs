mod compare;
mod compile;
mod runner;

use colored::*;
use ignore::WalkBuilder;
use serde::Deserialize;
use std::{
    env, fs,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;

use compare::compare_outputs;
use compile::BuildOutcome;
use runner::{run_once, Limits, RunError};

#[derive(Deserialize)]
#[allow(dead_code)]
struct CphConfig {
    name: String,
    interactive: bool,
    #[serde(rename = "timeLimit")]
    time_limit: Option<u64>,
    #[serde(rename = "memoryLimit")]
    memory_limit: Option<u64>,
    tests: Vec<Test>,
    #[serde(rename = "srcPath")]
    src_path: String,
}

#[derive(Deserialize)]
struct Test {
    input: String,
    output: String,
}

fn format_exit_status(status: std::process::ExitStatus) -> String {
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

fn find_prob_file(search_root: &str, target: &str) -> Option<PathBuf> {
    println!("{} {} in {}...", "🔍 Searching for".yellow(), target.bold().cyan(), search_root);
    WalkBuilder::new(search_root)
        .hidden(false)
        .build()
        .filter_map(|e| e.ok())
        .find(|e| {
            let name = e.file_name().to_string_lossy();
            name.ends_with(".prob") && name.contains(target)
        })
        .map(|e| e.into_path())
}

/// Exit codes: 0 = every test passed, 1 = compilation failed or a test failed,
/// 2 = usage error or the judge itself could not run.
#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            ExitCode::from(2)
        }
    }
}

async fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: judge <search_dir> <problem_name>");
        return Ok(ExitCode::from(2));
    }

    let prob_path = find_prob_file(&args[1], &args[2]).ok_or("Metadata file not found.")?;

    let config: CphConfig = serde_json::from_str(&fs::read_to_string(&prob_path)?)?;
    let project_root = prob_path.parent().and_then(|p| p.parent()).ok_or("Root not found")?;

    fs::copy(&config.src_path, project_root.join("src/main.rs"))?;

    let bin_name = compile::package_name(project_root)?;

    println!("🔨 Compiling {} ({})", bin_name.bold(), "release".yellow());
    if let BuildOutcome::Failed { log } = compile::build_release(project_root)? {
        eprintln!("{}", "❌ Compilation failed!".red().bold());
        println!("{}", log);
        return Ok(ExitCode::from(1));
    }

    let bin_path = project_root.join("target/release").join(&bin_name);
    let total_tests = config.tests.len();

    let default_timeout = Duration::from_secs(3);
    let limits = Limits {
        time: config.time_limit.map(Duration::from_millis).unwrap_or(default_timeout),
        memory_bytes: config.memory_limit.unwrap_or(256) * 1024 * 1024, // MB to Bytes
    };

    // Time limits are measured in CPU time, but running far more processes than
    // cores still slows each of them down (cache, memory bandwidth, scheduling),
    // so at most one test per core runs at a time.
    let parallelism = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let permits = Arc::new(Semaphore::new(parallelism));

    let mut handles = Vec::new();
    for (i, test) in config.tests.into_iter().enumerate() {
        let bin_path = bin_path.clone();
        let project_root = project_root.to_path_buf();
        let permits = Arc::clone(&permits);

        handles.push(tokio::spawn(async move {
            let _permit = permits.acquire_owned().await.expect("semaphore is never closed");
            let result = match run_once(&bin_path, [] as [&str; 0], &project_root, test.input.as_bytes(), &limits).await {
                Ok(report) => report.check(&limits),
                Err(e) => Err(e),
            };
            (i, test, result)
        }));
    }

    let mut passed_count = 0;
    for handle in handles {
        let (i, test, result) = handle.await.expect("Failed to join task");

        let test_label = format!("Test #{}", i + 1);

        match result {
            Err(RunError::Tle) => {
                let limit_label = format!("{:.2}s", limits.time.as_secs_f32());
                println!("{}  {} ({})", "⏱️".yellow(), format!("{} TLE", test_label).yellow(), limit_label);
            }
            Err(RunError::Mle) => {
                let limit_label = format!("{} MB", limits.memory_bytes / 1024 / 1024);
                println!("{}  {} ({})", "💾".yellow(), format!("{} MLE", test_label).yellow(), limit_label);
            }
            Err(RunError::Re { status, stderr }) => {
                let status_desc = format_exit_status(status);
                println!("{}  {} ({})", "💥".red(), format!("{} RE", test_label).red().bold(), status_desc.yellow());
                let stderr_trimmed = stderr.trim();
                if !stderr_trimmed.is_empty() {
                    println!("{}", "--- Stderr / Crash Trace ---".magenta());
                    println!("{}", stderr_trimmed);
                    println!("{}", "----------------------------".magenta());
                }
            }
            Err(RunError::System(e)) => {
                println!("{}  {} ({})", "⚠️".red(), format!("{} Error", test_label).red(), e);
            }
            Ok(output) => {
                let actual = output.stdout.trim().to_string();
                let expected = test.output.trim().to_string();
                let mem_mb = output.max_rss as f64 / 1048576.0;

                if compare_outputs(&actual, &expected) {
                    println!("{} {} ({:.2?}, {:.2} MB)", "✅".green(), test_label.green(), output.cpu_time, mem_mb);
                    passed_count += 1;
                } else {
                    println!("{} {}", "❌".red(), test_label.red().bold());
                    println!("{}", "--- Expected ---".blue());
                    println!("{}", expected);
                    println!("{}", "--- Received ---".blue());
                    println!("{}", actual);
                    println!("{}", "----------------".blue());
                }
            }
        }
    }

    let final_text = format!("\nFinal Score: {}/{}", passed_count, total_tests);
    if passed_count == total_tests {
        println!("{}", final_text.green().bold());
        Ok(ExitCode::SUCCESS)
    } else {
        println!("{}", final_text.yellow().bold());
        Ok(ExitCode::from(1))
    }
}

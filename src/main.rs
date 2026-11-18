mod checker;
mod compare;
mod compile;
mod runner;

use colored::*;
use ignore::WalkBuilder;
use serde::Deserialize;
use std::{
    env, fs,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;

use checker::run_checker;
use compare::compare_outputs;
use compile::BuildOutcome;
use runner::{Limits, RunError, RunOutput, run_once};

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

/// Everything needed to judge one test, shared by all test tasks.
struct Judge {
    bin_path: PathBuf,
    project_root: PathBuf,
    limits: Limits,
    /// Compiled checker, when the solution has a `_checker` file.
    checker: Option<PathBuf>,
    /// Where the checker's input / output / answer files for each test are written.
    workdir: PathBuf,
}

/// What the program's output was judged to be, for a run that stayed within its limits.
enum Judgement {
    Accepted,
    /// Wrong answer. Carries the checker's message when a checker made the call.
    Wrong(Option<String>),
    Presentation(String),
    /// The checker itself failed, which says nothing about the solution.
    CheckerFailed(String),
}

struct TestOutcome {
    run: Result<RunOutput, RunError>,
    /// `None` when the run did not complete within its limits.
    judgement: Option<Judgement>,
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

/// Runs the solution on one test and judges its output.
async fn judge_test(judge: &Judge, index: usize, test: &Test) -> TestOutcome {
    let run = match run_once(&judge.bin_path, [] as [&str; 0], &judge.project_root, test.input.as_bytes(), &judge.limits).await {
        Ok(report) => report.check(&judge.limits),
        Err(e) => Err(e),
    };
    let judgement = match &run {
        Ok(output) => Some(judge_output(judge, index, test, output).await),
        Err(_) => None,
    };
    TestOutcome { run, judgement }
}

async fn judge_output(judge: &Judge, index: usize, test: &Test, output: &RunOutput) -> Judgement {
    let Some(checker_bin) = &judge.checker else {
        return if compare_outputs(output.stdout.trim(), test.output.trim()) {
            Judgement::Accepted
        } else {
            Judgement::Wrong(None)
        };
    };

    // The checker works on files: the test input, the program's output, and the jury's
    // answer (the expected output stored in the .prob file).
    let base = judge.workdir.join((index + 1).to_string());
    let input_file = base.with_extension("in");
    let output_file = base.with_extension("out");
    let answer_file = base.with_extension("ans");
    let written = async {
        tokio::fs::write(&input_file, &test.input).await?;
        tokio::fs::write(&output_file, &output.stdout).await?;
        tokio::fs::write(&answer_file, &test.output).await
    }
    .await;
    if let Err(e) = written {
        return Judgement::CheckerFailed(format!("could not write files for the checker: {e}"));
    }

    match run_checker(checker_bin, &judge.project_root, &input_file, &output_file, &answer_file).await {
        checker::Verdict::Ok => Judgement::Accepted,
        checker::Verdict::Wrong(message) => Judgement::Wrong(Some(message)),
        checker::Verdict::Presentation(message) => Judgement::Presentation(message),
        checker::Verdict::Failed(message) => Judgement::CheckerFailed(message),
    }
}

/// Prints the result of one test; returns whether it was accepted.
fn print_outcome(test_label: &str, test: &Test, limits: &Limits, outcome: TestOutcome) -> bool {
    match (outcome.run, outcome.judgement) {
        (Err(RunError::Tle), _) => {
            let limit_label = format!("{:.2}s", limits.time.as_secs_f32());
            println!("{}  {} ({})", "⏱️".yellow(), format!("{} TLE", test_label).yellow(), limit_label);
        }
        (Err(RunError::Mle), _) => {
            let limit_label = format!("{} MB", limits.memory_bytes / 1024 / 1024);
            println!("{}  {} ({})", "💾".yellow(), format!("{} MLE", test_label).yellow(), limit_label);
        }
        (Err(RunError::Re { status, stderr }), _) => {
            let status_desc = format_exit_status(status);
            println!("{}  {} ({})", "💥".red(), format!("{} RE", test_label).red().bold(), status_desc.yellow());
            let stderr_trimmed = stderr.trim();
            if !stderr_trimmed.is_empty() {
                println!("{}", "--- Stderr / Crash Trace ---".magenta());
                println!("{}", stderr_trimmed);
                println!("{}", "----------------------------".magenta());
            }
        }
        (Err(RunError::System(e)), _) => {
            println!("{}  {} ({})", "⚠️".red(), format!("{} Error", test_label).red(), e);
        }
        (Ok(output), Some(Judgement::Accepted)) => {
            let mem_mb = output.max_rss as f64 / 1048576.0;
            println!("{} {} ({:.2?}, {:.2} MB)", "✅".green(), test_label.green(), output.cpu_time, mem_mb);
            return true;
        }
        (Ok(output), Some(Judgement::Wrong(None))) => {
            println!("{} {}", "❌".red(), test_label.red().bold());
            println!("{}", "--- Expected ---".blue());
            println!("{}", test.output.trim());
            println!("{}", "--- Received ---".blue());
            println!("{}", output.stdout.trim());
            println!("{}", "----------------".blue());
        }
        (Ok(_), Some(Judgement::Wrong(Some(message)))) => {
            println!("{} {}", "❌".red(), test_label.red().bold());
            print_checker_message(&message);
        }
        (Ok(_), Some(Judgement::Presentation(message))) => {
            println!("{} {}", "❌".red(), format!("{} PE", test_label).red().bold());
            print_checker_message(&message);
        }
        (Ok(_), Some(Judgement::CheckerFailed(message))) => {
            println!("{}  {} ({})", "⚠️".red(), format!("{} checker failed", test_label).red(), message);
        }
        (Ok(_), None) => unreachable!("a completed run is always judged"),
    }
    false
}

fn print_checker_message(message: &str) {
    println!("{}", "--- Checker ---".blue());
    if message.is_empty() {
        println!("(no message)");
    } else {
        println!("{}", message);
    }
    println!("{}", "---------------".blue());
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

    let package = compile::package_info(project_root)?;
    let src_path = Path::new(&config.src_path);
    let checker_src = compile::aux_source(src_path, "checker");
    if let Some(checker_src) = &checker_src
        && !package.autobins
    {
        return Err(format!(
            "found {} but Cargo.toml sets `autobins = false`, so the judge cannot build it from src/bin/; \
             remove that setting or the checker",
            checker_src.display()
        )
        .into());
    }

    fs::copy(src_path, project_root.join("src/main.rs"))?;

    println!("🔨 Compiling {} ({})", package.name.bold(), "release".yellow());
    let mut bins = vec![package.name.as_str()];
    let mut generated: Vec<(&str, &Path)> = Vec::new();
    if let Some(checker_src) = &checker_src {
        let file_name = checker_src.file_name().unwrap_or_default().to_string_lossy();
        println!("🧪 Using checker {}", file_name.bold().cyan());
        bins.push(compile::CHECKER_BIN);
        generated.push((compile::CHECKER_BIN, checker_src.as_path()));
    }
    let outcome = {
        // The installed sources are removed when this block ends, whatever the outcome.
        let _installed = compile::GeneratedBins::install(project_root, &generated)?;
        compile::build_release(project_root, &bins)?
    };
    if let BuildOutcome::Failed { log } = outcome {
        eprintln!("{}", "❌ Compilation failed!".red().bold());
        if checker_src.is_some() {
            eprintln!("(the solution and the checker are built together; the log shows which one failed)");
        }
        println!("{}", log);
        return Ok(ExitCode::from(1));
    }

    let release_dir = project_root.join("target/release");
    let workdir = project_root.join("target/judge");
    if checker_src.is_some() {
        let _ = fs::remove_dir_all(&workdir);
        fs::create_dir_all(&workdir)?;
    }

    let default_timeout = Duration::from_secs(3);
    let limits = Limits {
        time: config.time_limit.map(Duration::from_millis).unwrap_or(default_timeout),
        memory_bytes: config.memory_limit.unwrap_or(256) * 1024 * 1024, // MB to Bytes
    };
    let judge = Arc::new(Judge {
        bin_path: release_dir.join(&package.name),
        project_root: project_root.to_path_buf(),
        limits,
        checker: checker_src.as_ref().map(|_| release_dir.join(compile::CHECKER_BIN)),
        workdir: workdir.clone(),
    });

    // Time limits are measured in CPU time, but running far more processes than
    // cores still slows each of them down (cache, memory bandwidth, scheduling),
    // so at most one test per core runs at a time.
    let parallelism = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let permits = Arc::new(Semaphore::new(parallelism));

    let total_tests = config.tests.len();
    let mut handles = Vec::new();
    for (i, test) in config.tests.into_iter().enumerate() {
        let judge = Arc::clone(&judge);
        let permits = Arc::clone(&permits);

        handles.push(tokio::spawn(async move {
            let _permit = permits.acquire_owned().await.expect("semaphore is never closed");
            let outcome = judge_test(&judge, i, &test).await;
            (i, test, outcome)
        }));
    }

    let mut passed_count = 0;
    for handle in handles {
        let (i, test, outcome) = handle.await.expect("Failed to join task");
        let test_label = format!("Test #{}", i + 1);
        if print_outcome(&test_label, &test, &limits, outcome) {
            passed_count += 1;
        }
    }

    if checker_src.is_some() && passed_count < total_tests {
        println!(
            "\nℹ️  Checker files for test N are in {}: N.in, N.out (program output), N.ans (stored answer)",
            workdir.display()
        );
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

mod checker;
mod compare;
mod compile;
mod interactive;
mod runner;

use colored::*;
use ignore::WalkBuilder;
use serde::Deserialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;

use checker::run_checker;
use compare::compare_outputs;
use compile::BuildOutcome;
use interactive::{Outcome, Program};
use runner::{Limits, RunError, RunOutput, format_exit_status, run_once};

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

/// What decides whether a run is right, chosen from the files next to the solution.
enum Verifier {
    /// Compare the output with the stored expected output.
    Builtin,
    /// `<solution>_checker.rs`: validates the output of a normal run.
    Checker(PathBuf),
    /// `<solution>_interactor.rs`: talks to the solution and gives the verdict.
    Interactor(PathBuf),
}

impl Verifier {
    /// The source file and Cargo binary name to build, if there is anything to build.
    fn bin(&self) -> Option<(&'static str, &Path)> {
        match self {
            Verifier::Builtin => None,
            Verifier::Checker(src) => Some((compile::CHECKER_BIN, src)),
            Verifier::Interactor(src) => Some((compile::INTERACTOR_BIN, src)),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Verifier::Builtin => "checker",
            Verifier::Checker(_) => "checker",
            Verifier::Interactor(_) => "interactor",
        }
    }
}

/// How each test is run, with the compiled verifier where there is one.
enum Mode {
    Batch { checker: Option<PathBuf> },
    Interactive { interactor: PathBuf },
}

/// Everything needed to judge one test, shared by all test tasks.
struct Judge {
    bin_path: PathBuf,
    project_root: PathBuf,
    limits: Limits,
    mode: Mode,
    /// Where the files handed to the checker or interactor are written, per test.
    workdir: PathBuf,
}

/// What the program's output was judged to be, for a run that stayed within its limits.
enum Judgement {
    Accepted,
    /// Wrong answer. Carries the checker's or interactor's message when it made the call.
    Wrong(Option<String>),
    Presentation(String),
    /// The checker or interactor itself failed, which says nothing about the solution.
    VerifierFailed(String),
}

struct TestOutcome {
    run: Result<RunOutput, RunError>,
    /// `None` when the run did not complete within its limits.
    judgement: Option<Judgement>,
    /// Extra context, such as what the other side did when both failed.
    note: Option<String>,
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

async fn judge_test(judge: &Judge, index: usize, test: &Test) -> TestOutcome {
    match &judge.mode {
        Mode::Batch { checker } => judge_batch(judge, checker.as_deref(), index, test).await,
        Mode::Interactive { interactor } => judge_interactive(judge, interactor, index, test).await,
    }
}

/// Runs the solution on one test's input and judges its output.
async fn judge_batch(judge: &Judge, checker: Option<&Path>, index: usize, test: &Test) -> TestOutcome {
    let run = match run_once(&judge.bin_path, [] as [&str; 0], &judge.project_root, test.input.as_bytes(), &judge.limits).await {
        Ok(report) => report.check(&judge.limits),
        Err(e) => Err(e),
    };
    let judgement = match &run {
        Ok(output) => Some(judge_output(judge, checker, index, test, output).await),
        Err(_) => None,
    };
    TestOutcome { run, judgement, note: None }
}

async fn judge_output(judge: &Judge, checker: Option<&Path>, index: usize, test: &Test, output: &RunOutput) -> Judgement {
    let Some(checker_bin) = checker else {
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
        return Judgement::VerifierFailed(format!("could not write files for the checker: {e}"));
    }

    match run_checker(checker_bin, &judge.project_root, &input_file, &output_file, &answer_file).await {
        checker::Verdict::Ok => Judgement::Accepted,
        checker::Verdict::Wrong(message) => Judgement::Wrong(Some(message)),
        checker::Verdict::Presentation(message) => Judgement::Presentation(message),
        checker::Verdict::Failed(message) => Judgement::VerifierFailed(message),
    }
}

/// Runs the solution against the interactor, which gets the test's input as a file.
async fn judge_interactive(judge: &Judge, interactor: &Path, index: usize, test: &Test) -> TestOutcome {
    let input_file = judge.workdir.join(format!("{}.in", index + 1));
    if let Err(e) = tokio::fs::write(&input_file, &test.input).await {
        return TestOutcome {
            run: Ok(no_output(Duration::ZERO, 0)),
            judgement: Some(Judgement::VerifierFailed(format!("could not write the interactor's input file: {e}"))),
            note: None,
        };
    }

    let solution = Program::new(&judge.bin_path);
    let interactor = Program::new(interactor).with_args([&input_file]);
    let verdict = interactive::interact(&solution, &interactor, &judge.project_root, &judge.limits).await;

    let (run, judgement) = match verdict.outcome {
        Outcome::Accepted { cpu_time, max_rss } => (Ok(no_output(cpu_time, max_rss)), Some(Judgement::Accepted)),
        Outcome::Wrong(message) => (Ok(no_output(Duration::ZERO, 0)), Some(Judgement::Wrong(Some(message)))),
        Outcome::Presentation(message) => (Ok(no_output(Duration::ZERO, 0)), Some(Judgement::Presentation(message))),
        Outcome::InteractorFailed(message) => (Ok(no_output(Duration::ZERO, 0)), Some(Judgement::VerifierFailed(message))),
        Outcome::SolutionFailed(error) => (Err(error), None),
    };
    TestOutcome { run, judgement, note: verdict.note }
}

/// A completed run whose output the judge never sees (the interactor consumed it).
fn no_output(cpu_time: Duration, max_rss: u64) -> RunOutput {
    RunOutput { stdout: String::new(), cpu_time, max_rss }
}

/// Prints the result of one test; returns whether it was accepted.
fn print_outcome(test_label: &str, test: &Test, limits: &Limits, verifier: &str, outcome: TestOutcome) -> bool {
    let TestOutcome { run, judgement, note } = outcome;
    let mut accepted = false;
    match (run, judgement) {
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
            accepted = true;
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
            print_verifier_message(verifier, &message);
        }
        (Ok(_), Some(Judgement::Presentation(message))) => {
            println!("{} {}", "❌".red(), format!("{} PE", test_label).red().bold());
            print_verifier_message(verifier, &message);
        }
        (Ok(_), Some(Judgement::VerifierFailed(message))) => {
            println!("{}  {} ({})", "⚠️".red(), format!("{} {} failed", test_label, verifier).red(), message);
        }
        (Ok(_), None) => unreachable!("a completed run is always judged"),
    }
    if let Some(note) = note {
        println!("   {}", note.dimmed());
    }
    accepted
}

fn print_verifier_message(verifier: &str, message: &str) {
    let mut title = verifier.to_string();
    if let Some(first) = title.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    println!("{}", format!("--- {title} ---").blue());
    if message.is_empty() {
        println!("(no message)");
    } else {
        println!("{}", message);
    }
    println!("{}", "-".repeat(title.len() + 8).blue());
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

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// Picks the verifier from the files next to the solution and the problem's `interactive` flag.
fn choose_verifier(src_path: &Path, interactive: bool) -> Result<Verifier, String> {
    let checker = compile::aux_source(src_path, "checker");
    let interactor = compile::aux_source(src_path, "interactor");

    if interactive {
        let Some(interactor) = interactor else {
            return Err(format!(
                "the problem is marked interactive but no interactor was found; expected {}",
                compile::aux_path(src_path, "interactor").display()
            ));
        };
        if let Some(checker) = checker {
            println!("ℹ️  Ignoring checker {}: the interactor decides the verdict of an interactive problem", file_name(&checker));
        }
        return Ok(Verifier::Interactor(interactor));
    }

    if let Some(interactor) = interactor {
        println!(
            "ℹ️  Ignoring interactor {}: the problem is not marked interactive (`\"interactive\": true` in the .prob file)",
            file_name(&interactor)
        );
    }
    Ok(match checker {
        Some(checker) => Verifier::Checker(checker),
        None => Verifier::Builtin,
    })
}

async fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: judge <search_dir> <problem_name>");
        return Ok(ExitCode::from(2));
    }

    let prob_path = find_prob_file(&args[1], &args[2]).ok_or("Metadata file not found.")?;
    // Everything derived from this path (the project root, the binaries inside it) is used
    // as the program path of child processes started in the project root. A relative path
    // would then be resolved against the wrong directory.
    let prob_path = std::path::absolute(&prob_path)?;

    let config: CphConfig = serde_json::from_str(&fs::read_to_string(&prob_path)?)?;
    let project_root = prob_path.parent().and_then(|p| p.parent()).ok_or("Root not found")?;

    let package = compile::package_info(project_root)?;
    let src_path = Path::new(&config.src_path);
    let verifier = choose_verifier(src_path, config.interactive)?;
    if let Some((_, source)) = verifier.bin()
        && !package.autobins
    {
        return Err(format!(
            "found {} but Cargo.toml sets `autobins = false`, so the judge cannot build it from src/bin/; \
             remove that setting or the {}",
            source.display(),
            verifier.label()
        )
        .into());
    }

    fs::copy(src_path, project_root.join("src/main.rs"))?;

    println!("🔨 Compiling {} ({})", package.name.bold(), "release".yellow());
    let mut bins = vec![package.name.as_str()];
    let mut generated: Vec<(&str, &Path)> = Vec::new();
    if let Some((bin, source)) = verifier.bin() {
        println!("🧪 Using {} {}", verifier.label(), file_name(source).bold().cyan());
        bins.push(bin);
        generated.push((bin, source));
    }
    let outcome = {
        // The installed sources are removed when this block ends, whatever the outcome.
        let _installed = compile::GeneratedBins::install(project_root, &generated)?;
        compile::build_release(project_root, &bins)?
    };
    if let BuildOutcome::Failed { log } = outcome {
        eprintln!("{}", "❌ Compilation failed!".red().bold());
        if verifier.bin().is_some() {
            eprintln!("(the solution and the {} are built together; the log shows which one failed)", verifier.label());
        }
        println!("{}", log);
        return Ok(ExitCode::from(1));
    }

    let release_dir = project_root.join("target/release");
    let workdir = project_root.join("target/judge");
    let mode = match &verifier {
        Verifier::Builtin => Mode::Batch { checker: None },
        Verifier::Checker(_) => Mode::Batch { checker: Some(release_dir.join(compile::CHECKER_BIN)) },
        Verifier::Interactor(_) => Mode::Interactive { interactor: release_dir.join(compile::INTERACTOR_BIN) },
    };
    if verifier.bin().is_some() {
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
        mode,
        workdir: workdir.clone(),
    });

    // Time limits are measured in CPU time, but running far more processes than
    // cores still slows each of them down (cache, memory bandwidth, scheduling),
    // so at most one test per core runs at a time.
    let parallelism = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let permits = Arc::new(Semaphore::new(parallelism));

    let total_tests = config.tests.len();
    if total_tests == 0 {
        println!("{}", "⚠️  The .prob file has no tests; nothing will run.".yellow());
        if config.interactive {
            println!("   For an interactive problem, add tests whose input is what the interactor reads (e.g. the hidden number).");
        }
    }

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
        if print_outcome(&test_label, &test, &limits, verifier.label(), outcome) {
            passed_count += 1;
        }
    }

    if passed_count < total_tests {
        match &verifier {
            Verifier::Builtin => {}
            Verifier::Checker(_) => println!(
                "\nℹ️  Checker files for test N are in {}: N.in, N.out (program output), N.ans (stored answer)",
                workdir.display()
            ),
            Verifier::Interactor(_) => println!(
                "\nℹ️  The interactor's input file for test N is {}/N.in",
                workdir.display()
            ),
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

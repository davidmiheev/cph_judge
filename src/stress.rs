//! Stress testing: look for an input on which the solution is wrong.
//!
//! Each iteration asks a generator for a random test of a given size, runs the solution
//! on it, and has an oracle decide: a brute-force solution (whose output is compared with
//! the solution's, or handed to the checker as the jury's answer), or, for interactive
//! problems, the interactor.
//!
//! * The size grows over the run, so small tests (which make the clearest counterexamples)
//!   are tried first; every size gets an equal share of the run.
//! * Iterations run in parallel batches, but the **lowest** failing iteration is the one
//!   reported, so a run is repeatable.
//! * A counterexample is **shrunk**: smaller sizes are searched, with many seeds each, for
//!   a failure of the same kind. This needs nothing from the generator beyond honouring
//!   its size argument.
//! * Problems on the judge's side (a crashing generator or brute force, a checker that
//!   rejects the brute force's own answer) are reported as such, never as counterexamples.

use colored::*;
use std::{
    fs, io,
    ops::Range,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Semaphore, task::JoinSet};

use crate::{
    checker::{self, run_checker},
    compare::compare_outputs,
    generate,
    interactive::{self, Program},
    reduce, report,
    runner::{Limits, RunError, run_once, violation},
    schema::{Domains, Schema},
};
use crate::infer;

pub const DEFAULT_ITERS: u32 = 1000;
pub const DEFAULT_MAX_SIZE: u32 = 20;
pub const DEFAULT_SHRINK_SEEDS: u32 = 200;

/// The generator is only asked for one test; this just stops a broken one from hanging the run.
const GENERATOR_LIMITS: Limits = Limits { time: Duration::from_secs(10), memory_bytes: 1 << 30 };

/// The brute force is slow by nature, so it gets far more than the solution's limit.
fn brute_limits(solution: &Limits) -> Limits {
    Limits {
        time: (solution.time * 10).max(Duration::from_secs(10)),
        memory_bytes: solution.memory_bytes.max(1 << 30),
    }
}

// ---- the plan ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub iters: u32,
    pub base_seed: u64,
    pub max_size: u32,
    pub shrink_seeds: u32,
}

/// The size for an iteration: `1..=max_size` spread evenly over the run.
pub fn size_for(iteration: u32, iters: u32, max_size: u32) -> u32 {
    let i = u64::from(iteration.min(iters.saturating_sub(1)));
    (1 + i * u64::from(max_size) / u64::from(iters.max(1))) as u32
}

pub fn seed_for(base_seed: u64, iteration: u32) -> u64 {
    base_seed.wrapping_add(u64::from(iteration))
}

/// Seeds used while shrinking: unrelated to the run's own, but repeatable.
fn shrink_seed(base_seed: u64, size: u32, k: u32) -> u64 {
    // Distinct (size, k) pairs give distinct inputs to a multiplication by an odd constant,
    // which is a bijection, so no two of them share a seed (for k below 2^24).
    let pair = ((u64::from(size) << 24) | u64::from(k)).wrapping_add(1);
    base_seed ^ 0x9E37_79B9_7F4A_7C15u64.wrapping_mul(pair)
}

// ---- what can go wrong --------------------------------------------------------------

/// A failure of the judge's setup, which says nothing about the solution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    GeneratorFailed(String),
    BruteFailed(String),
    /// The checker rejected the brute force's own answer, so one of the two is wrong.
    BruteRejected(String),
    CheckerFailed(String),
    InteractorFailed(String),
}

/// A failure of the solution: what a counterexample is made of.
#[derive(Debug)]
pub enum Fault {
    /// Time limit, memory limit, crash.
    Run(RunError),
    /// The output was rejected. `expected` is the brute force's answer, `message` what the
    /// checker or interactor said, where there was one.
    WrongAnswer { expected: Option<String>, actual: Option<String>, message: Option<String> },
    Presentation { message: String },
}

#[derive(Debug)]
pub enum Failure {
    Problem(Problem),
    Fault(Fault),
}

/// The kind of a failure. Shrinking looks for a smaller input with the same kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Wrong,
    Presentation,
    Tle,
    Mle,
    Re,
    System,
    Problem,
}

impl Failure {
    pub fn class(&self) -> Class {
        match self {
            Failure::Problem(_) => Class::Problem,
            Failure::Fault(Fault::WrongAnswer { .. }) => Class::Wrong,
            Failure::Fault(Fault::Presentation { .. }) => Class::Presentation,
            Failure::Fault(Fault::Run(RunError::Tle)) => Class::Tle,
            Failure::Fault(Fault::Run(RunError::Mle)) => Class::Mle,
            Failure::Fault(Fault::Run(RunError::Re { .. })) => Class::Re,
            Failure::Fault(Fault::Run(RunError::System(_))) => Class::System,
        }
    }
}

/// The result of one iteration.
pub struct Evaluation {
    pub seed: u64,
    pub size: u32,
    /// The generated test; empty if the generator itself failed.
    pub input: Vec<u8>,
    pub failure: Option<Failure>,
}

// ---- running one iteration ----------------------------------------------------------

/// Where the tests come from.
pub enum Generator {
    /// The user's program, run as `gen <seed> <size>`; it prints one test.
    External(PathBuf),
    /// Made by the judge from the input format inferred from the samples (or given as a hint),
    /// with these constraints.
    Inferred { schema: Schema, domains: Domains },
}

pub enum Oracle {
    Brute { brute: PathBuf, checker: Option<PathBuf> },
    Interactor { interactor: PathBuf },
}

pub struct Env {
    project_root: PathBuf,
    /// Scratch files for the checker and interactor; one set per evaluation.
    workdir: PathBuf,
    solution: PathBuf,
    generator: Generator,
    oracle: Oracle,
    limits: Limits,
    permits: Semaphore,
    counter: AtomicU64,
}

/// What went wrong, with the first lines of a crash message (a panic's location and message;
/// the backtrace after them is not wanted here).
fn explain(error: &RunError) -> String {
    match error {
        RunError::Re { stderr, .. } if !stderr.trim().is_empty() => {
            let first_lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).take(2).collect();
            let snippet: String = first_lines.join(" / ").chars().take(300).collect();
            format!("{}: {}", error.describe(), snippet)
        }
        other => other.describe(),
    }
}

/// Generates the test for `seed` and `size`, runs the solution on it, and judges the result.
pub async fn evaluate(env: &Env, seed: u64, size: u32) -> Evaluation {
    // Evaluations are spawned freely; at most one per core actually runs.
    let _permit = env.permits.acquire().await.expect("semaphore is never closed");

    let input = match make_test(env, seed, size).await {
        Ok(input) => input,
        Err(problem) => {
            return Evaluation { seed, size, input: Vec::new(), failure: Some(Failure::Problem(problem)) };
        }
    };
    let failure = judge(env, &input).await;
    Evaluation { seed, size, input, failure }
}

/// Judges a given test (rather than a generated one). `seed` and `size` are only carried along.
pub async fn evaluate_input(env: &Env, seed: u64, size: u32, input: Vec<u8>) -> Evaluation {
    let _permit = env.permits.acquire().await.expect("semaphore is never closed");
    let failure = judge(env, &input).await;
    Evaluation { seed, size, input, failure }
}

async fn judge(env: &Env, input: &[u8]) -> Option<Failure> {
    match &env.oracle {
        Oracle::Brute { brute, checker } => judge_against_brute(env, input, brute, checker.as_deref()).await,
        Oracle::Interactor { interactor } => judge_by_interactor(env, input, interactor).await,
    }
}

async fn make_test(env: &Env, seed: u64, size: u32) -> Result<Vec<u8>, Problem> {
    let program = match &env.generator {
        Generator::External(program) => program,
        Generator::Inferred { schema, domains } => {
            return generate::generate(schema, domains, seed, size).map(String::into_bytes).map_err(Problem::GeneratorFailed);
        }
    };
    let args = [seed.to_string(), size.to_string()];
    let report = run_once(program, args, &env.project_root, b"", &GENERATOR_LIMITS)
        .await
        .map_err(|e| Problem::GeneratorFailed(explain(&e)))?;
    if let Some(error) = violation(report.status, &report.stderr, report.cpu_time, report.max_rss, &GENERATOR_LIMITS) {
        return Err(Problem::GeneratorFailed(explain(&error)));
    }
    if report.stdout.iter().all(u8::is_ascii_whitespace) {
        return Err(Problem::GeneratorFailed("it printed nothing".to_string()));
    }
    Ok(report.stdout)
}

/// Runs a program on `input` and returns its stdout, or what went wrong.
async fn run_on(program: &Path, input: &[u8], cwd: &Path, limits: &Limits) -> Result<String, RunError> {
    let report = run_once(program, [] as [&str; 0], cwd, input, limits).await?;
    match violation(report.status, &report.stderr, report.cpu_time, report.max_rss, limits) {
        Some(error) => Err(error),
        None => Ok(String::from_utf8_lossy(&report.stdout).into_owned()),
    }
}

async fn judge_against_brute(env: &Env, input: &[u8], brute: &Path, checker: Option<&Path>) -> Option<Failure> {
    let expected = match run_on(brute, input, &env.project_root, &brute_limits(&env.limits)).await {
        Ok(output) => output,
        Err(error) => return Some(Failure::Problem(Problem::BruteFailed(explain(&error)))),
    };
    let actual = match run_on(&env.solution, input, &env.project_root, &env.limits).await {
        Ok(output) => output,
        Err(error) => return Some(Failure::Fault(Fault::Run(error))),
    };

    let Some(checker) = checker else {
        return if compare_outputs(actual.trim(), expected.trim()) {
            None
        } else {
            Some(Failure::Fault(Fault::WrongAnswer { expected: Some(expected), actual: Some(actual), message: None }))
        };
    };

    // The checker works on files: the input, the solution's output and the brute force's.
    let base = env.workdir.join(format!("t{}", env.counter.fetch_add(1, Ordering::Relaxed)));
    let (input_file, output_file, answer_file) = (base.with_extension("in"), base.with_extension("out"), base.with_extension("ans"));
    let written = async {
        tokio::fs::write(&input_file, input).await?;
        tokio::fs::write(&output_file, &actual).await?;
        tokio::fs::write(&answer_file, &expected).await
    }
    .await;
    let failure = match written {
        Err(e) => Some(Failure::Problem(Problem::CheckerFailed(format!("could not write files for the checker: {e}")))),
        Ok(()) => check_with_checker(env, checker, &input_file, &output_file, &answer_file, &expected, &actual).await,
    };
    for file in [input_file, output_file, answer_file] {
        let _ = fs::remove_file(file);
    }
    failure
}

async fn check_with_checker(
    env: &Env,
    checker: &Path,
    input_file: &Path,
    output_file: &Path,
    answer_file: &Path,
    expected: &str,
    actual: &str,
) -> Option<Failure> {
    // A sound checker accepts the jury's own answer. If it does not, the brute force or the
    // checker is wrong, and judging the solution against them would mean nothing.
    match run_checker(checker, &env.project_root, input_file, answer_file, answer_file).await {
        checker::Verdict::Ok => {}
        checker::Verdict::Wrong(m) | checker::Verdict::Presentation(m) => {
            return Some(Failure::Problem(Problem::BruteRejected(m)));
        }
        checker::Verdict::Failed(m) => return Some(Failure::Problem(Problem::CheckerFailed(m))),
    }
    match run_checker(checker, &env.project_root, input_file, output_file, answer_file).await {
        checker::Verdict::Ok => None,
        checker::Verdict::Wrong(message) => Some(Failure::Fault(Fault::WrongAnswer {
            expected: Some(expected.to_string()),
            actual: Some(actual.to_string()),
            message: Some(message),
        })),
        checker::Verdict::Presentation(message) => Some(Failure::Fault(Fault::Presentation { message })),
        checker::Verdict::Failed(m) => Some(Failure::Problem(Problem::CheckerFailed(m))),
    }
}

async fn judge_by_interactor(env: &Env, input: &[u8], interactor: &Path) -> Option<Failure> {
    let input_file = env.workdir.join(format!("t{}.in", env.counter.fetch_add(1, Ordering::Relaxed)));
    if let Err(e) = tokio::fs::write(&input_file, input).await {
        return Some(Failure::Problem(Problem::InteractorFailed(format!("could not write the interactor's input file: {e}"))));
    }
    let solution = Program::new(&env.solution);
    let interactor = Program::new(interactor).with_args([&input_file]);
    let verdict = interactive::interact(&solution, &interactor, &env.project_root, &env.limits).await;
    let _ = fs::remove_file(&input_file);

    use interactive::Outcome;
    match verdict.outcome {
        Outcome::Accepted { .. } => None,
        Outcome::Wrong(message) => Some(Failure::Fault(Fault::WrongAnswer { expected: None, actual: None, message: Some(message) })),
        Outcome::Presentation(message) => Some(Failure::Fault(Fault::Presentation { message })),
        Outcome::SolutionFailed(error) => Some(Failure::Fault(Fault::Run(error))),
        Outcome::InteractorFailed(message) => Some(Failure::Problem(Problem::InteractorFailed(message))),
    }
}

// ---- searching ---------------------------------------------------------------------------

/// What a trial returned, as far as the search is concerned: did it find what it looks for?
pub trait Failing {
    fn failed(&self) -> bool;
}

impl Failing for Evaluation {
    fn failed(&self) -> bool {
        self.failure.is_some()
    }
}

impl Failing for bool {
    fn failed(&self) -> bool {
        *self
    }
}

/// Runs `trial` for every index in `range`, `chunk` at a time (each chunk in parallel), and
/// returns the failure with the lowest index. Stops after the first chunk that has one.
pub async fn first_failure<T, F, Fut>(
    range: Range<u32>,
    chunk: u32,
    mut progress: impl FnMut(u32),
    trial: F,
) -> Option<(u32, T)>
where
    T: Failing + Send + 'static,
    F: Fn(u32) -> Fut,
    Fut: Future<Output = T> + Send + 'static,
{
    let mut start = range.start;
    while start < range.end {
        let end = start.saturating_add(chunk.max(1)).min(range.end);
        let mut tasks = JoinSet::new();
        for index in start..end {
            let future = trial(index);
            tasks.spawn(async move { (index, future.await) });
        }
        let mut lowest: Option<(u32, T)> = None;
        while let Some(joined) = tasks.join_next().await {
            let (index, outcome) = joined.expect("an evaluation task panicked");
            if outcome.failed() && lowest.as_ref().is_none_or(|(best, _)| index < *best) {
                lowest = Some((index, outcome));
            }
        }
        if lowest.is_some() {
            return lowest;
        }
        progress(end);
        start = end;
    }
    None
}

/// Looks for a smaller input that fails in the same way: sizes `1..found_size` in turn, with
/// `plan.shrink_seeds` seeds each. Returns the first (hence smallest) one found.
pub async fn find_smaller<F, Fut>(plan: &Plan, chunk: u32, found_size: u32, class: Class, trial: F) -> Option<Evaluation>
where
    F: Fn(u64, u32) -> Fut,
    Fut: Future<Output = Evaluation> + Send + 'static,
{
    for size in 1..found_size {
        let search = first_failure(0..plan.shrink_seeds, chunk, |_| {}, |k| {
            let future = trial(shrink_seed(plan.base_seed, size, k), size);
            async move {
                let mut evaluation = future.await;
                // Only a failure of the same kind counts; anything else is a different bug.
                if evaluation.failure.as_ref().is_some_and(|f| f.class() != class) {
                    evaluation.failure = None;
                }
                evaluation
            }
        })
        .await;
        if let Some((_, evaluation)) = search {
            return Some(evaluation);
        }
    }
    None
}

/// The most candidate inputs tried while reducing one counterexample.
const MAX_REDUCE_EVALUATIONS: usize = 5000;

pub struct Reduced {
    pub input: String,
    /// How many reducing steps were accepted.
    pub rounds: usize,
}

/// Makes a failing input simpler without leaving the input format or its constraints: fewer
/// repetitions, smaller numbers, simpler strings (see [`reduce`]). `trial` says whether a candidate
/// input still fails in the same way. Returns `None` if the input does not fit the format.
pub async fn reduce_input<F, Fut>(schema: &Schema, domains: &Domains, input: &str, chunk: u32, trial: F) -> Option<Reduced>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = bool> + Send + 'static,
{
    let sample = infer::tokenize(input).ok()?;
    let mut doc = reduce::parse(schema, &sample).ok()?;
    let (mut text, mut rounds, mut evaluated) = (reduce::render(schema, &doc), 0, 0);

    while evaluated < MAX_REDUCE_EVALUATIONS {
        let candidates = reduce::candidates(schema, domains, &doc);
        if candidates.is_empty() {
            break;
        }
        let texts: Vec<String> = candidates.iter().map(|(_, text)| text.clone()).collect();
        let found = first_failure(0..texts.len() as u32, chunk, |_| {}, |i| trial(texts[i as usize].clone())).await;
        match found {
            Some((index, _)) => {
                let tried = ((index / chunk.max(1) + 1) * chunk.max(1)) as usize;
                evaluated += tried.min(texts.len());
                (doc, text) = candidates.into_iter().nth(index as usize).expect("an index of the candidates");
                rounds += 1;
            }
            None => break,
        }
    }
    Some(Reduced { input: text, rounds })
}

// ---- running a whole stress test -------------------------------------------------------------

pub struct Setup {
    pub project_root: PathBuf,
    pub solution: PathBuf,
    pub generator: Generator,
    pub oracle: Oracle,
    pub limits: Limits,
    pub plan: Plan,
    pub prob_path: PathBuf,
    pub save: bool,
}

/// Runs the stress test and prints the result. Exit code: 0 no counterexample, 1 a counterexample,
/// 2 a problem with the judge's own setup.
pub async fn run(setup: Setup) -> io::Result<ExitCode> {
    let Setup { project_root, solution, generator, oracle, limits, plan, prob_path, save } = setup;

    let workdir = project_root.join("target/judge/stress");
    let _ = fs::remove_dir_all(&workdir);
    fs::create_dir_all(&workdir)?;

    let parallelism = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let chunk = (parallelism as u32).saturating_mul(4);
    let verifier = match &oracle {
        Oracle::Brute { checker: Some(_), .. } => "checker",
        Oracle::Brute { checker: None, .. } => "comparison",
        Oracle::Interactor { .. } => "interactor",
    };
    let env = Arc::new(Env {
        project_root,
        workdir: workdir.clone(),
        solution,
        generator,
        oracle,
        limits,
        permits: Semaphore::new(parallelism),
        counter: AtomicU64::new(0),
    });

    println!(
        "🎲 Stress testing: up to {} tests, sizes 1..={}, seed {} (repeat the run with --seed {})",
        plan.iters, plan.max_size, plan.base_seed, plan.base_seed
    );

    let mut last_reported = 0;
    let found = first_failure(
        0..plan.iters,
        chunk,
        |done| {
            if done / 250 > last_reported / 250 {
                println!("   … {done}/{} tests, no counterexample yet", plan.iters);
            }
            last_reported = done;
        },
        |iteration| {
            let env = Arc::clone(&env);
            let (seed, size) = (seed_for(plan.base_seed, iteration), size_for(iteration, plan.iters, plan.max_size));
            async move { evaluate(&env, seed, size).await }
        },
    )
    .await;

    let Some((iteration, first)) = found else {
        println!(
            "{}",
            format!("✅ No counterexample in {} tests (seed {}, sizes 1..={})", plan.iters, plan.base_seed, plan.max_size)
                .green()
                .bold()
        );
        return Ok(ExitCode::SUCCESS);
    };

    let class = first.failure.as_ref().map(Failure::class).expect("a found evaluation has a failure");
    if let Some(Failure::Problem(problem)) = &first.failure {
        print_problem(problem, &first, &env.generator);
        return Ok(ExitCode::from(2));
    }

    // A counterexample. Look for a smaller one of the same kind.
    let mut shrunk_from = None;
    let mut best = first;
    if class != Class::System && best.size > 1 {
        println!("🔍 Counterexample at test #{} (seed {}, size {}); looking for a smaller one...", iteration + 1, best.seed, best.size);
        let trial_env = Arc::clone(&env);
        let smaller = find_smaller(&plan, chunk, best.size, class, move |seed, size| {
            let env = Arc::clone(&trial_env);
            async move { evaluate(&env, seed, size).await }
        })
        .await;
        if let Some(smaller) = smaller {
            shrunk_from = Some(best.size);
            best = smaller;
        }
    }

    // With the input format known, reduce the values too: shorter lengths, smaller numbers.
    let mut reduced_rounds = 0;
    if class != Class::System
        && let Generator::Inferred { schema, domains } = &env.generator
    {
        let (seed, size) = (best.seed, best.size);
        let trial_env = Arc::clone(&env);
        let reduced = reduce_input(schema, domains, &String::from_utf8_lossy(&best.input), chunk, move |text| {
            let env = Arc::clone(&trial_env);
            async move {
                let evaluation = evaluate_input(&env, seed, size, text.into_bytes()).await;
                evaluation.failure.as_ref().is_some_and(|f| f.class() == class)
            }
        })
        .await;
        if let Some(reduced) = reduced.filter(|r| r.rounds > 0) {
            // Run the reduced input once more, to have what the solution and the oracle said about it.
            let again = evaluate_input(&env, seed, size, reduced.input.into_bytes()).await;
            if again.failure.as_ref().is_some_and(|f| f.class() == class) {
                reduced_rounds = reduced.rounds;
                best = again;
            }
        }
    }

    let Some(Failure::Fault(fault)) = &best.failure else { unreachable!("only solution faults are shrunk") };
    print_counterexample(&env, iteration, shrunk_from, reduced_rounds, &best, fault, verifier);
    write_counterexample_files(&workdir, &best, fault)?;
    if save {
        let expected = match fault {
            Fault::WrongAnswer { expected: Some(expected), .. } => expected.as_str(),
            _ => "",
        };
        match save_counterexample(&prob_path, &String::from_utf8_lossy(&best.input), expected) {
            Ok(count) => println!("💾 Saved as test #{count} in {}", prob_path.display()),
            Err(e) => println!("{} could not save the counterexample: {e}", "⚠️".red()),
        }
    }
    Ok(ExitCode::from(1))
}

fn print_problem(problem: &Problem, evaluation: &Evaluation, generator: &Generator) {
    let (title, advice) = match problem {
        Problem::GeneratorFailed(m) => (
            format!("The generator failed (seed {}, size {}): {m}", evaluation.seed, evaluation.size),
            match generator {
                Generator::External(program) => {
                    format!("It is run as `{} <seed> <size>` and must print one test to stdout.", program.display())
                }
                Generator::Inferred { .. } => {
                    "The constraints do not allow a valid input here: change them with `--ask-constraints` or the `vars` hint.".to_string()
                }
            },
        ),
        Problem::BruteFailed(m) => (
            format!("The brute force failed on a generated input: {m}"),
            "Either the generator produced an invalid input, the brute force has a bug, or it is too slow at this size (try a smaller --max-size).".to_string(),
        ),
        Problem::BruteRejected(m) => (
            format!("The checker rejected the brute force's own answer: {m}"),
            "A checker must accept the jury's answer, so either the brute force or the checker is wrong.".to_string(),
        ),
        Problem::CheckerFailed(m) => (format!("The checker failed: {m}"), "Fix the checker; nothing was learned about the solution.".to_string()),
        Problem::InteractorFailed(m) => (format!("The interactor failed: {m}"), "Fix the interactor; nothing was learned about the solution.".to_string()),
    };
    println!("{} {}", "⚠️ ".red(), title.red().bold());
    if !evaluation.input.is_empty() {
        report::print_block("Input", &report::truncate_for_display(&String::from_utf8_lossy(&evaluation.input)));
    }
    println!("{advice}");
    println!("{}", "This is a problem with the setup, not a counterexample.".yellow());
}

fn print_counterexample(
    env: &Env,
    iteration: u32,
    shrunk_from: Option<u32>,
    reduced_rounds: usize,
    found: &Evaluation,
    fault: &Fault,
    verifier: &str,
) {
    match shrunk_from {
        Some(from) => println!(
            "{}",
            format!("❌ Counterexample found, shrunk from size {from} (test #{}) to size {} (seed {})", iteration + 1, found.size, found.seed)
                .red()
                .bold()
        ),
        None => println!(
            "{}",
            format!("❌ Counterexample found at test #{} (seed {}, size {})", iteration + 1, found.seed, found.size).red().bold()
        ),
    }
    if reduced_rounds > 0 {
        println!("   then reduced in {reduced_rounds} step(s): shorter lengths, fewer repetitions, smaller values");
    }
    report::print_block("Input", &report::truncate_for_display(&String::from_utf8_lossy(&found.input)));

    match fault {
        Fault::Run(error) => report::print_run_error("Counterexample", &env.limits, error),
        Fault::Presentation { message } => {
            println!("{} {}", "❌".red(), "Counterexample PE".red().bold());
            report::print_verifier_message(verifier, message);
        }
        Fault::WrongAnswer { expected, actual, message } => {
            println!("{} {}", "❌".red(), "Counterexample WA".red().bold());
            if let Some(message) = message {
                report::print_verifier_message(verifier, message);
            }
            if let (Some(expected), Some(actual)) = (expected, actual) {
                let title = if message.is_some() { "Brute force answer" } else { "Expected (brute force)" };
                report::print_block(title, &report::truncate_for_display(expected));
                report::print_block("Received", &report::truncate_for_display(actual));
            }
        }
    }

    match &env.generator {
        Generator::External(program) => {
            println!("Reproduce the input with: {} {} {}", program.display(), found.seed, found.size);
        }
        Generator::Inferred { .. } => {
            println!("The input was made by the inferred generator (seed {}, size {}).", found.seed, found.size);
        }
    }
    println!(
        "Files: {}/counterexample.in (and .out / .ans where they exist)",
        env.workdir.display()
    );
}

fn write_counterexample_files(workdir: &Path, found: &Evaluation, fault: &Fault) -> io::Result<()> {
    fs::write(workdir.join("counterexample.in"), &found.input)?;
    if let Fault::WrongAnswer { expected, actual, .. } = fault {
        if let Some(actual) = actual {
            fs::write(workdir.join("counterexample.out"), actual)?;
        }
        if let Some(expected) = expected {
            fs::write(workdir.join("counterexample.ans"), expected)?;
        }
    }
    Ok(())
}

/// Appends a test to the `.prob` file's `tests`, giving it an `id` like the editor does.
/// Everything else in the file is kept; the file is replaced atomically. Returns the number
/// of tests afterwards.
pub fn save_counterexample(prob_path: &Path, input: &str, output: &str) -> Result<usize, String> {
    use serde_json::{Value, json};

    let text = fs::read_to_string(prob_path).map_err(|e| e.to_string())?;
    let mut value: Value = serde_json::from_str(&text).map_err(|e| format!("not valid JSON: {e}"))?;
    let tests = value
        .get_mut("tests")
        .and_then(Value::as_array_mut)
        .ok_or("the file has no `tests` array")?;

    let next_id = tests
        .iter()
        .filter_map(|test| test.get("id").and_then(Value::as_u64))
        .max()
        .map(|highest| highest + 1)
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        });
    tests.push(json!({ "id": next_id, "input": input, "output": output }));
    let count = tests.len();

    let temp = prob_path.with_extension("prob.tmp");
    fs::write(&temp, serde_json::to_string(&value).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&temp, prob_path).map_err(|e| e.to_string())?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- size and seeds -----------------------------------------------------------

    #[test]
    fn sizes_ramp_from_one_to_the_maximum() {
        assert_eq!(size_for(0, 1000, 20), 1);
        assert_eq!(size_for(999, 1000, 20), 20);
        let sizes: Vec<u32> = (0..1000).map(|i| size_for(i, 1000, 20)).collect();
        assert!(sizes.windows(2).all(|w| w[0] <= w[1]), "sizes never shrink during a run");
        for size in 1..=20 {
            assert_eq!(sizes.iter().filter(|&&s| s == size).count(), 50, "each size gets an equal share");
        }
    }

    #[test]
    fn sizes_stay_in_range_for_odd_plans() {
        assert_eq!(size_for(0, 1, 20), 1);
        assert_eq!(size_for(0, 1, 1), 1);
        assert!((0..5).all(|i| (1..=20).contains(&size_for(i, 5, 20))));
        // An iteration beyond the plan is clamped rather than overflowing the maximum.
        assert_eq!(size_for(5000, 1000, 20), 20);
    }

    #[test]
    fn seeds_are_consecutive_and_wrap() {
        assert_eq!(seed_for(10, 0), 10);
        assert_eq!(seed_for(10, 5), 15);
        assert_eq!(seed_for(u64::MAX, 2), 1);
    }

    #[test]
    fn shrink_seeds_are_repeatable_and_mostly_distinct() {
        assert_eq!(shrink_seed(7, 3, 4), shrink_seed(7, 3, 4));
        let seeds: std::collections::HashSet<u64> = (1..=5).flat_map(|size| (0..200).map(move |k| shrink_seed(7, size, k))).collect();
        assert_eq!(seeds.len(), 1000);
    }

    // ---- failure classes ----------------------------------------------------------

    fn wrong() -> Failure {
        Failure::Fault(Fault::WrongAnswer { expected: None, actual: None, message: None })
    }

    #[test]
    fn failures_are_classified_by_kind() {
        assert_eq!(wrong().class(), Class::Wrong);
        assert_eq!(Failure::Fault(Fault::Presentation { message: String::new() }).class(), Class::Presentation);
        assert_eq!(Failure::Fault(Fault::Run(RunError::Tle)).class(), Class::Tle);
        assert_eq!(Failure::Fault(Fault::Run(RunError::Mle)).class(), Class::Mle);
        assert_eq!(Failure::Fault(Fault::Run(RunError::System("x".into()))).class(), Class::System);
        assert_eq!(Failure::Problem(Problem::BruteFailed("x".into())).class(), Class::Problem);
    }

    // ---- first_failure -------------------------------------------------------------

    fn evaluation(index: u32, fails: bool) -> Evaluation {
        Evaluation { seed: u64::from(index), size: 1, input: Vec::new(), failure: fails.then(wrong) }
    }

    #[tokio::test]
    async fn no_failures_means_none_and_progress_is_reported() {
        let mut reported = Vec::new();
        let found = first_failure(0..100, 16, |done| reported.push(done), |i| async move { evaluation(i, false) }).await;
        assert!(found.is_none());
        assert_eq!(reported, vec![16, 32, 48, 64, 80, 96, 100]);
    }

    #[tokio::test]
    async fn the_lowest_failing_index_wins_even_if_a_later_one_finishes_first() {
        // 37 is slow, 40 is quick and in the same chunk: the lower index must still be reported.
        let found = first_failure(0..100, 16, |_| {}, |i| async move {
            if i == 37 {
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
            evaluation(i, i == 37 || i == 40)
        })
        .await;
        assert_eq!(found.map(|(i, _)| i), Some(37));
    }

    #[tokio::test]
    async fn the_search_stops_after_the_chunk_with_the_first_failure() {
        let started = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&started);
        let found = first_failure(0..1000, 10, |_| {}, move |i| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move { evaluation(i, i == 25) }
        })
        .await;
        assert_eq!(found.map(|(i, _)| i), Some(25));
        assert_eq!(started.load(Ordering::SeqCst), 30, "chunks 0-9, 10-19 and 20-29 only");
    }

    #[tokio::test]
    async fn a_chunk_of_zero_still_makes_progress() {
        let found = first_failure(0..5, 0, |_| {}, |i| async move { evaluation(i, i == 3) }).await;
        assert_eq!(found.map(|(i, _)| i), Some(3));
    }

    // ---- find_smaller ----------------------------------------------------------------

    fn plan() -> Plan {
        Plan { iters: 100, base_seed: 1, max_size: 20, shrink_seeds: 30 }
    }

    /// A pretend solution that fails only for sizes of at least 4, and then on a third of seeds.
    fn fails_from_size_4(seed: u64, size: u32) -> Evaluation {
        Evaluation { seed, size, input: Vec::new(), failure: (size >= 4 && seed.is_multiple_of(3)).then(wrong) }
    }

    #[tokio::test]
    async fn shrinking_finds_the_smallest_failing_size() {
        let smaller = find_smaller(&plan(), 8, 15, Class::Wrong, |seed, size| async move { fails_from_size_4(seed, size) }).await;
        let smaller = smaller.expect("sizes 4.. fail");
        assert_eq!(smaller.size, 4);
        assert!(smaller.seed.is_multiple_of(3));
    }

    #[tokio::test]
    async fn shrinking_returns_none_when_nothing_smaller_fails() {
        // Already at the smallest failing size.
        assert!(find_smaller(&plan(), 8, 4, Class::Wrong, |seed, size| async move { fails_from_size_4(seed, size) }).await.is_none());
        // And nothing to look at for size 1.
        assert!(find_smaller(&plan(), 8, 1, Class::Wrong, |seed, size| async move { fails_from_size_4(seed, size) }).await.is_none());
    }

    #[tokio::test]
    async fn shrinking_only_accepts_the_same_kind_of_failure() {
        // Smaller inputs crash instead of answering wrongly: a different bug, so not a shrink of this one.
        let crashing = |seed: u64, size: u32| async move {
            let failure = if size < 8 { Failure::Fault(Fault::Run(RunError::Tle)) } else { wrong() };
            Evaluation { seed, size, input: Vec::new(), failure: Some(failure) }
        };
        let smaller = find_smaller(&plan(), 8, 12, Class::Wrong, crashing).await.expect("size 8 fails the same way");
        assert_eq!(smaller.size, 8);
    }

    #[tokio::test]
    async fn a_judge_side_problem_at_a_smaller_size_is_not_a_shrink() {
        let broken_generator = |seed: u64, size: u32| async move {
            let failure = if size < 5 { Failure::Problem(Problem::GeneratorFailed("x".into())) } else { wrong() };
            Evaluation { seed, size, input: Vec::new(), failure: Some(failure) }
        };
        let smaller = find_smaller(&plan(), 8, 12, Class::Wrong, broken_generator).await.expect("size 5 fails");
        assert_eq!(smaller.size, 5);
    }

    // ---- reduce_input -------------------------------------------------------------------------

    fn schema_of(lines: &[&str]) -> Schema {
        Schema::parse(lines).unwrap()
    }

    fn domains_of(pairs: &[(&str, &str)]) -> Domains {
        pairs.iter().map(|(n, t)| (n.to_string(), crate::schema::Spec::parse(t).unwrap())).collect()
    }

    /// The numbers in a text, line by line.
    fn rows(text: &str) -> Vec<Vec<i64>> {
        text.lines().map(|l| l.split_whitespace().map(|t| t.parse().unwrap()).collect()).collect()
    }

    async fn reduced(schema: &Schema, domains: &Domains, input: &str, fails: fn(&str) -> bool) -> Reduced {
        reduce_input(schema, domains, input, 4, move |text| async move { fails(&text) }).await.expect("the input fits")
    }

    #[tokio::test]
    async fn an_input_shrinks_to_the_least_that_still_fails() {
        // Fails when there are at least three numbers: the shortest and simplest is three ones.
        let (s, d) = (schema_of(&["n", "a[n]"]), domains_of(&[("n", "1..size"), ("a", "1..9")]));
        let r = reduced(&s, &d, "7\n9 8 7 6 5 4 3\n", |t| rows(t)[0][0] >= 3).await;
        assert_eq!(r.input, "3\n1 1 1\n");
        assert!(r.rounds > 0);
    }

    #[tokio::test]
    async fn values_stop_at_the_smallest_that_still_fails() {
        // Fails when some number is at least 4: one number, and exactly 4.
        let (s, d) = (schema_of(&["n", "a[n]"]), domains_of(&[("n", "1..size"), ("a", "1..9")]));
        let r = reduced(&s, &d, "5\n9 1 2 8 3\n", |t| rows(t)[1].iter().any(|&v| v >= 4)).await;
        assert_eq!(r.input, "1\n4\n");
    }

    #[tokio::test]
    async fn the_constraints_are_respected_all_the_way_down() {
        // The numbers must be 5 to 9, so the least is 5, never 0 or 1.
        let (s, d) = (schema_of(&["n", "a[n]"]), domains_of(&[("n", "2..size"), ("a", "5..9")]));
        let r = reduced(&s, &d, "4\n9 8 7 6\n", |t| rows(t)[1].iter().sum::<i64>() >= 10).await;
        assert_eq!(r.input, "2\n5 5\n");
    }

    #[tokio::test]
    async fn a_failing_test_can_be_pulled_out_of_a_multitest() {
        // Only a test with n = 2 fails; the other tests of the file are dropped, wherever it is.
        let (s, d) = (schema_of(&["t", "repeat t {", "  n", "  a[n]", "}"]), domains_of(&[("t", "1..5"), ("n", "1..size"), ("a", "1..9")]));
        let r = reduced(&s, &d, "4\n1\n5\n3\n6 7 8\n2\n9 9\n1\n4\n", |t| {
            let lines: Vec<&str> = t.lines().collect();
            lines.iter().skip(1).step_by(2).any(|l| l.trim() == "2")
        })
        .await;
        assert_eq!(r.input, "1\n2\n1 1\n");
    }

    #[tokio::test]
    async fn an_input_that_cannot_be_simpler_is_returned_as_it_is() {
        let (s, d) = (schema_of(&["n", "a[n]"]), domains_of(&[("n", "1..size"), ("a", "1..9")]));
        let r = reduced(&s, &d, "1\n1\n", |_| true).await;
        assert_eq!((r.input.as_str(), r.rounds), ("1\n1\n", 0));
    }

    #[tokio::test]
    async fn a_reduction_that_would_stop_failing_is_not_taken() {
        // Fails only for exactly 5 numbers: nothing shorter qualifies, and values may all drop.
        let (s, d) = (schema_of(&["n", "a[n]"]), domains_of(&[("n", "1..size"), ("a", "1..9")]));
        let r = reduced(&s, &d, "5\n9 8 7 6 5\n", |t| rows(t)[0][0] == 5).await;
        assert_eq!(r.input, "5\n1 1 1 1 1\n");
    }

    #[tokio::test]
    async fn a_permutation_stays_one() {
        let (s, d) = (schema_of(&["n", "p[n]"]), domains_of(&[("n", "1..size"), ("p", "perm")]));
        let r = reduced(&s, &d, "6\n4 6 2 5 1 3\n", |t| rows(t)[0][0] >= 3).await;
        let numbers = &rows(&r.input)[1];
        let mut sorted = numbers.clone();
        sorted.sort();
        assert_eq!(sorted, (1..=numbers.len() as i64).collect::<Vec<_>>(), "{}", r.input);
        assert_eq!(numbers.len(), 3);
    }

    #[tokio::test]
    async fn an_input_that_does_not_fit_the_format_is_not_reduced() {
        let (s, d) = (schema_of(&["n", "a[n]"]), domains_of(&[("n", "1..size"), ("a", "1..9")]));
        assert!(reduce_input(&s, &d, "3\n1 2\n", 4, |_| async { true }).await.is_none());
    }

    // ---- explain ----------------------------------------------------------------------

    #[test]
    fn explain_includes_a_crash_message_but_keeps_it_short() {
        use std::os::unix::process::ExitStatusExt;
        let status = std::process::ExitStatus::from_raw(101 << 8);
        let long = RunError::Re { status, stderr: format!("boom {}", "x".repeat(1000)) };
        let text = explain(&long);
        assert!(text.starts_with("runtime error (exit code 101): boom"), "{text}");
        assert!(text.chars().count() < 400, "{}", text.chars().count());
        // Only the first two lines of a multi-line crash message are kept.
        let panic = RunError::Re {
            status,
            stderr: "thread 'main' panicked at a.rs:1:2:\nbrute exploded\nstack backtrace:\n   0: rust_begin_unwind\n".into(),
        };
        assert_eq!(
            explain(&panic),
            "runtime error (exit code 101): thread 'main' panicked at a.rs:1:2: / brute exploded"
        );
        assert_eq!(explain(&RunError::Tle), "time limit exceeded");
        assert_eq!(explain(&RunError::Re { status, stderr: "  \n".into() }), "runtime error (exit code 101)");
    }

    // ---- save_counterexample ----------------------------------------------------------------

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cph_judge_stress_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saving_appends_a_test_and_keeps_everything_else() {
        let dir = scratch("save");
        let prob = dir.join("x.prob");
        fs::write(
            &prob,
            r#"{"name":"A","url":"https://example.com/a","interactive":false,"timeLimit":1000,"group":"g",
                "tests":[{"id":7,"input":"1\n","output":"2\n"},{"id":3,"input":"5\n","output":"6\n"}]}"#,
        )
        .unwrap();

        let count = save_counterexample(&prob, "3\n-4\n", "-4\n").unwrap();
        assert_eq!(count, 3);

        let saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(&prob).unwrap()).unwrap();
        assert_eq!(saved["name"], "A");
        assert_eq!(saved["url"], "https://example.com/a");
        assert_eq!(saved["group"], "g");
        assert_eq!(saved["tests"].as_array().unwrap().len(), 3);
        assert_eq!(saved["tests"][0]["input"], "1\n", "existing tests are untouched");
        assert_eq!(saved["tests"][2]["input"], "3\n-4\n");
        assert_eq!(saved["tests"][2]["output"], "-4\n");
        assert_eq!(saved["tests"][2]["id"], 8, "one above the highest existing id");
        assert!(!dir.join("x.prob.tmp").exists(), "the temporary file is gone");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_into_a_file_without_ids_still_gives_the_test_one() {
        let dir = scratch("save_no_ids");
        let prob = dir.join("x.prob");
        fs::write(&prob, r#"{"tests":[]}"#).unwrap();
        save_counterexample(&prob, "1\n", "1\n").unwrap();
        let saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(&prob).unwrap()).unwrap();
        assert!(saved["tests"][0]["id"].as_u64().is_some_and(|id| id > 0), "{saved}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_refuses_a_file_it_cannot_understand_and_leaves_it_alone() {
        let dir = scratch("save_bad");
        let prob = dir.join("x.prob");
        fs::write(&prob, r#"{"name":"A"}"#).unwrap();
        assert!(save_counterexample(&prob, "1", "1").unwrap_err().contains("tests"));
        assert_eq!(fs::read_to_string(&prob).unwrap(), r#"{"name":"A"}"#);
        fs::write(&prob, "not json").unwrap();
        assert!(save_counterexample(&prob, "1", "1").unwrap_err().contains("JSON"));
        assert_eq!(fs::read_to_string(&prob).unwrap(), "not json");
        fs::remove_dir_all(&dir).unwrap();
    }
}

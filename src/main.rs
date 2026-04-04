use colored::*; // 1. Import the trait
use ignore::WalkBuilder;
use serde::Deserialize;
use std::{
    env, fs, 
    io::{Read, Write},
    path::{Path, PathBuf}, 
    process::{Command, Output, Stdio},
    time::{Duration, Instant}, 
};
use tokio::time::timeout;
use wait4::Wait4;

#[derive(Deserialize)]
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
struct Test { input: String, output: String }

#[derive(Deserialize)]
struct CargoToml { package: Package }
#[derive(Deserialize)]
struct Package { name: String }

async fn run_with_timeout(
    bin_path: &Path,
    project_root: &Path,
    input: &str,
    timeout_duration: Duration,
    memory_limit_bytes: u64,
) -> Result<(std::process::Output, u64), &'static str> {
    let mut child = Command::new(bin_path)
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn process");

    let pid = child.id();
    let input_owned = input.as_bytes().to_vec();

    let run_task = tokio::task::spawn_blocking(move || -> Result<(Output, u64), &'static str> {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&input_owned);
        }

        let mut stdout_pipe = child.stdout.take().ok_or("RTE")?;
        let read_stdout = std::thread::spawn(move || {
            let mut stdout = Vec::new();
            let _ = stdout_pipe.read_to_end(&mut stdout);
            stdout
        });

        let res_use = child.wait4().map_err(|_| "RTE")?;
        let stdout = read_stdout.join().unwrap_or_default();

        let output = Output {
            status: res_use.status,
            stdout,
            stderr: Vec::new(),
        };

        Ok((output, res_use.rusage.maxrss))
    });

    match timeout(timeout_duration, run_task).await {
        Ok(Ok(Ok((output, max_mem)))) => {
            if max_mem > memory_limit_bytes {
                return Err("MLE");
            }
            Ok((output, max_mem))
        }
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(_)) => Err("RTE"),
        Err(_) => {
            kill_process_tree(pid);
            Err("TLE")
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: judge <search_dir> <problem_name>");
        std::process::exit(1);
    }

    let prob_path = find_prob_file(&args[1], &args[2])
        .ok_or("Metadata file not found.")?;
    
    let config: CphConfig = serde_json::from_str(&fs::read_to_string(&prob_path)?)?;
    let project_root = prob_path.parent().and_then(|p| p.parent()).ok_or("Root not found")?;
    
    fs::copy(&config.src_path, project_root.join("src/main.rs"))?;

    let cargo_contents = fs::read_to_string(project_root.join("Cargo.toml"))?;
    let cargo_toml: CargoToml = toml::from_str(&cargo_contents)?;
    let bin_name = cargo_toml.package.name;

    println!("🔨 Compiling {} ({})", bin_name.bold(), "release".yellow());
    let log_file = fs::File::create(project_root.join("compiler_log.txt"))?;
    
    let status = std::process::Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(project_root)
        .stderr(log_file)
        .status()?;

    if !status.success() {
        eprintln!("{}", "❌ Compilation failed!".red().bold());
        let log_content = fs::read_to_string(project_root.join("compiler_log.txt"))?;
        println!("{}", log_content);
        return Ok(());
    }

    let bin_path = project_root.join("target/release").join(&bin_name);
    let mut passed_count = 0;
    let total_tests = config.tests.len();

    let mut handles = Vec::new();
    let default_timeout = Duration::from_secs(3);
    let time_limit = config.time_limit.map(Duration::from_millis).unwrap_or(default_timeout);
    let memory_limit_bytes = config.memory_limit.unwrap_or(256) * 1024 * 1024; // MB to Bytes

    for (i, test) in config.tests.into_iter().enumerate() {
        let bin_path = bin_path.clone();
        let project_root = project_root.to_path_buf();
        
        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let output_result = run_with_timeout(&bin_path, &project_root, &test.input, time_limit, memory_limit_bytes).await;
            (i, test, start.elapsed(), output_result)
        }));
    }

    for handle in handles {
        let (i, test, duration, output_result) = handle.await.expect("Failed to join task");
        
        let test_label = format!("Test #{}", i + 1);
        
        match output_result {
            Err("TLE") => {
                let limit_label = format!("{:.2}s", time_limit.as_secs_f32());
                println!("{}  {} ({})", "⏱️".yellow(), format!("{} TLE", test_label).yellow(), limit_label);
            }
            Err("MLE") => {
                let limit_label = format!("{} MB", memory_limit_bytes / 1024 / 1024);
                println!("{}  {} ({})", "💾".yellow(), format!("{} MLE", test_label).yellow(), limit_label);
            }
            Err(e) => {
                println!("{}  {} ({})", "⚠️".red(), format!("{} Error", test_label).red(), e);
            }
            Ok((output, max_mem)) => {
                let actual = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let expected = test.output.trim().to_string();
                let mem_mb = max_mem as f64 / 1048576.0;

                if actual == expected {
                    println!("{} {} ({:?}, {:.2} MB)", "✅".green(), test_label.green(), duration, mem_mb);
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
    } else {
        println!("{}", final_text.yellow().bold());
    }

    Ok(())
}

//! Terminal output shared by the normal judge and stress mode.

use colored::*;

use crate::runner::{Limits, RunError, format_exit_status};

/// Prints the verdict line for a run that broke a limit or crashed: TLE, MLE, RE with the
/// crash trace, or a system error. `label` is what the run is called, e.g. `Test #3`.
pub fn print_run_error(label: &str, limits: &Limits, error: &RunError) {
    match error {
        RunError::Tle => {
            let limit_label = format!("{:.2}s", limits.time.as_secs_f32());
            println!("{}  {} ({})", "⏱️".yellow(), format!("{} TLE", label).yellow(), limit_label);
        }
        RunError::Mle => {
            let limit_label = format!("{} MB", limits.memory_bytes / 1024 / 1024);
            println!("{}  {} ({})", "💾".yellow(), format!("{} MLE", label).yellow(), limit_label);
        }
        RunError::Re { status, stderr } => {
            let status_desc = format_exit_status(*status);
            println!("{}  {} ({})", "💥".red(), format!("{} RE", label).red().bold(), status_desc.yellow());
            let stderr_trimmed = stderr.trim();
            if !stderr_trimmed.is_empty() {
                println!("{}", "--- Stderr / Crash Trace ---".magenta());
                println!("{}", stderr_trimmed);
                println!("{}", "----------------------------".magenta());
            }
        }
        RunError::System(e) => {
            println!("{}  {} ({})", "⚠️".red(), format!("{} Error", label).red(), e);
        }
    }
}

/// A titled block of text, e.g. the message of a checker or interactor.
pub fn print_block(title: &str, body: &str) {
    println!("{}", format!("--- {title} ---").blue());
    if body.is_empty() {
        println!("(no message)");
    } else {
        println!("{}", body);
    }
    println!("{}", "-".repeat(title.chars().count() + 8).blue());
}

/// `checker` -> a block titled `Checker`.
pub fn print_verifier_message(verifier: &str, message: &str) {
    let mut title = verifier.to_string();
    if let Some(first) = title.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    print_block(&title, message);
}

const MAX_LINES: usize = 40;
const MAX_CHARS: usize = 2000;

/// Cuts a long text down to what is worth printing, saying how much was left out.
pub fn truncate_for_display(text: &str) -> String {
    let text = text.trim_end();
    let total_lines = text.lines().count();
    let mut shown = String::new();
    let mut shown_lines = 0;
    let mut cut_inside_a_line = false;

    for line in text.lines() {
        let used = shown.chars().count();
        if shown_lines == MAX_LINES || used >= MAX_CHARS {
            break;
        }
        let budget = MAX_CHARS - used;
        if line.chars().count() > budget {
            shown.extend(line.chars().take(budget));
            shown.push('…');
            shown_lines += 1;
            cut_inside_a_line = true;
            break;
        }
        shown.push_str(line);
        shown.push('\n');
        shown_lines += 1;
    }

    let shown = shown.trim_end_matches('\n').to_string();
    if shown_lines < total_lines || cut_inside_a_line {
        let left_out = total_lines - shown_lines;
        if left_out > 0 {
            format!("{shown}\n… ({left_out} more line{} not shown)", if left_out == 1 { "" } else { "s" })
        } else {
            shown
        }
    } else {
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_unchanged_apart_from_trailing_newlines() {
        assert_eq!(truncate_for_display("1 2\n3 4\n\n"), "1 2\n3 4");
        assert_eq!(truncate_for_display(""), "");
    }

    #[test]
    fn many_lines_are_cut_with_a_count_of_what_is_left_out() {
        let text: String = (1..=100).map(|i| format!("{i}\n")).collect();
        let shown = truncate_for_display(&text);
        assert!(shown.starts_with("1\n2\n3\n"), "{shown}");
        assert!(shown.contains("\n40\n"), "{shown}");
        assert!(!shown.contains("\n41\n"), "{shown}");
        assert!(shown.ends_with("… (60 more lines not shown)"), "{shown}");
    }

    #[test]
    fn a_single_left_out_line_is_not_pluralised() {
        let text: String = (1..=41).map(|i| format!("{i}\n")).collect();
        assert!(truncate_for_display(&text).ends_with("… (1 more line not shown)"));
    }

    #[test]
    fn a_very_long_line_is_cut_inside_the_line() {
        let shown = truncate_for_display(&"x".repeat(10_000));
        assert_eq!(shown.chars().count(), MAX_CHARS + 1);
        assert!(shown.ends_with('…'), "{shown}");
    }

    #[test]
    fn many_medium_lines_are_limited_by_characters_too() {
        let text: String = (0..30).map(|_| format!("{}\n", "y".repeat(200))).collect();
        let shown = truncate_for_display(&text);
        assert!(shown.chars().count() < MAX_CHARS + 100, "{}", shown.chars().count());
        assert!(shown.contains("more line"), "{shown}");
    }
}

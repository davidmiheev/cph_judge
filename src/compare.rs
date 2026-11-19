/// Compares actual output against expected output token by token.
/// Floating-point numbers are compared with relative and absolute epsilon tolerance (1e-6).
pub fn compare_outputs(actual: &str, expected: &str) -> bool {
    let actual_tokens: Vec<&str> = actual.split_whitespace().collect();
    let expected_tokens: Vec<&str> = expected.split_whitespace().collect();

    if actual_tokens.len() != expected_tokens.len() {
        return false;
    }

    const EPS: f64 = 1e-6;

    for (a, e) in actual_tokens.iter().zip(expected_tokens.iter()) {
        if a == e {
            continue;
        }

        match (a.parse::<f64>(), e.parse::<f64>()) {
            (Ok(a_num), Ok(e_num)) if a_num.is_finite() && e_num.is_finite() => {
                let diff = (a_num - e_num).abs();
                if diff > EPS && diff > EPS * e_num.abs() {
                    return false;
                }
            }
            _ => return false,
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        assert!(compare_outputs("hello world", "hello world"));
        assert!(compare_outputs("42 100", "42 100"));
        assert!(!compare_outputs("hello world", "hello there"));
    }

    #[test]
    fn test_whitespace_insensitivity() {
        assert!(compare_outputs("1  2 \n 3", "1 2 3\n"));
        assert!(compare_outputs("  a   b  ", "a b"));
    }

    #[test]
    fn test_float_comparison() {
        // Equal numbers with different precision formatting
        assert!(compare_outputs("3.141590", "3.14159"));
        assert!(compare_outputs("1.0", "1"));
        assert!(compare_outputs("0.500000", "0.5"));

        // Within 1e-6 epsilon
        assert!(compare_outputs("3.14159265", "3.14159270"));

        // Large numbers with relative error within 1e-6
        assert!(compare_outputs("1000000.5", "1000000.0"));

        // Exceeding 1e-6 epsilon
        assert!(!compare_outputs("3.14159", "3.14160"));
        assert!(!compare_outputs("1.0", "1.0001"));
    }

    #[test]
    fn test_mixed_tokens() {
        assert!(compare_outputs("Case #1: 3.14159265 42", "Case #1: 3.14159270 42"));
        assert!(!compare_outputs("Case #1: 3.14159265 42", "Case #2: 3.14159265 42"));
    }

    #[test]
    fn test_token_length_mismatch() {
        assert!(!compare_outputs("1 2 3", "1 2"));
        assert!(!compare_outputs("1 2", "1 2 3"));
    }
}

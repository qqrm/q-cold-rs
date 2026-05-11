pub const DEFAULT_PROMPT_SNIPPET_LINES: usize = 5;
pub const DEFAULT_PROMPT_SNIPPET_CHARS: usize = 600;

pub fn prompt_snippet(value: &str) -> String {
    prompt_snippet_with_limits(
        value,
        DEFAULT_PROMPT_SNIPPET_LINES,
        DEFAULT_PROMPT_SNIPPET_CHARS,
    )
}

pub fn prompt_snippet_with_limits(value: &str, max_lines: usize, max_chars: usize) -> String {
    let max_lines = max_lines.max(1);
    let max_chars = max_chars.max(1);
    let mut text = value
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n");
    truncate_chars(&mut text, max_chars);
    text
}

pub fn truncate_chars(text: &mut String, max_chars: usize) {
    if text.chars().count() <= max_chars {
        return;
    }
    if max_chars <= 3 {
        *text = ".".repeat(max_chars);
        return;
    }
    let mut truncated = text.chars().take(max_chars.saturating_sub(3)).collect::<String>();
    truncated.push_str("...");
    *text = truncated;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_snippet_keeps_first_non_empty_lines() {
        assert_eq!(
            prompt_snippet_with_limits(" first task \n\n second   task \n third \n fourth", 2, 80),
            "first task\nsecond task"
        );
    }

    #[test]
    fn prompt_snippet_truncates_by_chars() {
        assert_eq!(prompt_snippet_with_limits("abcdef", 5, 5), "ab...");
        assert_eq!(prompt_snippet_with_limits("abcdef", 5, 2), "..");
    }
}

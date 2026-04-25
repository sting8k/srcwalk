use std::fmt::Write;

/// Unknown file types: preview only; default reads should not become `cat`.
pub fn head_tail(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if total == 0 {
        return String::new();
    }

    let shown = total.min(20);
    let mut result = lines[..shown].join("\n");
    let omitted = total.saturating_sub(shown);
    let _ = write!(
        result,
        "\n\n... preview: {total} lines total, {omitted} omitted. Use --full or --section for raw content."
    );
    result
}

/// Log files: preview only; default reads should not become `cat`.
pub fn log_view(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if total == 0 {
        return String::new();
    }

    let shown = total.min(10);
    let mut result = lines[..shown].join("\n");
    let omitted = total.saturating_sub(shown);
    let _ = write!(
        result,
        "\n\n... log preview: {total} lines total, {omitted} omitted. Use --full or --section for raw content."
    );
    result
}

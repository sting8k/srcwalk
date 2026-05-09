use std::collections::HashSet;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use crate::format::rel_nonempty;
use crate::read::RAW_TOKEN_CAP;
use crate::types::{estimate_tokens, Match};

const EXPAND_FULL_FILE_THRESHOLD: u64 = 800;

pub(crate) struct ExpandBudget {
    pub(super) cap_tokens: u64,
    pub(super) remaining_tokens: u64,
    pub(super) expanded: usize,
    pub(super) omitted: usize,
}

impl ExpandBudget {
    pub(crate) fn new(expand: usize, budget_tokens: Option<u64>) -> Self {
        let default = RAW_TOKEN_CAP / 2;
        let remaining_tokens =
            budget_tokens.map_or(default, |budget| budget.saturating_mul(7) / 10);
        let cap_tokens = if expand == 0 { 0 } else { remaining_tokens };
        Self {
            cap_tokens,
            remaining_tokens: cap_tokens,
            expanded: 0,
            omitted: 0,
        }
    }

    pub(super) fn try_consume(&mut self, text: &str) -> bool {
        let tokens = estimate_tokens(text.len() as u64).max(1);
        if tokens > self.remaining_tokens {
            self.omitted += 1;
            return false;
        }
        self.remaining_tokens -= tokens;
        self.expanded += 1;
        true
    }
}

pub(crate) fn append_expand_budget_note(out: &mut String, budget: &ExpandBudget) {
    if budget.omitted == 0 {
        return;
    }
    let expanded = budget.expanded;
    let omitted = budget.omitted;
    let used = budget.cap_tokens.saturating_sub(budget.remaining_tokens);
    let cap = budget.cap_tokens;
    let _ = write!(
        out,
        "\n\n> Note: expand cap ~{used}/{cap} tokens; expanded {expanded}, omitted {omitted}.\n> Next: drill into omitted hits with `srcwalk <path>:<line>` or `srcwalk <path> --section <symbol|range>`."
    );
}

pub(super) fn expand_match(m: &Match, scope: &Path) -> Option<(String, String)> {
    let content = fs::read_to_string(&m.path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len() as u32;

    let (mut start, end) = if estimate_tokens(content.len() as u64) < EXPAND_FULL_FILE_THRESHOLD {
        (1, total)
    } else {
        let (s, e) = m
            .def_range
            .unwrap_or((m.line.saturating_sub(10), m.line.saturating_add(10)));
        (s.max(1), e.min(total))
    };

    // Skip leading import blocks in expanded definitions near top of file
    if m.is_definition && start <= 5 {
        let mut first_non_import = start;
        for i in start..=end {
            let idx = (i - 1) as usize;
            if idx >= lines.len() {
                break;
            }
            let trimmed = lines[idx].trim();
            let is_import = trimmed.starts_with("use ")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("from ")
                || trimmed.starts_with("#include")
                || trimmed.starts_with("require(")
                || trimmed.starts_with("require ")
                || (trimmed.starts_with("const ") && trimmed.contains("= require("));

            if !is_import && !trimmed.is_empty() {
                first_non_import = i;
                break;
            }
        }
        // Guard: only skip if we found at least one non-import line
        if first_non_import > start && first_non_import <= end {
            start = first_non_import;
        }
    }

    let mut out = String::new();
    let _ = write!(
        out,
        "\n```{}:{}-{}",
        rel_nonempty(&m.path, scope),
        start,
        end
    );

    // Track consecutive blank lines for collapsing
    let mut prev_blank = false;
    for i in start..=end {
        let idx = (i - 1) as usize;
        if idx < lines.len() {
            let line = lines[idx];
            let is_blank = line.trim().is_empty();

            // Skip consecutive blank lines (keep first, drop rest)
            if is_blank && prev_blank {
                continue;
            }

            let _ = write!(out, "\n{i:>4} │ {line}");
            prev_blank = is_blank;
        }
    }
    out.push_str("\n```");
    Some((out, content))
}

/// Filter formatted code lines using a set of line numbers to skip.
/// Input is the fenced code block from `expand_match` (opening/closing fence lines
/// plus numbered content lines). Inserts gap markers for runs of >3 skipped lines.
pub(super) fn filter_code_lines(code: &str, skip_lines: &HashSet<u32>) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut consecutive_skipped: u32 = 0;

    for segment in code.split('\n') {
        // Fence lines and the leading empty segment pass through unchanged
        if segment.starts_with("```") || segment.is_empty() {
            flush_gap_marker(&mut kept, &mut consecutive_skipped);
            kept.push(segment.to_owned());
            continue;
        }

        // Extract line number from formatted line: "  42 │ content"
        let line_num = segment
            .find('│')
            .and_then(|pos| segment[..pos].trim().parse::<u32>().ok());

        if let Some(num) = line_num {
            if skip_lines.contains(&num) {
                consecutive_skipped += 1;
                continue;
            }
        }

        flush_gap_marker(&mut kept, &mut consecutive_skipped);
        kept.push(segment.to_owned());
    }

    kept.join("\n")
}

/// If >3 lines were skipped consecutively, push a gap marker and reset counter.
fn flush_gap_marker(kept: &mut Vec<String>, consecutive_skipped: &mut u32) {
    if *consecutive_skipped > 3 {
        kept.push(format!(
            "       ... ({} lines omitted)",
            *consecutive_skipped
        ));
    }
    *consecutive_skipped = 0;
}

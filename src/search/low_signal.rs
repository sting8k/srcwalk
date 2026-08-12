use crate::types::SearchResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LowSignalTermStats {
    pub(crate) term: String,
    pub(crate) total_matches: usize,
    pub(crate) matched_files: usize,
    pub(crate) eligible_files: usize,
}

pub(crate) fn low_signal_term_stats(term: &str, result: &SearchResult) -> LowSignalTermStats {
    let matched_files = result
        .matches
        .iter()
        .map(|matched| &matched.path)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    LowSignalTermStats {
        term: term.to_string(),
        total_matches: result.total_found,
        matched_files,
        eligible_files: result.eligible_files,
    }
}

/// v1 thresholds are evidence-derived, not a universal semantic classifier.
/// Validation date: 2026-08-12. Pinned repositories: Ktor
/// `a8d8038939d788547b807acbc580609e7603d9dd`, Spring Framework
/// `68e6acd37ed0c12e395ef96d170971028e727383`, Symfony
/// `3b11ffbe2520436b0951974d1993dc2144f91dd3`, and dotnet/runtime
/// `656d1d6f42b140e751f65a2ec58e5066b54e1f54`. The closest observed separator
/// was runtime `GetTypeInfo` near 1.26% versus runtime `execute` near 1.92%, a
/// 0.66 percentage-point margin. Retuning any constant requires new
/// cross-repository evidence, an updated matrix and boundary tests, and reviewer
/// approval.
const MIN_MATCHES: usize = 400;
const MIN_MATCHED_FILES: usize = 150;
const SPREAD_SCALE: u128 = 1_000;
const MIN_SPREAD: u128 = 15;

pub(crate) fn low_signal_term_advisory(stats: &LowSignalTermStats) -> Option<String> {
    if stats.eligible_files == 0
        || stats.total_matches < MIN_MATCHES
        || stats.matched_files < MIN_MATCHED_FILES
    {
        return None;
    }

    let matched_scaled = (stats.matched_files as u128).checked_mul(SPREAD_SCALE)?;
    let eligible_scaled = (stats.eligible_files as u128).checked_mul(MIN_SPREAD)?;
    if matched_scaled < eligible_scaled {
        return None;
    }

    Some(format!(
        "> Note: {} matches across {} of {} eligible files for `{}`; if this spread is not intentional, consider `overview`, a narrower term or scope, or a structural route.",
        format_count(stats.total_matches),
        format_count(stats.matched_files),
        format_count(stats.eligible_files),
        escape_term(&stats.term),
    ))
}

/// Put shared advisory lines into the trailing footer immediately before the
/// existing generic `> Next:` action. The budget layer preserves this footer.
pub(crate) fn insert_low_signal_advisories(output: String, advisories: &[String]) -> String {
    if advisories.is_empty() {
        return output;
    }
    let notes = advisories.join("\n");
    let Some((body, footer)) = crate::format::split_trailing_footer(&output) else {
        return format!("{}\n\n{notes}", output.trim_end());
    };

    let footer = footer.trim();
    // Insert before the generic `> Next:` action when present; otherwise append
    // at the footer end so pre-existing caveats/notes stay first (contract: existing
    // evidence first, advisory before generic Next).
    let insertion = footer.find("> Next:").unwrap_or(footer.len());
    let (before, after) = footer.split_at(insertion);
    let before = before.trim_end();
    let after = after.trim_start();
    let mut rendered_footer = String::new();
    if !before.is_empty() {
        rendered_footer.push_str(before);
        rendered_footer.push('\n');
    }
    rendered_footer.push_str(&notes);
    if !after.is_empty() {
        rendered_footer.push('\n');
        rendered_footer.push_str(after);
    }
    format!("{}\n\n{rendered_footer}", body.trim_end())
}

fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut rendered = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            rendered.push(',');
        }
        rendered.push(character);
    }
    rendered
}

fn escape_term(term: &str) -> String {
    let mut escaped = String::with_capacity(term.len());
    for character in term.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '`' => escaped.push_str("\\`"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{{{:x}}}", character as u32);
            }
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{insert_low_signal_advisories, low_signal_term_advisory, LowSignalTermStats};

    fn stats(
        total_matches: usize,
        matched_files: usize,
        eligible_files: usize,
    ) -> LowSignalTermStats {
        LowSignalTermStats {
            term: "execute".to_string(),
            total_matches,
            matched_files,
            eligible_files,
        }
    }

    #[test]
    fn threshold_boundaries_are_inclusive() {
        assert!(low_signal_term_advisory(&stats(399, 150, 1_000)).is_none());
        assert!(low_signal_term_advisory(&stats(400, 149, 1_000)).is_none());
        assert!(low_signal_term_advisory(&stats(400, 150, 10_001)).is_none());
        assert_eq!(
            low_signal_term_advisory(&stats(400, 150, 10_000)).as_deref(),
            Some("> Note: 400 matches across 150 of 10,000 eligible files for `execute`; if this spread is not intentional, consider `overview`, a narrower term or scope, or a structural route.")
        );
    }

    #[test]
    fn zero_eligible_files_never_trigger() {
        assert!(low_signal_term_advisory(&stats(10_000, 10_000, 0)).is_none());
    }

    #[test]
    fn repeated_calls_are_byte_identical_and_wording_is_conditional() {
        let stats = stats(452, 171, 3_046);
        let first = low_signal_term_advisory(&stats).unwrap();
        assert_eq!(first, low_signal_term_advisory(&stats).unwrap());
        assert!(first.contains("if this spread is not intentional"));
        for forbidden in ["stop", "must", "avoid"] {
            assert!(!first.to_ascii_lowercase().contains(forbidden), "{first}");
        }
    }

    #[test]
    fn term_escaping_keeps_the_note_single_line_and_quoted() {
        let mut stats = stats(400, 150, 10_000);
        stats.term = "a`b\\c\n".to_string();
        let note = low_signal_term_advisory(&stats).unwrap();
        assert!(note.contains("for `a\\`b\\\\c\\n`;"), "{note}");
        assert!(!note.contains('\n'));
    }

    #[test]
    fn advisory_inserts_before_existing_next_footer() {
        let output = "body\n\n> Caveat: existing\n\n> Next: continue".to_string();
        let note = low_signal_term_advisory(&stats(400, 150, 10_000)).unwrap();
        let rendered = insert_low_signal_advisories(output, &[note]);
        assert!(
            rendered.contains("> Caveat: existing\n> Note:"),
            "{rendered}"
        );
        assert!(rendered.find("> Note:").unwrap() < rendered.find("> Next:").unwrap());
    }

    #[test]
    fn advisory_appends_after_existing_notes_when_no_next() {
        // Footer with existing caveat + note but no `> Next:`. The low-signal
        // note must go after the existing note, not before it.
        let out = "body\n\n> Caveat: existing\n\n> Note: prior note\n".to_string();
        let note = low_signal_term_advisory(&stats(400, 150, 10_000)).unwrap();
        let rendered = insert_low_signal_advisories(out, &[note]);
        let caveat = rendered.find("> Caveat:").unwrap();
        let prior = rendered.find("> Note: prior note").unwrap();
        let low = rendered.rfind("> Note: 400 matches").unwrap();
        assert!(
            caveat < prior && prior < low,
            "existing evidence must precede advisory:\n{rendered}"
        );
    }
}

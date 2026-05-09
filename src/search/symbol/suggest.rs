use std::path::Path;
use std::sync::Mutex;

/// Find spelling-similar symbols for a query that produced 0 hits.
///
/// Strategy: case-insensitive `\bquery\b` regex sweep over the same scope used
/// by the failed search. Collects each distinct **actual spelling** found in
/// source, with its first location, then ranks by `edit_distance(query_lower`,
/// `hit_lower`). Returns up to `top_n` suggestions.
///
/// Cheap because it only fires on the 0-hit path. Uses ripgrep's `\b…\b`
/// matcher with `(?i)` flag — same engine as `find_usages`.
/// Normalize an identifier for fuzzy comparison: lowercase + strip underscores.
/// Lets `searchSymbol` ↔ `search_symbol` ↔ `SearchSymbol` all collapse to
/// the same canonical string, so a single `edit_distance` over normalized
/// forms covers both naming-convention mismatches and typos uniformly.
fn normalize_ident(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b == b'_' {
            continue;
        }
        out.push(b.to_ascii_lowercase() as char);
    }
    out
}

/// Sweep `scope` for identifiers whose normalized form is close to the
/// normalized `query` (edit distance ≤ threshold). Only considers source
/// files (JSON / markdown / lockfiles excluded) to avoid noise from i18n
/// bundles, build manifests, etc.
pub(super) fn suggest(
    query: &str,
    scope: &Path,
    glob: Option<&str>,
    top_n: usize,
) -> Vec<(String, std::path::PathBuf, u32)> {
    if query.is_empty() {
        return Vec::new();
    }
    let q_norm = normalize_ident(query);
    if q_norm.is_empty() {
        return Vec::new();
    }
    // Threshold scales with query length: allow 1 edit for short queries,
    // up to 2 for ≥6 normalized chars. Keeps recall for typos without
    // matching unrelated identifiers.
    let max_dist: usize = if q_norm.len() >= 6 { 2 } else { 1 };

    let Ok(walker) = crate::search::walker(scope, glob) else {
        return Vec::new();
    };

    // spelling → (path, line)
    let hits: Mutex<std::collections::HashMap<String, (std::path::PathBuf, u32)>> =
        Mutex::new(std::collections::HashMap::new());

    walker.run(|| {
        let hits = &hits;
        let q_norm = q_norm.clone();
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }
            let path = entry.path();
            // Only sweep real source files — drops i18n JSON, SOURCES.txt,
            // lockfiles, markdown, etc. that would otherwise pollute
            // suggestions with non-identifier text.
            if !matches!(
                crate::lang::detect_file_type(path),
                crate::types::FileType::Code(_)
            ) {
                return ignore::WalkState::Continue;
            }
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.len() > 500_000 {
                    return ignore::WalkState::Continue;
                }
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                return ignore::WalkState::Continue;
            };
            let mut local: Vec<(String, u32)> = Vec::new();
            let mut seen_on_path: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for (line_idx, line) in content.lines().enumerate() {
                let bytes = line.as_bytes();
                let mut i = 0;
                while i < bytes.len() {
                    let b = bytes[i];
                    if !(b.is_ascii_alphabetic() || b == b'_') {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    let word = &line[start..i];
                    // Cheap length prefilter — keeps the inner Levenshtein
                    // call off >90% of tokens.
                    let w_norm_len = word.bytes().filter(|&c| c != b'_').count();
                    if w_norm_len == 0
                        || w_norm_len + max_dist < q_norm.len()
                        || w_norm_len > q_norm.len() + max_dist
                    {
                        continue;
                    }
                    if seen_on_path.contains(word) {
                        continue;
                    }
                    let w_norm = normalize_ident(word);
                    if w_norm == q_norm && word == query {
                        // Exact match — caller already handles hit path.
                        continue;
                    }
                    let d = crate::read::edit_distance(&q_norm, &w_norm);
                    if d <= max_dist {
                        seen_on_path.insert(word.to_string());
                        local.push((word.to_string(), line_idx as u32 + 1));
                    }
                }
            }
            if !local.is_empty() {
                let mut h = hits
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for (spelling, line) in local {
                    h.entry(spelling)
                        .or_insert_with(|| (path.to_path_buf(), line));
                }
            }
            ignore::WalkState::Continue
        })
    });

    let mut all: Vec<(String, std::path::PathBuf, u32)> = hits
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .into_iter()
        .filter(|(s, _)| s != query)
        .map(|(s, (p, l))| (s, p, l))
        .collect();
    // Rank by distance on normalized form; prefer same-case exact normalized
    // match over mere typo; then alphabetical for stability.
    all.sort_by(|a, b| {
        let an = normalize_ident(&a.0);
        let bn = normalize_ident(&b.0);
        let da = crate::read::edit_distance(&q_norm, &an);
        let db = crate::read::edit_distance(&q_norm, &bn);
        da.cmp(&db).then_with(|| a.0.cmp(&b.0))
    });
    all.truncate(top_n);
    all
}

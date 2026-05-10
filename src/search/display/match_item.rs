use std::collections::HashSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use crate::cache::OutlineCache;
use crate::format::rel_nonempty;
use crate::session::Session;
use crate::types::Match;

use crate::search::{callees, siblings, strip, truncate};

use super::{expand, non_definition_label, outline_context_for_match, semantic, ExpandBudget};

pub(super) fn format_single_match(
    m: &Match,
    scope: &Path,
    cache: &OutlineCache,
    session: Option<&Session>,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand_remaining: &mut usize,
    expand_budget: &mut ExpandBudget,
    expanded_files: &mut HashSet<PathBuf>,
    context_shown_files: &mut HashSet<PathBuf>,
    smart_truncated: &mut bool,
    multi_file: bool,
    out: &mut String,
) {
    if m.is_definition {
        semantic::format_definition_semantic_match(m, scope, cache, out);
    } else {
        let kind = non_definition_label(m);
        let _ = write!(
            out,
            "\n\n## {}:{} [{kind}]",
            rel_nonempty(&m.path, scope),
            m.line
        );

        let _ = write!(out, "\n→ [{}]   {}", m.line, m.text);

        // Artifact byte snippets are already centered evidence; do not replace them with outline context.
        if crate::artifact::is_artifact_js_ts_file(&m.path) && m.text.contains("--section bytes:") {
            // Exact byte evidence already printed above.
            // Skip outline for small files — the expanded code speaks for itself.
            // For larger files, show outline context only once per file to avoid
            // repeated imports/module headers across consecutive matches.
        } else if m.file_lines >= 50 && context_shown_files.insert(m.path.clone()) {
            if let Some(context) = outline_context_for_match(&m.path, m.line, cache) {
                out.push_str(&context);
            }
        } else if m.file_lines >= 50 {
            out.push_str(" [context shown earlier]");
        }
    }

    if *expand_remaining > 0 {
        // Check session dedup for definitions with def_range
        let deduped = m.is_definition
            && m.def_range.is_some()
            && session.is_some_and(|s| s.is_expanded(&m.path, m.line));

        if deduped {
            if let Some((start, end)) = m.def_range {
                let _ = write!(
                    out,
                    "\n\n[shown earlier] {}:{}-{} {}",
                    rel_nonempty(&m.path, scope),
                    start,
                    end,
                    m.text
                );
            }
        } else {
            let skip = multi_file && expanded_files.contains(&m.path);
            if !skip {
                if let Some((code, content)) = expand::expand_match(m, scope) {
                    if m.is_definition && m.def_range.is_some() {
                        if let Some(s) = session {
                            s.record_expand(&m.path, m.line);
                        }
                    }

                    let file_type = crate::lang::detect_file_type(&m.path);
                    let mut skip_lines = strip::strip_noise(&content, &m.path, m.def_range);

                    if let Some((def_start, def_end)) = m.def_range {
                        if let crate::types::FileType::Code(lang) = file_type {
                            if let Some(keep) =
                                truncate::select_diverse_lines(&content, def_start, def_end, lang)
                            {
                                *smart_truncated = true;
                                let keep_set: HashSet<u32> = keep.into_iter().collect();
                                for ln in def_start..=def_end {
                                    if !keep_set.contains(&ln) {
                                        skip_lines.insert(ln);
                                    }
                                }
                            }
                        }
                    }

                    let stripped_code = if skip_lines.is_empty() {
                        code
                    } else {
                        expand::filter_code_lines(&code, &skip_lines)
                    };

                    if !expand_budget.try_consume(&stripped_code) {
                        return;
                    }

                    out.push('\n');
                    out.push_str(&stripped_code);

                    if m.is_definition && m.def_range.is_some() {
                        if let crate::types::FileType::Code(lang) = file_type {
                            let callee_names =
                                callees::extract_callee_names(&content, lang, m.def_range);
                            if !callee_names.is_empty() {
                                let mut nodes = callees::resolve_callees_transitive(
                                    &callee_names,
                                    &m.path,
                                    &content,
                                    cache,
                                    bloom,
                                    2,
                                    15,
                                );

                                if let Some(ref name) = m.def_name {
                                    nodes.retain(|n| n.callee.name != *name);
                                }
                                if nodes.len() > 8 {
                                    nodes.sort_by_key(|n| i32::from(n.callee.file == m.path));
                                    nodes.truncate(8);
                                }

                                if !nodes.is_empty() {
                                    out.push_str("\n\n\u{2500}\u{2500} calls \u{2500}\u{2500}");
                                    for n in &nodes {
                                        let c = &n.callee;
                                        let _ = write!(
                                            out,
                                            "\n  {}  {}:{}-{}",
                                            c.name,
                                            rel_nonempty(&c.file, scope),
                                            c.start_line,
                                            c.end_line
                                        );
                                        if let Some(ref sig) = c.signature {
                                            let _ = write!(out, "  {sig}");
                                        }
                                        for child in &n.children {
                                            let _ = write!(
                                                out,
                                                "\n    \u{2192} {}  {}:{}-{}",
                                                child.name,
                                                rel_nonempty(&child.file, scope),
                                                child.start_line,
                                                child.end_line
                                            );
                                            if let Some(ref sig) = child.signature {
                                                let _ = write!(out, "  {sig}");
                                            }
                                        }
                                    }
                                }
                            }

                            if let Some(def_range) = m.def_range {
                                let entries =
                                    crate::lang::outline::get_outline_entries(&content, lang);
                                if let Some(parent) = siblings::find_parent_entry(&entries, m.line)
                                {
                                    let refs = siblings::extract_sibling_references(
                                        &content, lang, def_range,
                                    );
                                    if !refs.is_empty() {
                                        let filtered: Vec<String> =
                                            if let Some(ref name) = m.def_name {
                                                refs.into_iter().filter(|r| r != name).collect()
                                            } else {
                                                refs
                                            };

                                        let resolved =
                                            siblings::resolve_siblings(&filtered, &parent.children);
                                        if !resolved.is_empty() {
                                            out.push_str(
                                                "\n\n\u{2500}\u{2500} siblings \u{2500}\u{2500}",
                                            );
                                            for s in &resolved {
                                                let _ = write!(
                                                    out,
                                                    "\n  {}  {}:{}-{}  {}",
                                                    s.name,
                                                    rel_nonempty(&m.path, scope),
                                                    s.start_line,
                                                    s.end_line,
                                                    s.signature,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    *expand_remaining -= 1;
                    expanded_files.insert(m.path.clone());
                }
            }
        }
    }
}

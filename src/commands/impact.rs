use std::path::Path;

use crate::cache::OutlineCache;
use crate::commands::context::apply_optional_budget;
use crate::commands::flow::is_test_path;
use crate::error::SrcwalkError;
use crate::{format, index, search};

/// Lab: compact upstream blast-radius slice for changing a symbol.
pub(crate) fn run_impact(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt::Write as _;

    const DEF_DISPLAY_LIMIT: usize = 8;
    const CALLSITE_DISPLAY_LIMIT: usize = 20;
    const GROUP_DISPLAY_LIMIT: usize = 10;
    const BROAD_DEFINITION_THRESHOLD: usize = 5;
    const BROAD_CALLSITE_THRESHOLD: usize = 50;

    let bloom = index::bloom::BloomFilterCache::new();
    let raw = search::search_symbol_raw(target, scope, None)?;
    let mut seen_defs = BTreeSet::new();
    let mut defs: Vec<_> = raw
        .matches
        .iter()
        .filter(|m| m.is_definition)
        .filter(|m| {
            seen_defs.insert((
                m.path.clone(),
                m.def_range.unwrap_or((m.line, m.line)),
                m.text.trim().to_string(),
            ))
        })
        .collect();
    defs.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));

    let mut out = format!("# Slice: {target} — impact\n\n[symbol] {target}\n");
    if defs.is_empty() {
        out.push_str("= definitions\n  (none)\n");
    } else {
        out.push_str("= definitions\n");
        for def in defs.iter().take(DEF_DISPLAY_LIMIT) {
            let rel = format::rel_nonempty(&def.path, scope);
            let range = def
                .def_range
                .map_or(String::new(), |(s, e)| format!(":{s}-{e}"));
            let text = def.text.trim();
            let _ = writeln!(out, "  [def] {rel}{range} {text}");
        }
        if defs.len() > DEF_DISPLAY_LIMIT {
            let _ = writeln!(
                out,
                "  ... {} more definitions",
                defs.len() - DEF_DISPLAY_LIMIT
            );
        }
    }

    let mut callers = search::callers::find_callers(target, scope, &bloom, None, Some(cache))?;
    callers.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    let total_callers = callers.len();
    out.push_str("\n<- name-matched calls from\n");
    if callers.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for caller in callers.iter().take(CALLSITE_DISPLAY_LIMIT) {
            let rel = format::rel_nonempty(&caller.path, scope);
            let mut facts = String::new();
            if let Some(recv) = &caller.receiver {
                let _ = write!(facts, " recv={recv}");
            }
            if let Some(args) = caller.arg_count {
                let _ = write!(facts, " args={args}");
            }
            let _ = writeln!(
                out,
                "  [fn] {} {}:{}{}",
                caller.calling_function, rel, caller.line, facts
            );
        }
        if callers.len() > CALLSITE_DISPLAY_LIMIT {
            let _ = writeln!(
                out,
                "  ... {} more call sites",
                callers.len() - CALLSITE_DISPLAY_LIMIT
            );
        }
    }

    if !callers.is_empty() {
        let mut by_file: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut by_receiver: BTreeMap<String, usize> = BTreeMap::new();
        for caller in &callers {
            *by_file
                .entry(format::rel_nonempty(&caller.path, scope))
                .or_insert(0) += 1;
            let kind = if is_test_path(&caller.path) {
                "test"
            } else {
                "prod"
            };
            *by_kind.entry(kind).or_insert(0) += 1;
            *by_receiver
                .entry(
                    caller
                        .receiver
                        .clone()
                        .unwrap_or_else(|| "<bare>".to_string()),
                )
                .or_insert(0) += 1;
        }

        out.push_str("\n~ groups\n");
        for (kind, count) in by_kind {
            let _ = writeln!(out, "  [group] kind={kind} count={count}");
        }

        let mut receivers: Vec<_> = by_receiver.into_iter().collect();
        receivers.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (receiver, count) in receivers.into_iter().take(GROUP_DISPLAY_LIMIT) {
            let _ = writeln!(out, "  [group] receiver={receiver} count={count}");
        }

        let mut files: Vec<_> = by_file.into_iter().collect();
        files.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (file, count) in files.into_iter().take(GROUP_DISPLAY_LIMIT) {
            let _ = writeln!(out, "  [group] file={file} count={count}");
        }
    }

    if defs.is_empty() && !callers.is_empty() {
        out.push_str("\n> Warning: no definitions found; showing name-matched call sites only.");
    }
    if defs.len() > BROAD_DEFINITION_THRESHOLD || total_callers > BROAD_CALLSITE_THRESHOLD {
        out.push_str("\n> Warning: broad symbol name; impact is name-matched and may include unrelated receivers.");
    }

    let _ = write!(
        out,
        "\n> Caveat: {total_callers} direct name-matched call site{} found; impact output capped.\n> Next: use `srcwalk callers <symbol> --depth 2` or `srcwalk callers <symbol> --count-by receiver|file`.",
        if total_callers == 1 { "" } else { "s" }
    );
    if total_callers == 0 {
        out.push_str("\n> Note: no direct name-matched calls found in scope; this is not proof of no runtime callers.");
    }
    out.push_str("\n> Note: direct-name scope scan; misses dynamic dispatch, reflection, generated/ignored, out-of-scope callers.");
    Ok(apply_optional_budget(out, budget_tokens))
}

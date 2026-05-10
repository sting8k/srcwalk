use std::path::Path;

use crate::cache::OutlineCache;
use crate::commands::context::{apply_optional_budget, ArtifactMode};
use crate::commands::flow::is_test_path;
use crate::error::SrcwalkError;
use crate::{format, index, search};

/// Lab: compact upstream blast-radius slice for changing a symbol.
fn run_artifact_impact(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt::Write as _;

    const DEF_DISPLAY_LIMIT: usize = 8;
    const CALLSITE_DISPLAY_LIMIT: usize = 20;
    const GROUP_DISPLAY_LIMIT: usize = 10;
    const BROAD_CALLSITE_THRESHOLD: usize = 50;

    let bloom = index::bloom::BloomFilterCache::new();
    let raw = search::search_symbol_raw_with_artifact(target, scope, None, artifact)?;
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

    let mut out = format!("# Slice: {target} — artifact impact\n\n[symbol] {target}\n");
    if defs.is_empty() {
        out.push_str("= definitions\n  (none)\n");
    } else {
        out.push_str("= definitions\n");
        for def in defs.iter().take(DEF_DISPLAY_LIMIT) {
            let rel = format::rel_nonempty(&def.path, scope);
            let _ = writeln!(
                out,
                "  [def] {rel}:{}  section: srcwalk {} --artifact --section {target}",
                def.line,
                def.path.display()
            );
        }
        if defs.len() > DEF_DISPLAY_LIMIT {
            let _ = writeln!(
                out,
                "  ... {} more definitions",
                defs.len() - DEF_DISPLAY_LIMIT
            );
        }
    }

    let mut callers = search::callers::find_callers_with_artifact(
        target,
        scope,
        &bloom,
        None,
        Some(cache),
        artifact,
    )?;
    callers.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    let total_callers = callers.len();
    out.push_str("\n<- artifact name-matched calls from\n");
    append_artifact_impact_callers(&mut out, scope, &callers, CALLSITE_DISPLAY_LIMIT);

    if !callers.is_empty() {
        let mut by_file: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_receiver: BTreeMap<String, usize> = BTreeMap::new();
        for caller in &callers {
            *by_file
                .entry(format::rel_nonempty(&caller.path, scope))
                .or_insert(0) += 1;
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
        out.push_str(
            "\n> Warning: no definitions found; showing artifact name-matched call sites only.",
        );
    }
    if total_callers > BROAD_CALLSITE_THRESHOLD {
        out.push_str("\n> Warning: broad artifact symbol; name-matched calls may include generated/helper noise.");
    }
    let _ = write!(
        out,
        "\n> Caveat: {total_callers} artifact name-matched call site{} found; byte-level bundle evidence, not source-level blast radius.",
        if total_callers == 1 { "" } else { "s" }
);
    if total_callers == 0 {
        out.push_str("\n> Note: no direct name-matched calls found in artifact scope; this is not proof of no runtime callers.");
    }
    out.push_str("\n> Next: use `srcwalk callers <symbol> --artifact --expand=1`, `srcwalk find <api|string> --artifact`, or `srcwalk <path> --artifact --section bytes:<start>-<end>`.");
    if let Some(note) = artifact.callees_note() {
        out.push_str("\n> ");
        out.push_str(note);
    }
    Ok(apply_optional_budget(out, budget_tokens))
}

fn append_source_impact_callers(
    out: &mut String,
    scope: &Path,
    callers: &[search::callers::CallerMatch],
    limit: usize,
) {
    use std::fmt::Write as _;

    if callers.is_empty() {
        out.push_str("  (none)\n");
        return;
    }

    let displayed = callers.len().min(limit);
    let mut current_path: Option<String> = None;
    for caller in callers.iter().take(displayed) {
        let rel = format::rel_nonempty(&caller.path, scope);
        if current_path.as_deref() != Some(rel.as_str()) {
            current_path = Some(rel.clone());
            let _ = writeln!(out, "  {rel}");
        }
        let _ = write!(out, "    [fn] {}:{}", caller.calling_function, caller.line);
        append_caller_facts(out, caller);
        let _ = writeln!(out);
    }
    if callers.len() > displayed {
        let _ = writeln!(out, "  ... {} more call sites", callers.len() - displayed);
    }
}

fn append_artifact_impact_callers(
    out: &mut String,
    scope: &Path,
    callers: &[search::callers::CallerMatch],
    limit: usize,
) {
    use std::fmt::Write as _;

    if callers.is_empty() {
        out.push_str("  (none)\n");
        return;
    }

    let mut groups = artifact_impact_call_groups(callers);
    groups.sort_by(|a, b| {
        b.callers
            .len()
            .cmp(&a.callers.len())
            .then(a.path.cmp(&b.path))
            .then(a.calling_function.cmp(&b.calling_function))
            .then(a.line.cmp(&b.line))
    });

    let displayed = groups.len().min(limit);
    let mut current_path: Option<String> = None;
    let mut shown_calls = 0_usize;
    for group in groups.iter().take(displayed) {
        let rel = format::rel_nonempty(&group.path, scope);
        if current_path.as_deref() != Some(rel.as_str()) {
            current_path = Some(rel.clone());
            let _ = writeln!(out, "  {rel}");
        }
        shown_calls += group.callers.len();
        append_artifact_call_group(out, &group.callers);
    }
    if groups.len() > displayed {
        let hidden_groups = groups.len() - displayed;
        let hidden_calls = callers.len().saturating_sub(shown_calls);
        let _ = writeln!(
            out,
            "  ... {hidden_calls} more call sites across {hidden_groups} groups"
        );
    }
}

fn append_artifact_call_group(out: &mut String, group: &[&search::callers::CallerMatch]) {
    use std::fmt::Write as _;

    let first = &group[0];
    let _ = write!(out, "    [fn] {}:{}", first.calling_function, first.line);
    if group.len() > 1 {
        let _ = write!(out, " [{} calls]", group.len());
    }
    append_caller_facts(out, first);

    let ranges: Vec<_> = group
        .iter()
        .filter_map(|caller| caller.call_byte_range)
        .collect();
    if group.len() == 1 {
        if let Some((start, end)) = ranges.first() {
            let _ = write!(out, " bytes:{start}-{end}");
        }
        let _ = writeln!(out);
        return;
    }

    let _ = writeln!(out);
    for (start, end) in ranges.iter().take(6) {
        let _ = writeln!(out, "      bytes:{start}-{end}");
    }
    if ranges.len() > 6 {
        let _ = writeln!(out, "      ... {} more byte ranges", ranges.len() - 6);
    }
}

struct ArtifactImpactCallGroup<'a> {
    path: std::path::PathBuf,
    calling_function: String,
    line: u32,
    callers: Vec<&'a search::callers::CallerMatch>,
    _marker: std::marker::PhantomData<&'a ()>,
}

fn artifact_impact_call_groups(
    callers: &[search::callers::CallerMatch],
) -> Vec<ArtifactImpactCallGroup<'_>> {
    let mut groups: Vec<ArtifactImpactCallGroup<'_>> = Vec::new();
    for caller in callers {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| same_impact_call_group(group.callers[0], caller))
        {
            group.callers.push(caller);
        } else {
            groups.push(ArtifactImpactCallGroup {
                path: caller.path.clone(),
                calling_function: caller.calling_function.clone(),
                line: caller.line,
                callers: vec![caller],
                _marker: std::marker::PhantomData,
            });
        }
    }
    groups
}

fn same_impact_call_group(
    a: &search::callers::CallerMatch,
    b: &search::callers::CallerMatch,
) -> bool {
    a.path == b.path
        && a.calling_function == b.calling_function
        && a.line == b.line
        && a.receiver == b.receiver
        && a.arg_count == b.arg_count
}

fn append_caller_facts(out: &mut String, caller: &search::callers::CallerMatch) {
    use std::fmt::Write as _;

    if let Some(recv) = &caller.receiver {
        let _ = write!(out, " recv={recv}");
    }
    if let Some(args) = caller.arg_count {
        let _ = write!(out, " args={args}");
    }
}

pub(crate) fn run_impact(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt::Write as _;

    const DEF_DISPLAY_LIMIT: usize = 8;
    const CALLSITE_DISPLAY_LIMIT: usize = 20;
    const GROUP_DISPLAY_LIMIT: usize = 10;
    const BROAD_DEFINITION_THRESHOLD: usize = 5;
    const BROAD_CALLSITE_THRESHOLD: usize = 50;

    if artifact.enabled() {
        return run_artifact_impact(target, scope, budget_tokens, cache, artifact);
    }

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
    append_source_impact_callers(&mut out, scope, &callers, CALLSITE_DISPLAY_LIMIT);

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

use std::path::Path;

use crate::cache::OutlineCache;
use crate::commands::call_format::format_call_site;
use crate::commands::context::{apply_optional_budget, ArtifactMode};
use crate::commands::find::symbol_or_file_suggestion;
use crate::error::SrcwalkError;
use crate::{format, index, lang, search, types};

/// Lab: compact downstream flow slice for a known symbol.
pub(crate) fn run_flow(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    depth: Option<usize>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    use std::fmt::Write as _;

    if artifact.enabled() {
        return run_artifact_flow(target, scope, budget_tokens, cache, filter, artifact);
    }

    let bloom = index::bloom::BloomFilterCache::new();
    let def_match = find_primary_definition(target, scope)?;
    let content = std::fs::read_to_string(&def_match.path).map_err(|e| SrcwalkError::IoError {
        path: def_match.path.clone(),
        source: e,
    })?;
    let types::FileType::Code(lang) = lang::detect_file_type(&def_match.path) else {
        return Ok(format!("# Slice: {target} — flow\n\n(not a code file)"));
    };

    let rel = format::rel_nonempty(&def_match.path, scope);
    let range = def_match
        .def_range
        .map_or(String::new(), |(s, e)| format!(":{s}-{e}"));
    let mut out = format!("# Slice: {target} — flow\n\n[symbol] {target} {rel}{range}\n");

    let sites = search::callees::extract_call_sites(&content, lang, def_match.def_range);
    let total_sites = sites.len();
    let sites = search::callees::filter_call_sites(sites, filter)?;
    if let Some(filter) = filter {
        let _ = writeln!(out, "-> calls (ordered, filtered {filter})");
    } else {
        out.push_str("-> calls (ordered)\n");
    }
    if sites.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for site in sites.iter().take(40) {
            let _ = writeln!(out, "  [call] {}", format_call_site(site));
        }
        if sites.len() > 40 {
            let _ = writeln!(out, "  ... {} more call sites", sites.len() - 40);
        }
    }

    let names = if filter.is_some() {
        sites
            .iter()
            .map(|site| site.callee.clone())
            .collect::<Vec<_>>()
    } else {
        search::callees::extract_callee_names(&content, lang, def_match.def_range)
    };
    let depth_limit = depth.map_or(1, |d| d.min(3) as u32);
    let nodes = search::callees::resolve_callees_transitive(
        &names,
        &def_match.path,
        &content,
        cache,
        &bloom,
        depth_limit,
        30,
    );
    let flow_nodes = prioritize_flow_resolves(nodes, &def_match.path);
    if !flow_nodes.is_empty() {
        out.push_str("\n-> resolves (selected local helpers)\n");
        for node in flow_nodes.iter().take(12) {
            append_resolved_callee(&mut out, scope, &node.callee, 1);
            for child in node.children.iter().take(2) {
                append_resolved_callee(&mut out, scope, child, 2);
            }
        }
        if flow_nodes.len() > 12 {
            let _ = writeln!(out, "  ... {} more resolved callees", flow_nodes.len() - 12);
        }
    }

    if let Ok(mut callers) = search::callers::find_callers(target, scope, &bloom, None, Some(cache))
    {
        callers.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        if !callers.is_empty() {
            out.push_str("\n<- callers\n");
            for caller in callers.iter().take(8) {
                let rel_c = format::rel_nonempty(&caller.path, scope);
                let _ = writeln!(
                    out,
                    "  [fn] {} {}:{}",
                    caller.calling_function, rel_c, caller.line
                );
            }
            if callers.len() > 8 {
                let _ = writeln!(out, "  ... {} more callers", callers.len() - 8);
            }
        }
    }

    if filter.is_some() {
        let _ = write!(
            out,
            "\n> Note: filter matched {}/{} call sites. Qualifiers: callee:NAME.",
            sites.len(),
            total_sites
        );
    }
    out.push_str("\n> Caveat: flow output capped.\n> Next: use `srcwalk callees <symbol> --detailed` or `srcwalk callers <symbol>`.");
    Ok(apply_optional_budget(out, budget_tokens))
}

fn run_artifact_flow(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    use std::fmt::Write as _;

    let bloom = index::bloom::BloomFilterCache::new();
    let def_match = find_primary_definition_with_artifact(target, scope, artifact)?;
    let content = std::fs::read_to_string(&def_match.path).map_err(|e| SrcwalkError::IoError {
        path: def_match.path.clone(),
        source: e,
    })?;
    let types::FileType::Code(lang) = lang::detect_file_type(&def_match.path) else {
        return Ok(format!(
            "# Slice: {target} — artifact flow\n\n(not a code file)"
        ));
    };

    let rel = format::rel_nonempty(&def_match.path, scope);
    let mut out = format!(
        "# Slice: {target} — artifact flow\n\n[symbol] {target} {rel}:{}\n",
        def_match.line
    );
    let _ = writeln!(
        out,
        "  section: srcwalk {} --artifact --section {}",
        def_match.path.display(),
        target
    );

    let mut sites = search::callees::extract_call_sites_for_artifact_target(
        &content,
        lang,
        target,
        def_match.def_range,
    );
    let total_sites = sites.len();
    sites = search::callees::filter_call_sites(sites, filter)?;
    if let Some(filter) = filter {
        let _ = writeln!(out, "\n-> calls (artifact, filtered {filter})");
    } else {
        out.push_str("\n-> calls (artifact)\n");
    }
    if sites.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for site in sites.iter().take(12) {
            append_artifact_call_site(&mut out, site);
        }
        if sites.len() > 12 {
            let _ = writeln!(out, "  ... {} more call sites", sites.len() - 12);
        }
    }

    if let Ok(mut callers) = search::callers::find_callers_with_artifact(
        target,
        scope,
        &bloom,
        None,
        Some(cache),
        artifact,
    ) {
        callers.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        if !callers.is_empty() {
            out.push_str("\n<- callers (artifact)\n");
            let mut current_path: Option<String> = None;
            for caller in callers.iter().take(8) {
                let rel_c = format::rel_nonempty(&caller.path, scope);
                if current_path.as_deref() != Some(rel_c.as_str()) {
                    current_path = Some(rel_c.clone());
                    let _ = writeln!(out, "  {rel_c}");
                }
                let _ = write!(out, "    [fn] {}:{}", caller.calling_function, caller.line);
                if let Some((start, end)) = caller.call_byte_range {
                    let _ = write!(out, "  bytes:{start}-{end}");
                }
                let _ = writeln!(out);
            }
            if callers.len() > 8 {
                let _ = writeln!(out, "  ... {} more callers", callers.len() - 8);
            }
        }
    }

    if filter.is_some() {
        let _ = write!(
            out,
            "\n> Note: filter matched {}/{} call sites. Qualifiers: callee:NAME.",
            sites.len(),
            total_sites
        );
    }
    out.push_str(
        "\n> Caveat: artifact flow is byte-level bundle evidence, not sourcemap/source semantics.",
    );
    out.push_str("\n> Next: use `srcwalk <path> --artifact --section <symbol|bytes:start-end>`, `callers --artifact --expand=1`, or `callees --artifact --detailed`.");
    if let Some(note) = artifact.callees_note() {
        out.push_str("\n> ");
        out.push_str(note);
    }
    Ok(apply_optional_budget(out, budget_tokens))
}

fn append_artifact_call_site(out: &mut String, site: &search::callees::CallSite) {
    use std::fmt::Write as _;

    let _ = write!(out, "  [call] L{} {}", site.line, site.callee);
    if !site.args.is_empty() {
        let _ = write!(out, " args={}", site.args.len());
    }
    if let Some((start, end)) = site.call_byte_range {
        let _ = write!(out, "  --section bytes:{start}-{end}");
    }
    let _ = writeln!(out);
}

fn find_primary_definition_with_artifact(
    target: &str,
    scope: &Path,
    artifact: ArtifactMode,
) -> Result<types::Match, SrcwalkError> {
    let raw = search::search_symbol_raw_with_artifact(target, scope, None, artifact)?;
    raw.matches
        .into_iter()
        .find(|m| m.is_definition && m.def_range.is_some())
        .ok_or_else(|| SrcwalkError::NoMatches {
            query: target.to_string(),
            scope: scope.to_path_buf(),
            suggestion: symbol_or_file_suggestion(scope, target, None),
        })
}

fn find_primary_definition(target: &str, scope: &Path) -> Result<types::Match, SrcwalkError> {
    let raw = search::search_symbol_raw(target, scope, None)?;
    raw.matches
        .into_iter()
        .find(|m| m.is_definition && m.def_range.is_some())
        .ok_or_else(|| SrcwalkError::NoMatches {
            query: target.to_string(),
            scope: scope.to_path_buf(),
            suggestion: symbol_or_file_suggestion(scope, target, None),
        })
}

fn prioritize_flow_resolves(
    mut nodes: Vec<search::callees::ResolvedCalleeNode>,
    source_path: &Path,
) -> Vec<search::callees::ResolvedCalleeNode> {
    nodes.retain(|node| is_flow_helper(&node.callee));
    nodes.sort_by_key(|node| {
        (
            flow_resolve_location_rank(&node.callee.file, source_path),
            node.callee.start_line,
            node.callee.name.clone(),
        )
    });
    nodes
}

fn flow_resolve_location_rank(path: &Path, source_path: &Path) -> u8 {
    if path == source_path {
        return 0;
    }
    if path.parent() == source_path.parent() {
        return 1;
    }
    2
}

fn is_flow_helper(callee: &search::callees::ResolvedCallee) -> bool {
    if callee.end_line > callee.start_line {
        return true;
    }
    callee.signature.as_deref().is_some_and(|sig| {
        let sig = sig.trim_start();
        sig.contains('(')
            || sig.starts_with("fn ")
            || sig.starts_with("pub fn ")
            || sig.starts_with("pub(crate) fn ")
            || sig.starts_with("async fn ")
            || sig.starts_with("pub async fn ")
            || sig.starts_with("function ")
            || sig.starts_with("def ")
            || sig.starts_with("func ")
    })
}

fn append_resolved_callee(
    out: &mut String,
    scope: &Path,
    callee: &search::callees::ResolvedCallee,
    indent: usize,
) {
    use std::fmt::Write as _;

    let rel = format::rel_nonempty(&callee.file, scope);
    let pad = "  ".repeat(indent);
    let sig = callee.signature.as_deref().unwrap_or("");
    if sig.is_empty() {
        let _ = writeln!(
            out,
            "{pad}[fn] {} {}:{}-{}",
            callee.name, rel, callee.start_line, callee.end_line
        );
    } else {
        let _ = writeln!(
            out,
            "{pad}[fn] {} {}:{}-{}  {}",
            callee.name, rel, callee.start_line, callee.end_line, sig
        );
    }
}

pub(crate) fn is_test_path(path: &Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        s == "test" || s == "tests" || s == "spec" || s == "specs" || s.contains("test")
    })
}

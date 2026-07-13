use std::collections::BTreeSet;
use std::path::Path;

use crate::cache::OutlineCache;
use crate::commands::call_format::format_call_site;
use crate::commands::context::{apply_optional_budget, ArtifactMode};
use crate::commands::decision_flow::resolve_decision_flow_target;
use crate::commands::find::symbol_or_file_suggestion;
use crate::error::SrcwalkError;
use crate::evidence::{
    confidence_label_for, render_next_actions, Anchor, EvidenceSource, NextAction,
};
use crate::lang::decision_flow::{self, TargetSelector};
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
    let resolved = resolve_decision_flow_target(target, scope)?;
    let content = std::fs::read_to_string(&resolved.path).map_err(|e| SrcwalkError::IoError {
        path: resolved.path.clone(),
        source: e,
    })?;
    let types::FileType::Code(lang) = lang::detect_file_type(&resolved.path) else {
        return Ok(format!("# Context Packet: {target}\n\n(not a code file)"));
    };

    let display_path = format::display_path(&resolved.path);
    let confidence = confidence_label_for(EvidenceSource::Ast);
    let mut out = format!("# Context Packet: {target}");
    out.push_str("\nconfidence: ");
    out.push_str(confidence);
    out.push_str("\ncaveat: source-evidence navigation only; no runtime proof");
    let packet_budget = budget_tokens;

    let (focus_range, call_target) =
        match decision_flow::render_flow_map(&resolved, &content, lang, packet_budget) {
            Ok(flow_map) => {
                append_context_flow_map(&mut out, &resolved.path, &flow_map);
                (
                    Some((flow_map.entry_start, flow_map.entry_end)),
                    Some(flow_map.entry_label.clone()),
                )
            }
            Err(err) if is_flow_map_fallback_error(&err) => {
                append_context_flow_map_fallback(&mut out, &display_path, &resolved.selector);
                (
                    selector_range(&resolved.selector),
                    context_call_target(&resolved.selector),
                )
            }
            Err(err) => return Err(err),
        };

    append_context_neighborhood(
        &mut out,
        call_target.as_deref(),
        &resolved.path,
        &content,
        lang,
        focus_range,
        scope,
        cache,
        &bloom,
        depth,
        filter,
    )?;

    let show_anchor = focus_range.map(|(start, end)| Anchor::lines(&resolved.path, start, end));
    let show_target = show_anchor
        .as_ref()
        .map_or_else(|| display_path.clone(), Anchor::display);
    let mut actions = Vec::new();
    if let Some(anchor) = show_anchor {
        actions.push(NextAction::from_evidence(
            format!("srcwalk show {show_target} -C 20"),
            "show the resolved context target source",
            10,
            EvidenceSource::Ast,
            anchor,
        ));
    } else {
        actions.push(NextAction::guidance(
            format!("srcwalk show {show_target} -C 20"),
            "show the resolved file source",
            10,
        ));
    }
    if let Some(call_target) = &call_target {
        actions.push(NextAction::from_evidence(
            format!("srcwalk trace callers {call_target}"),
            "inspect direct callers of the context target",
            20,
            EvidenceSource::Ast,
            Anchor::file(&resolved.path),
        ));
        actions.push(NextAction::from_evidence(
            format!("srcwalk trace callees {call_target} --detailed"),
            "inspect direct callees from the context target",
            30,
            EvidenceSource::Ast,
            Anchor::file(&resolved.path),
        ));
    }
    let rendered = render_next_actions(&actions);
    if !rendered.is_empty() {
        let _ = write!(out, "\n\n{rendered}");
    }
    Ok(apply_optional_budget(out, packet_budget))
}

fn append_context_flow_map(
    out: &mut String,
    path: &Path,
    flow_map: &decision_flow::RenderedFlowMap,
) {
    use std::fmt::Write as _;

    let target_anchor = Anchor::lines(path, flow_map.entry_start, flow_map.entry_end).display();
    let _ = write!(
        out,
        "\n\n## Target\n- {target_anchor} {}",
        flow_map.entry_label
    );
    out.push_str("\n\n## Flow Map\n");
    out.push_str(flow_map.body.trim_end());
    out.push('\n');

    out.push_str("\n## Exits");
    if flow_map.exits.is_empty() {
        out.push_str("\n- none structurally detected");
    } else {
        for exit in &flow_map.exits {
            let _ = write!(out, "\n- {exit}");
        }
    }
}

fn append_context_flow_map_fallback(
    out: &mut String,
    display_path: &str,
    selector: &TargetSelector,
) {
    use std::fmt::Write as _;

    out.push_str("\n\n## Target");
    if let Some((start, end)) = selector_range(selector) {
        let _ = write!(out, "\n- {display_path}:{start}-{end}");
    } else {
        let _ = write!(out, "\n- {display_path}");
    }
    out.push_str(
        "\n\n## Flow Map\nfile-level evidence only; structural function map unavailable for this target",
    );
    out.push_str("\n\n## Exits\n- not available from structural parser");
}

fn selector_range(selector: &TargetSelector) -> Option<(u32, u32)> {
    match selector {
        TargetSelector::LineRange { start, end }
        | TargetSelector::FocusedLineRange { start, end } => Some((*start, *end)),
        TargetSelector::Symbol(_) => None,
    }
}

fn context_call_target(selector: &TargetSelector) -> Option<String> {
    match selector {
        TargetSelector::Symbol(name) => Some(name.clone()),
        TargetSelector::LineRange { .. } | TargetSelector::FocusedLineRange { .. } => None,
    }
}

fn is_flow_map_fallback_error(err: &SrcwalkError) -> bool {
    match err {
        SrcwalkError::InvalidQuery { reason, .. } => {
            reason.contains("target did not resolve to a supported function-like AST node")
                || reason.contains("decision-flow requires a source code file")
                || reason.contains("symbol target did not provide a definition range")
                || reason.contains("line/range target must be inside one supported function")
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn append_context_neighborhood(
    out: &mut String,
    call_target: Option<&str>,
    source_path: &Path,
    content: &str,
    lang: types::Lang,
    focus_range: Option<(u32, u32)>,
    scope: &Path,
    cache: &OutlineCache,
    bloom: &index::bloom::BloomFilterCache,
    depth: Option<usize>,
    filter: Option<&str>,
) -> Result<(), SrcwalkError> {
    use std::fmt::Write as _;

    out.push_str("\n\n## Call Neighborhood");

    let sites = search::callees::extract_call_sites(content, lang, focus_range);
    let total_sites = sites.len();
    let sites = search::callees::filter_call_sites(sites, filter)?;
    if let Some(filter) = filter {
        let _ = writeln!(out, "\n### Callees (ordered, filtered {filter})");
    } else {
        out.push_str("\n### Callees (ordered)");
    }
    if sites.is_empty() {
        out.push_str("\n- none");
    } else {
        for site in sites.iter().take(12) {
            let _ = write!(out, "\n- {}", format_call_site(site));
        }
        if sites.len() > 12 {
            let _ = write!(out, "\n- ... {} more call sites", sites.len() - 12);
        }
    }

    append_local_structural_links(out, source_path, content, lang, focus_range, scope, &sites);
    let names = if filter.is_some() {
        sites
            .iter()
            .map(|site| site.callee.clone())
            .collect::<Vec<_>>()
    } else {
        search::callees::extract_callee_names(content, lang, focus_range)
    };
    let depth_limit = depth.map_or(1, |d| d.min(3) as u32);
    let nodes = search::callees::resolve_callees_transitive(
        &names,
        source_path,
        content,
        cache,
        bloom,
        depth_limit,
        30,
    );
    let flow_nodes = prioritize_flow_resolves(nodes, source_path);
    if !flow_nodes.is_empty() {
        out.push_str("\n\n### Resolved local callees\n");
        for node in flow_nodes.iter().take(8) {
            append_resolved_callee(out, scope, &node.callee, 1);
            for child in node.children.iter().take(2) {
                append_resolved_callee(out, scope, child, 2);
            }
        }
        if flow_nodes.len() > 8 {
            let _ = write!(
                out,
                "\n- ... {} more resolved callees",
                flow_nodes.len() - 8
            );
        }
    }

    out.push_str("\n\n### Callers");
    if let Some(call_target) = call_target {
        match search::callers::find_callers(call_target, scope, bloom, None, Some(cache)) {
            Ok(mut callers) => {
                callers.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
                if callers.is_empty() {
                    out.push_str("\n- none");
                } else {
                    for caller in callers.iter().take(8) {
                        let anchor =
                            Anchor::line(&caller.path, caller.line).display_relative_to(scope);
                        let _ = write!(out, "\n- [fn] {} {anchor}", caller.calling_function);
                    }
                    if callers.len() > 8 {
                        let _ = write!(out, "\n- ... {} more callers", callers.len() - 8);
                    }
                }
            }
            Err(_) => out.push_str("\n- unavailable"),
        }
    } else {
        out.push_str("\n- not available for non-symbol range targets");
    }

    if filter.is_some() {
        let _ = write!(
            out,
            "\n\n> Note: filter matched {}/{} call sites. Qualifiers: callee:NAME.",
            sites.len(),
            total_sites
        );
    }
    out.push_str(
        "\n\n> Caveat: static context packet is capped; verify exact edges with trace commands.",
    );
    Ok(())
}

fn append_local_structural_links(
    out: &mut String,
    source_path: &Path,
    content: &str,
    lang: types::Lang,
    focus_range: Option<(u32, u32)>,
    scope: &Path,
    sites: &[search::callees::CallSite],
) {
    use std::fmt::Write as _;

    const MAX_ROWS: usize = 12;
    let Some((start, end)) = focus_range else {
        return;
    };
    if sites.is_empty() {
        return;
    }

    let scope_id = format!("{}:{start}-{end}", format::display_path(source_path));
    let mut graphs = crate::evidence::local_links::collect_local_links_for_function_spans(
        source_path,
        content,
        lang,
        &[(&scope_id, start, end)],
    );
    let Some(graph) = graphs.pop() else {
        return;
    };
    if graph.budget_exceeded() {
        return;
    }

    let visible_calls = sites
        .iter()
        .filter_map(|site| compact_call_site_identity(site, content))
        .collect::<BTreeSet<_>>();
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();

    for argument_use in graph.links().iter().filter(|link| {
        link.kind() == crate::evidence::local_links::LocalLinkKind::ArgumentUse
            && visible_calls.contains(link.to().identity())
    }) {
        let Some(mut chain) = graph.unique_predecessor_chain(
            argument_use.from().identity(),
            crate::evidence::local_links::DEFAULT_LOCAL_LINK_MAX_HOPS,
        ) else {
            continue;
        };
        if chain.is_empty() {
            continue;
        }
        chain.push(argument_use.clone());

        for link in chain {
            let anchor = link.anchor().display_relative_to(scope);
            let key = (
                link.kind(),
                link.from().identity().to_string(),
                link.to().identity().to_string(),
                anchor.clone(),
            );
            if seen.insert(key) {
                selected.push((link, anchor));
            }
        }
    }

    selected.sort_by(|(left, _), (right, _)| {
        left.anchor()
            .start_line()
            .cmp(&right.anchor().start_line())
            .then(left.kind().cmp(&right.kind()))
            .then(left.from().identity().cmp(right.from().identity()))
            .then(left.to().identity().cmp(right.to().identity()))
    });

    if selected.is_empty() {
        return;
    }

    out.push_str("\n\n### Local structural links");
    let _ = write!(out, "\nconfidence: {}", selected[0].0.confidence());
    out.push_str("\ncaveat: same-function structural links only; not runtime dataflow");
    for (link, anchor) in selected.iter().take(MAX_ROWS) {
        let _ = write!(
            out,
            "\n- {} -> {} [{}] {anchor}",
            link.from().identity(),
            link.to().identity(),
            link.kind().as_str()
        );
    }
    if selected.len() > MAX_ROWS {
        let _ = write!(
            out,
            "\n- ... {} more local structural links omitted",
            selected.len() - MAX_ROWS
        );
    }
}

fn compact_call_site_identity(site: &search::callees::CallSite, content: &str) -> Option<String> {
    let text = site
        .call_byte_range
        .and_then(|(start, end)| content.get(start..end))
        .unwrap_or(&site.call_text);
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!compact.is_empty()).then_some(compact)
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
            "# Context: {target} — artifact\n\n(not a code file)"
        ));
    };

    let rel = format::rel_nonempty(&def_match.path, scope);
    let mut out = format!(
        "# Context: {target} — artifact\n\n[symbol] {target} {rel}:{}\n",
        def_match.line
    );
    let _ = writeln!(
        out,
        "  section: srcwalk {} --artifact --section {}",
        format::display_path(&def_match.path),
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
    let rendered = render_next_actions(&[NextAction::guidance(
        "use `srcwalk <path> --artifact --section <symbol|bytes:start-end>`, `srcwalk trace callers <symbol> --artifact --expand=1`, or `srcwalk trace callees <symbol> --artifact --detailed`.",
        "artifact flow drilldown",
        40,
    )]);
    if !rendered.is_empty() {
        out.push('\n');
        out.push_str(&rendered);
    }
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
            guidance: None,
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

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::cache::OutlineCache;
use crate::commands::context::ArtifactMode;
use crate::error::SrcwalkError;
use crate::evidence::{render_next_actions, NextAction};
use crate::lang::tsconfig::ConfigCache;
use crate::{budget, format, index, lang, search, types};

use crate::commands::call_format::{
    format_call_site, format_direct_call_edge, format_direct_call_unknown,
};
use crate::commands::find::symbol_or_file_suggestion;
use crate::commands::pathsymbol::{self, PathSymbolOutcome};

/// Show what a symbol calls (forward call graph).
pub(crate) fn run_callees(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    depth: Option<usize>,
    detailed: bool,
    filter: Option<&str>,
) -> Result<String, SrcwalkError> {
    run_callees_with_artifact(
        target,
        scope,
        budget_tokens,
        cache,
        depth,
        detailed,
        filter,
        ArtifactMode::Source,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_callees_with_artifact(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    depth: Option<usize>,
    detailed: bool,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    if artifact.enabled() && matches!(depth, Some(d) if d >= 2) {
        return Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "--artifact callees currently supports direct call evidence only; omit --depth"
                .to_string(),
        });
    }

    // A canonical `path:symbol` root names its own definition, so use that exact
    // path/range directly. Global uniqueness is NOT required: a same-name definition
    // in another file no longer blocks the exact root. Missing / unreadable /
    // unresolved / ambiguous / `::` roots surface the shared explicit intent instead
    // of silently broadening to a bare-name search.
    let exact_root = match pathsymbol::resolve_path_symbol_outcome(target, scope) {
        PathSymbolOutcome::Error(err) | PathSymbolOutcome::UnresolvedInNamedFile(err) => {
            return Err(err)
        }
        PathSymbolOutcome::Unique {
            path,
            symbol,
            start_line,
            end_line,
        } => Some((path, symbol, (start_line as u32, end_line as u32))),
        PathSymbolOutcome::FallThrough => None,
    };

    // The path only IDENTIFIES the root, so it may name a definition outside
    // --scope. Callee resolution and traversal stay bounded by --scope, so label
    // the boundary rather than implicitly widening it. Applying the note at this
    // single exit is what stops any render shape (detailed, no-call, direct, or
    // transitive) from dropping it on an early return.
    let outside_scope_note = exact_root
        .as_ref()
        .filter(|(path, _, _)| !path.starts_with(scope))
        .map(|(path, _, _)| {
            format!(
                "\n> Note: the exact root {} lies outside --scope {}; callee resolution and traversal stay inside --scope only.",
                format::display_path(path),
                format::display_path(scope)
            )
        });

    let body = render_callees(
        target, scope, cache, depth, detailed, filter, artifact, exact_root,
    )?;
    let output = match outside_scope_note {
        Some(note) => format!("{body}{note}"),
        None => body,
    };
    match budget_tokens {
        Some(b) => Ok(budget::apply_preserving_footer(&output, b)),
        None => Ok(output),
    }
}

/// Render the callee report body for an already-resolved root.
#[allow(clippy::too_many_arguments)]
fn render_callees(
    target: &str,
    scope: &Path,
    cache: &OutlineCache,
    depth: Option<usize>,
    detailed: bool,
    filter: Option<&str>,
    artifact: ArtifactMode,
    exact_root: Option<(PathBuf, String, (u32, u32))>,
) -> Result<String, SrcwalkError> {
    use std::fmt::Write;
    let bloom = index::bloom::BloomFilterCache::new();
    let from_exact_path = exact_root.is_some();

    let (def_path, def_range, lookup_target) = if let Some((path, symbol, range)) = exact_root {
        (path, Some(range), symbol)
    } else {
        let raw = search::search_symbol_raw_with_artifact(target, scope, None, artifact)?;
        let def_match = raw
            .matches
            .into_iter()
            .find(|m| m.is_definition && m.def_range.is_some())
            .ok_or_else(|| SrcwalkError::NoMatches {
                query: target.to_string(),
                scope: scope.to_path_buf(),
                suggestion: symbol_or_file_suggestion(scope, target, None),
                guidance: None,
            })?;
        (def_match.path, def_match.def_range, target.to_string())
    };

    let content = std::fs::read_to_string(&def_path).map_err(|e| SrcwalkError::IoError {
        path: def_path.clone(),
        source: e,
    })?;

    let file_type = lang::detect_file_type(&def_path);
    let types::FileType::Code(lang) = file_type else {
        return Ok(format!("# Callees: {target}\n\n(not a code file)"));
    };

    let config_cache = ConfigCache::new();
    let logical_sources = if matches!(
        lang,
        types::Lang::JavaScript | types::Lang::TypeScript | types::Lang::Tsx
    ) {
        lang::js_imports::logical_sources(&content, lang)
    } else {
        Vec::new()
    };
    let decisions = if logical_sources.is_empty() {
        None
    } else {
        Some(crate::read::js_alias::classify_js_imports(
            &def_path,
            &logical_sources,
            scope,
            &config_cache,
        ))
    };

    let rel = format::rel_nonempty(&def_path, scope);

    // Detailed mode: ordered call sites with args + assignment context.
    if detailed {
        let sites = if artifact.enabled() {
            search::callees::extract_call_sites_for_artifact_target(
                &content,
                lang,
                &lookup_target,
                def_range,
            )
        } else {
            search::callees::extract_call_sites(&content, lang, def_range)
        };
        let total_sites = sites.len();
        let sites = search::callees::filter_call_sites(sites, filter)?;
        let direct_calls = (!artifact.enabled()).then(|| {
            crate::evidence::direct_call::build_direct_call_evidence_index(
                &def_path, &content, lang, def_range, &sites,
            )
        });
        if sites.is_empty() {
            let suffix = filter.map_or(String::new(), |f| format!(" matching `{f}`"));
            return Ok(format!(
                "# Callees: {target} ({rel})\n\n(no calls found{suffix})"
            ));
        }
        let filter_suffix = filter.map_or(String::new(), |f| format!(" matching `{f}`"));
        let mut out = format!("# Callees: {target} ({rel}){filter_suffix}\n");
        let js_ts_artifact = artifact.enabled()
            && matches!(
                lang,
                types::Lang::JavaScript | types::Lang::TypeScript | types::Lang::Tsx
            );
        for s in &sites {
            if js_ts_artifact {
                let _ = write!(out, "\n{}", format_artifact_call_site(s, &content));
            } else {
                let _ = write!(out, "\n{}", format_call_site(s));
            }
            if let Some(index) = &direct_calls {
                if let Some(edge) = index.edge_for_site(s, &content) {
                    let _ = write!(out, "\n{}", format_direct_call_edge(edge, scope, 2));
                } else if let Some(unknown) = index.unknown_for_site(s, &content) {
                    let _ = write!(out, "\n{}", format_direct_call_unknown(unknown, scope, 2));
                }
            }
        }
        if let Some(index) = &direct_calls {
            let omitted = index
                .omitted_edges()
                .saturating_add(index.omitted_unknowns());
            if omitted > 0 {
                let _ = write!(
                    out,
                    "\n\n> Note: {omitted} direct-call evidence rows omitted."
                );
            }
            if index.omitted_related_files() > 0 {
                let _ = write!(
                    out,
                    "\n> Note: {} related files omitted from direct-call resolution.",
                    index.omitted_related_files()
                );
            }
        }

        if filter.is_some() {
            let _ = write!(
                out,
                "\n\n> Note: filter matched {}/{} call sites. Qualifiers: callee:NAME.",
                sites.len(),
                total_sites
            );
        } else if js_ts_artifact {
            let rendered = render_next_actions(&[NextAction::guidance(
                "drill into call evidence with `srcwalk <path> --artifact --section bytes:<start>-<end>`, or omit --detailed for resolved callee summaries.",
                "artifact callee evidence drilldown",
                40,
            )]);
            if !rendered.is_empty() {
                let _ = write!(out, "\n\n{rendered}");
            }
        } else {
            out.push_str("\n\n> Caveat: detailed call sites can be long. Retry with --budget <N>, or omit --detailed for resolved callee summaries.");
        }
        if let Some(note) = artifact.callees_note() {
            out.push_str("\n> ");
            out.push_str(note);
        }
        return Ok(out);
    }

    // Default mode: resolved callees with transitive expansion.
    let callee_names = if artifact.enabled() {
        search::callees::extract_callee_names_for_artifact_target(
            &content,
            lang,
            &lookup_target,
            def_range,
        )
    } else {
        search::callees::extract_callee_names(&content, lang, def_range)
    };
    if callee_names.is_empty() {
        return Ok(format!(
            "# Callees: {target} (in {rel})\n\n(no calls found)"
        ));
    }

    let depth_limit = depth.map_or(1, |d| d.min(5) as u32);
    let nodes = if artifact.enabled() {
        search::callees::resolve_callees_same_file_artifact(
            &lookup_target,
            &def_path,
            &content,
            lang,
            &callee_names,
        )
        .unwrap_or_else(|| {
            search::callees::resolve_callees_same_file(&callee_names, &def_path, &content, lang)
        })
        .into_iter()
        .map(|callee| search::callees::ResolvedCalleeNode {
            callee,
            children: Vec::new(),
        })
        .collect()
    } else {
        search::callees::resolve_callees_transitive_with_stream(
            &callee_names,
            &def_path,
            &content,
            &logical_sources,
            decisions.as_deref(),
            cache,
            &bloom,
            depth_limit,
            50,
            scope,
            &config_cache,
        )
    };

    let mut out = format!("# Callees: {target} (in {rel})\n");
    // Only the root body came from the named file; every deeper hop is resolved
    // from a callee name, so say that before rendering transitive children.
    if from_exact_path && depth_limit >= 2 {
        let _ = writeln!(
            out,
            "> Caveat: only the root is exact (`{lookup_target}` in the named file); every later hop resolves by name."
        );
    }

    // Unresolved callees
    let resolved_names: std::collections::HashSet<&str> =
        nodes.iter().map(|n| n.callee.name.as_str()).collect();
    let unresolved: Vec<&String> = callee_names
        .iter()
        .filter(|n| !resolved_names.contains(n.as_str()))
        .collect();

    for node in &nodes {
        let c = &node.callee;
        let rel_c = format::rel_nonempty(&c.file, scope);
        let sig = c.signature.as_deref().unwrap_or("");
        let _ = write!(
            out,
            "\n  {:<30} {}:{}-{}",
            c.name, rel_c, c.start_line, c.end_line
        );
        if !sig.is_empty() {
            let _ = write!(out, "  {sig}");
        }
        for child in &node.children {
            let rel_ch = format::rel_nonempty(&child.file, scope);
            let _ = write!(
                out,
                "\n    {:<28} {}:{}-{}",
                child.name, rel_ch, child.start_line, child.end_line
            );
            if let Some(ref s) = child.signature {
                let _ = write!(out, "  {s}");
            }
        }
    }

    if !unresolved.is_empty() {
        let sites = if artifact.enabled() {
            search::callees::extract_call_sites_for_artifact_target(
                &content,
                lang,
                &lookup_target,
                def_range,
            )
        } else {
            search::callees::extract_call_sites(&content, lang, def_range)
        };
        append_unresolved_call_site_evidence(&mut out, &unresolved, &sites);
    }

    let rendered = render_next_actions(&[NextAction::guidance(
        "use --detailed for ordered call sites with args and assignments",
        "callee call-site drilldown",
        40,
    )]);
    if !rendered.is_empty() {
        let _ = write!(out, "\n\n{rendered}");
    }
    if let Some(note) = artifact.callees_note() {
        out.push_str("\n> ");
        out.push_str(note);
    }

    Ok(out)
}

fn append_unresolved_call_site_evidence(
    out: &mut String,
    unresolved: &[&String],
    sites: &[search::callees::CallSite],
) {
    const LIMIT: usize = 12;
    let unresolved_names = unresolved
        .iter()
        .map(|name| name.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut unresolved_sites = sites
        .iter()
        .filter(|site| unresolved_names.contains(site.callee.as_str()))
        .collect::<Vec<_>>();
    unresolved_sites.sort_by_key(|site| (site.line, site.callee.as_str()));

    if unresolved_sites.is_empty() {
        out.push_str("\n\n  (unresolved; call-site reason not classified): ");
        out.push_str(
            &unresolved
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        return;
    }

    out.push_str("\n\n  unresolved call sites (reason not classified):");
    let rendered_names = unresolved_sites
        .iter()
        .take(LIMIT)
        .map(|site| site.callee.as_str())
        .collect::<std::collections::HashSet<_>>();
    for site in unresolved_sites.iter().take(LIMIT) {
        let _ = write!(out, "\n    {}", format_call_site(site));
    }
    if unresolved_sites.len() > LIMIT {
        let _ = write!(
            out,
            "\n    ... {} more unresolved call sites",
            unresolved_sites.len() - LIMIT
        );
    }

    let unrendered_names = unresolved
        .iter()
        .map(|name| name.as_str())
        .filter(|name| !rendered_names.contains(*name))
        .collect::<Vec<_>>();
    if !unrendered_names.is_empty() {
        let _ = write!(
            out,
            "\n    unresolved names without rendered call-site rows: {}",
            unrendered_names.join(", ")
        );
    }
}

fn format_artifact_call_site(site: &search::callees::CallSite, content: &str) -> String {
    let mut out = format_call_site(site);
    let Some((start_byte, end_byte)) = site.call_byte_range else {
        return out;
    };
    let _ = write!(out, " --section bytes:{start_byte}-{end_byte}");
    out.push_str(&format_artifact_call_window(
        content, site.line, start_byte, end_byte,
    ));
    out
}

fn format_artifact_call_window(
    content: &str,
    line: u32,
    start_byte: usize,
    end_byte: usize,
) -> String {
    const CONTEXT: usize = 80;
    const MAX_WINDOW: usize = 360;

    if start_byte >= end_byte || start_byte >= content.len() {
        return String::new();
    }
    let end_byte = end_byte.min(content.len());
    let mut window_start = start_byte.saturating_sub(CONTEXT);
    let mut window_end = (end_byte + CONTEXT).min(content.len());
    if window_end.saturating_sub(window_start) > MAX_WINDOW {
        window_start = start_byte.saturating_sub(MAX_WINDOW / 3);
        window_end = (end_byte + MAX_WINDOW / 3).min(content.len());
    }
    window_start = floor_char_boundary(content, window_start);
    window_end = ceil_char_boundary(content, window_end);

    let prefix = if window_start > 0 { "…" } else { "" };
    let suffix = if window_end < content.len() {
        "…"
    } else {
        ""
    };
    let snippet = content[window_start..window_end].trim();
    format!(
        "\n```js\n// line {line}, bytes {start_byte}-{end_byte}\n{prefix}{snippet}{suffix}\n```"
    )
}

fn floor_char_boundary(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx < text.len() && !text.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

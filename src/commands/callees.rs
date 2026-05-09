use std::path::Path;

use crate::cache::OutlineCache;
use crate::commands::context::ArtifactMode;
use crate::error::SrcwalkError;
use crate::{budget, format, index, lang, search, types};

use crate::commands::call_format::format_call_site;
use crate::commands::find::symbol_or_file_suggestion;

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
    use std::fmt::Write;
    if artifact.enabled() && matches!(depth, Some(d) if d >= 2) {
        return Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "--artifact callees currently supports direct call evidence only; omit --depth"
                .to_string(),
        });
    }
    let bloom = index::bloom::BloomFilterCache::new();

    // Find definition of target symbol
    let raw = search::search_symbol_raw_with_artifact(target, scope, None, artifact)?;
    let def_match = raw
        .matches
        .iter()
        .find(|m| m.is_definition && m.def_range.is_some())
        .ok_or_else(|| SrcwalkError::NoMatches {
            query: target.to_string(),
            scope: scope.to_path_buf(),
            suggestion: symbol_or_file_suggestion(scope, target, None),
        })?;

    let content = std::fs::read_to_string(&def_match.path).map_err(|e| SrcwalkError::IoError {
        path: def_match.path.clone(),
        source: e,
    })?;

    let file_type = lang::detect_file_type(&def_match.path);
    let types::FileType::Code(lang) = file_type else {
        return Ok(format!("# Callees: {target}\n\n(not a code file)"));
    };

    let rel = format::rel_nonempty(&def_match.path, scope);

    // Detailed mode: ordered call sites with args + assignment context.
    if detailed {
        let sites = if artifact.enabled() {
            search::callees::extract_call_sites_for_artifact_target(
                &content,
                lang,
                target,
                def_match.def_range,
            )
        } else {
            search::callees::extract_call_sites(&content, lang, def_match.def_range)
        };
        let total_sites = sites.len();
        let sites = search::callees::filter_call_sites(sites, filter)?;
        if sites.is_empty() {
            let suffix = filter.map_or(String::new(), |f| format!(" matching `{f}`"));
            return Ok(format!(
                "# Callees: {target} ({rel})\n\n(no calls found{suffix})"
            ));
        }
        let filter_suffix = filter.map_or(String::new(), |f| format!(" matching `{f}`"));
        let mut out = format!("# Callees: {target} ({rel}){filter_suffix}\n");
        for s in &sites {
            let _ = write!(out, "\n{}", format_call_site(s));
        }
        if filter.is_some() {
            let _ = write!(
                out,
                "\n\n> Note: filter matched {}/{} call sites. Qualifiers: callee:NAME.",
                sites.len(),
                total_sites
            );
        } else {
            out.push_str("\n\n> Caveat: detailed call sites can be long. Retry with --budget <N>, or omit --detailed for resolved callee summaries.");
        }
        if let Some(note) = artifact.callees_note() {
            out.push_str("\n> ");
            out.push_str(note);
        }
        let output = match budget_tokens {
            Some(b) => budget::apply_preserving_footer(&out, b),
            None => out,
        };
        return Ok(output);
    }

    // Default mode: resolved callees with transitive expansion.
    let callee_names = if artifact.enabled() {
        search::callees::extract_callee_names_for_artifact_target(
            &content,
            lang,
            target,
            def_match.def_range,
        )
    } else {
        search::callees::extract_callee_names(&content, lang, def_match.def_range)
    };
    if callee_names.is_empty() {
        return Ok(format!(
            "# Callees: {target} (in {rel})\n\n(no calls found)"
        ));
    }

    let depth_limit = depth.map_or(1, |d| d.min(5) as u32);
    let nodes = if artifact.enabled() {
        search::callees::resolve_callees_same_file_artifact(
            target,
            &def_match.path,
            &content,
            lang,
            &callee_names,
        )
        .unwrap_or_else(|| {
            search::callees::resolve_callees_same_file(
                &callee_names,
                &def_match.path,
                &content,
                lang,
            )
        })
        .into_iter()
        .map(|callee| search::callees::ResolvedCalleeNode {
            callee,
            children: Vec::new(),
        })
        .collect()
    } else {
        search::callees::resolve_callees_transitive(
            &callee_names,
            &def_match.path,
            &content,
            cache,
            &bloom,
            depth_limit,
            50,
        )
    };

    let mut out = format!("# Callees: {target} (in {rel})\n");

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
        out.push_str("\n\n  (unresolved): ");
        out.push_str(
            &unresolved
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    out.push_str("\n\n> Next: use --detailed for ordered call sites with args and assignments");
    if let Some(note) = artifact.callees_note() {
        out.push_str("\n> ");
        out.push_str(note);
    }

    let output = match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&out, b),
        None => out,
    };
    Ok(output)
}

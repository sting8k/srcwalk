use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

use crate::budget;
use crate::cache::OutlineCache;
use crate::commands::decision_flow::resolve_decision_flow_target;
use crate::commands::diff::{self, DiffEvidence, DiffFile, DiffMode, DiffStatus, EnclosingSymbol};
use crate::error::SrcwalkError;
use crate::evidence::{
    confidence_label_for, render_next_actions, Anchor, EvidenceSource, NextAction,
};
use crate::format;
use crate::lang::{self, decision_flow, decision_flow::FlowTarget, decision_flow::TargetSelector};
use crate::types::{estimate_tokens, FileType, OutlineKind};

const DEFAULT_CHANGED_FILE_LIMIT: usize = 20;
const MAX_FLOW_MAPS: usize = 5;
const MAX_READ_NEXT_TARGETS: usize = 3;
const DIFF_METADATA_LABEL: &str = "diff metadata";

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_review(
    target: Option<&str>,
    staged: bool,
    scope: &Path,
    scope_glob: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    _cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let target = target.map(str::trim).filter(|target| !target.is_empty());

    let mut out = match (staged, target) {
        (true, Some(target)) => {
            return Err(SrcwalkError::InvalidQuery {
                query: target.to_string(),
                reason: "--staged cannot be combined with a review target or revision range"
                    .to_string(),
            });
        }
        (true, None) => {
            run_change_review(None, true, scope, scope_glob, budget_tokens, limit, offset)?
        }
        (false, None) => {
            run_change_review(None, false, scope, scope_glob, budget_tokens, limit, offset)?
        }
        (false, Some(target)) if diff::is_explicit_rev_range(target) => run_change_review(
            Some(target),
            false,
            scope,
            scope_glob,
            budget_tokens,
            limit,
            offset,
        )?,
        (false, Some(target)) => {
            if limit.is_some() || offset > 0 || scope_glob.is_some() {
                return Err(SrcwalkError::InvalidQuery {
                    query: target.to_string(),
                    reason: "local review supports --scope and --budget only; --limit/--offset and glob scopes apply to change review".to_string(),
                });
            }
            run_local_review(target, scope, budget_tokens)?
        }
    };

    if let Some(budget) = budget_tokens {
        out = apply_review_budget(&out, budget);
    }
    Ok(out)
}

fn run_local_review(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
) -> Result<String, SrcwalkError> {
    let resolved = resolve_decision_flow_target(target, scope)?;
    let content =
        std::fs::read_to_string(&resolved.path).map_err(|source| SrcwalkError::IoError {
            path: resolved.path.clone(),
            source,
        })?;
    let display_path = format::display_path(&resolved.path);

    let mut out = format!("# Review Packet: {target}");
    out.push_str("\nconfidence: structural syntax");
    out.push_str("\ncaveat: source-evidence navigation only; no runtime proof");

    match lang::detect_file_type(&resolved.path) {
        FileType::Code(lang) => {
            match decision_flow::render_flow_map(&resolved, &content, lang, budget_tokens) {
                Ok(flow_map) => {
                    append_flow_map_sections(&mut out, &display_path, &resolved.path, &flow_map);
                }
                Err(err) if is_flow_map_fallback_error(&err) => {
                    append_file_level_flow_map(&mut out, &display_path, &resolved);
                }
                Err(err) => return Err(err),
            }
        }
        _ => append_file_level_flow_map(&mut out, &display_path, &resolved),
    }

    append_local_read_next(&mut out, &display_path);
    append_token_footer(&mut out);
    Ok(out)
}

fn append_flow_map_sections(
    out: &mut String,
    display_path: &str,
    path: &Path,
    flow_map: &decision_flow::RenderedFlowMap,
) {
    let _ = write!(
        out,
        "\n\n## target\n- {display_path}:{}-{} {}",
        flow_map.entry_start, flow_map.entry_end, flow_map.entry_label
    );
    out.push_str("\n\n## flow map\n");
    out.push_str(flow_map.body.trim_end());
    out.push('\n');

    out.push_str("\n## exits");
    if flow_map.exits.is_empty() {
        out.push_str("\n- none structurally detected");
    } else {
        for exit in &flow_map.exits {
            let _ = write!(out, "\n- {exit}");
        }
    }

    let anchor = Anchor::lines(path, flow_map.entry_start, flow_map.entry_end);
    let rendered = render_next_actions(&[NextAction::from_evidence(
        format!(
            "srcwalk show {display_path}:{}-{} -C 20",
            flow_map.entry_start, flow_map.entry_end
        ),
        "read reviewed flow-map source range",
        10,
        EvidenceSource::Ast,
        anchor,
    )]);
    if !rendered.is_empty() {
        let _ = write!(out, "\n\n{rendered}");
    }
}

fn append_file_level_flow_map(out: &mut String, display_path: &str, resolved: &FlowTarget) {
    out.push_str("\n\n## target");
    if let Some(range) = selector_range(&resolved.selector) {
        let _ = write!(out, "\n- {display_path}:{range}");
    } else {
        let _ = write!(out, "\n- {display_path}");
    }
    out.push_str("\n\n## flow map\nfile-level evidence only; structural function map unavailable for this target");
    out.push_str("\n\n## exits\n- not available from structural parser");
}

fn append_local_read_next(out: &mut String, display_path: &str) {
    if !out.contains("\n> Next:") {
        let rendered = render_next_actions(&[NextAction::guidance(
            format!("srcwalk show {display_path} -C 20"),
            "read reviewed file source",
            10,
        )]);
        if !rendered.is_empty() {
            let _ = write!(out, "\n\n{rendered}");
        }
    }
}

fn selector_range(selector: &TargetSelector) -> Option<String> {
    match selector {
        TargetSelector::LineRange { start, end }
        | TargetSelector::FocusedLineRange { start, end } => Some(display_range(*start, *end)),
        TargetSelector::Symbol(_) => None,
    }
}

fn is_flow_map_fallback_error(err: &SrcwalkError) -> bool {
    matches!(
        err,
        SrcwalkError::InvalidQuery { reason, .. }
            if reason.contains("requires tree-sitter source support")
                || reason.contains("currently supports")
    )
}

fn run_change_review(
    rev_range: Option<&str>,
    staged: bool,
    scope: &Path,
    scope_glob: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
) -> Result<String, SrcwalkError> {
    let mode = if staged {
        DiffMode::Staged
    } else if rev_range.is_some() {
        DiffMode::Range
    } else {
        DiffMode::Working
    };
    let evidence = diff::collect_diff_evidence(rev_range, mode, scope, scope_glob)?;
    let page_size = limit.unwrap_or(DEFAULT_CHANGED_FILE_LIMIT);
    let shown_files: Vec<&DiffFile> = evidence.files.iter().skip(offset).take(page_size).collect();
    let changed_targets = changed_function_targets(&shown_files);
    let shown_hunks = shown_files
        .iter()
        .map(|file| file.hunks.len())
        .sum::<usize>();

    let mut out = format!("# Review Packet: {}", evidence.title());
    let _ = write!(
        out,
        "\nconfidence: structural syntax + diff metadata\ncaveat: source-evidence navigation only; no runtime proof\nfiles: changed={} shown={}\nhunks: total={} shown={}\nsymbols: total={} shown={}",
        evidence.total_files,
        shown_files.len(),
        evidence.total_hunks,
        shown_hunks,
        evidence.total_symbols,
        changed_targets.len()
    );

    append_changed_evidence(&mut out, &shown_files);
    append_changed_symbols(&mut out, &changed_targets);
    append_change_flow_maps(&mut out, &evidence, &changed_targets, budget_tokens);
    append_change_omitted(
        &mut out,
        &evidence,
        scope,
        scope_glob,
        shown_files.len(),
        changed_targets.len(),
        limit,
        offset,
    );
    append_change_read_next(&mut out, &shown_files, &changed_targets);
    append_token_footer(&mut out);
    Ok(out)
}

#[derive(Clone)]
struct ChangedTarget<'a> {
    file: &'a DiffFile,
    symbol: &'a EnclosingSymbol,
}

fn changed_function_targets<'a>(files: &[&'a DiffFile]) -> Vec<ChangedTarget<'a>> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for file in files {
        if file.status == DiffStatus::Deleted {
            continue;
        }
        for hunk in &file.hunks {
            let Some(symbol) = &hunk.symbol else {
                continue;
            };
            if symbol.kind != OutlineKind::Function {
                continue;
            }
            let key = (
                file.path.clone(),
                symbol.name.clone(),
                symbol.start_line,
                symbol.end_line,
            );
            if seen.insert(key) {
                targets.push(ChangedTarget { file, symbol });
            }
        }
    }
    targets
}

fn append_changed_evidence(out: &mut String, files: &[&DiffFile]) {
    out.push_str("\n\n## changed evidence");
    if files.is_empty() {
        out.push_str("\nNo diff evidence in selected scope.");
        return;
    }
    for file in files {
        let _ = write!(
            out,
            "\n\n### {}\nstatus: {}",
            file.path,
            file.status.as_str()
        );
        if let Some(old_path) = file.old_path.as_ref().filter(|old| *old != &file.path) {
            let _ = write!(out, "\nold-path: {old_path}");
        }
        out.push_str("\nhunks:");
        for hunk in &file.hunks {
            append_changed_hunk_evidence(out, file, hunk);
        }
    }
}

fn append_changed_hunk_evidence(out: &mut String, file: &DiffFile, hunk: &diff::DiffHunk) {
    let range = diff::display_hunk_range(hunk);
    let provenance = hunk_provenance(file, hunk);
    if let Some(symbol) = &hunk.symbol {
        let confidence = confidence_label_for(EvidenceSource::Ast);
        let _ = write!(
            out,
            "\n- {range} inside {} | source: {DIFF_METADATA_LABEL} | provenance: {provenance}\n  context: {} :{}-{} | confidence: {confidence}",
            symbol.name, symbol.name, symbol.start_line, symbol.end_line
        );
    } else {
        let _ = write!(
            out,
            "\n- {range} file-level | source: {DIFF_METADATA_LABEL} | provenance: {provenance}"
        );
    }
}

fn hunk_provenance(file: &DiffFile, hunk: &diff::DiffHunk) -> String {
    if hunk.new_lines == 0 {
        format!(
            "{}:old:{}",
            file.path,
            diff::display_line_span(hunk.old_start, hunk.old_lines)
        )
    } else {
        format!(
            "{}:{}",
            file.path,
            diff::display_line_span(hunk.new_start, hunk.new_lines)
        )
    }
}

fn append_changed_symbols(out: &mut String, targets: &[ChangedTarget<'_>]) {
    out.push_str("\n\n## changed symbols");
    if targets.is_empty() {
        out.push_str("\n- none function-like in selected diff evidence");
        return;
    }
    for target in targets {
        let hunk = target
            .file
            .hunks
            .iter()
            .find(|hunk| hunk.symbol.as_ref() == Some(target.symbol));
        let changed = hunk.map_or_else(|| "unknown".to_string(), diff::display_changed_line_span);
        let _ = write!(
            out,
            "\n- {} :{}-{} modified lines {}",
            target.symbol.name, target.symbol.start_line, target.symbol.end_line, changed
        );
    }
}

fn append_change_flow_maps(
    out: &mut String,
    evidence: &DiffEvidence,
    targets: &[ChangedTarget<'_>],
    budget_tokens: Option<u64>,
) {
    out.push_str("\n\n## flow maps");
    if targets.is_empty() {
        out.push_str("\n- none rendered; no changed function-like symbols in selected files");
        return;
    }

    let shown = targets.len().min(MAX_FLOW_MAPS);
    let omitted = targets.len().saturating_sub(MAX_FLOW_MAPS);
    let confidence = confidence_label_for(EvidenceSource::Ast);
    let _ = write!(
        out,
        "\nbounds: changed function targets; shown={shown} omitted={omitted} cap={MAX_FLOW_MAPS}; confidence: {confidence}"
    );

    for target in targets.iter().take(MAX_FLOW_MAPS) {
        let display_target = format!("{}:{}", target.file.path, target.symbol.name);
        let _ = write!(
            out,
            "\n\n### {display_target}\nprovenance: post-change {}:{}-{} | confidence: {confidence}",
            target.file.path, target.symbol.start_line, target.symbol.end_line
        );
        let Some(content) = diff::after_content(
            &evidence.repo_root,
            &target.file.path,
            evidence.rev_range.as_deref(),
            evidence.mode,
        ) else {
            out.push_str("\nflow map unavailable; changed file content could not be read");
            continue;
        };
        let path = evidence.repo_root.join(&target.file.path);
        let FileType::Code(lang) = lang::detect_file_type(&path) else {
            out.push_str(
                "\nfile-level evidence only; structural function map unavailable for this file",
            );
            continue;
        };
        let flow_target = FlowTarget {
            path,
            display_target: display_target.clone(),
            selector: TargetSelector::LineRange {
                start: target.symbol.start_line,
                end: target.symbol.end_line,
            },
        };
        match decision_flow::render_flow_map(&flow_target, &content, lang, budget_tokens) {
            Ok(flow_map) => append_embedded_flow_map(out, &target.file.path, &flow_map),
            Err(err) if is_flow_map_fallback_error(&err) => {
                out.push_str("\nfile-level evidence only; structural function map unavailable for this target");
            }
            Err(err) => {
                let _ = write!(out, "\nflow map unavailable: {err}");
            }
        }
    }
}

fn append_embedded_flow_map(
    out: &mut String,
    display_path: &str,
    flow_map: &decision_flow::RenderedFlowMap,
) {
    let _ = write!(
        out,
        "\ntarget: {display_path}:{}-{} {}\n\nflow map:\n{}",
        flow_map.entry_start,
        flow_map.entry_end,
        flow_map.entry_label,
        flow_map.body.trim_end()
    );
    out.push_str("\n\nexits:");
    if flow_map.exits.is_empty() {
        out.push_str("\n- none structurally detected");
    } else {
        for exit in &flow_map.exits {
            let _ = write!(out, "\n- {exit}");
        }
    }
}

fn append_change_omitted(
    out: &mut String,
    evidence: &DiffEvidence,
    scope: &Path,
    scope_glob: Option<&str>,
    shown_files: usize,
    changed_targets: usize,
    limit: Option<usize>,
    offset: usize,
) {
    out.push_str("\n\n## omitted");
    let next_offset = offset.saturating_add(shown_files);
    let omitted_files = evidence.total_files.saturating_sub(next_offset);
    let omitted_flow_maps = changed_targets.saturating_sub(MAX_FLOW_MAPS);
    let _ = write!(
        out,
        "\n- files: {omitted_files}\n- flow maps: {omitted_flow_maps}"
    );
    if let Some(limit) = limit.filter(|_| omitted_files > 0) {
        let base = change_review_base_command(evidence, scope, scope_glob);
        let rendered = render_next_actions(&[NextAction::metadata(
            format!("{omitted_files} more changed files. Continue with {base} --offset {next_offset} --limit {limit}."),
            "changed-file pagination",
            10,
        )]);
        if !rendered.is_empty() {
            let _ = write!(out, "\n\n{rendered}");
        }
    }
}

fn append_change_read_next(out: &mut String, files: &[&DiffFile], targets: &[ChangedTarget<'_>]) {
    out.push('\n');

    let mut actions = Vec::new();
    if let Some(file) = files.first() {
        if let Some(hunk) = file.hunks.first() {
            let range = if hunk.new_lines == 0 {
                diff::display_line_span(hunk.old_start, hunk.old_lines)
            } else {
                diff::display_line_span(hunk.new_start, hunk.new_lines)
            };
            actions.push(NextAction::from_evidence(
                format!("srcwalk show {}:{range} -C 20", file.path),
                "read first changed hunk source",
                10,
                EvidenceSource::Text,
                Anchor::file(Path::new(&file.path)),
            ));
        }
    }
    for (idx, target) in targets.iter().take(MAX_READ_NEXT_TARGETS).enumerate() {
        actions.push(NextAction::from_evidence(
            format!("srcwalk review {}:{}", target.file.path, target.symbol.name),
            "review changed function target",
            20 + idx as u16,
            EvidenceSource::Ast,
            Anchor::lines(
                Path::new(&target.file.path),
                target.symbol.start_line,
                target.symbol.end_line,
            ),
        ));
    }

    let rendered = render_next_actions(&actions);
    if rendered.is_empty() {
        let rendered = render_next_actions(&[NextAction::guidance(
            "none; selected change scope has no source reads",
            "selected change scope has no source reads",
            90,
        )]);
        if !rendered.is_empty() {
            let _ = write!(out, "\n{rendered}");
        }
    } else {
        let _ = write!(out, "\n{rendered}");
    }
}

fn change_review_base_command(
    evidence: &DiffEvidence,
    scope: &Path,
    scope_glob: Option<&str>,
) -> String {
    let scope_arg = scope_arg(scope, scope_glob);
    match evidence.mode {
        DiffMode::Working => format!("srcwalk review --scope {scope_arg}"),
        DiffMode::Staged => format!("srcwalk review --staged --scope {scope_arg}"),
        DiffMode::Range => format!(
            "srcwalk review {} --scope {scope_arg}",
            evidence.rev_range.as_deref().unwrap_or_default()
        ),
    }
}

fn scope_arg(scope: &Path, scope_glob: Option<&str>) -> String {
    let mut scope = format::display_path(scope);
    if let Some(glob) = scope_glob {
        if scope == "." {
            scope = glob.trim_start_matches('/').to_string();
        } else {
            scope.push_str(glob);
        }
    }
    scope
}

fn display_range(start: u32, end: u32) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

fn apply_review_budget(output: &str, budget_tokens: u64) -> String {
    if estimate_tokens(output.len() as u64) <= budget_tokens {
        return output.to_string();
    }

    let footer_start = output
        .find("\n\n## omitted")
        .or_else(|| output.find("\n\n## exits"))
        .or_else(|| output.find("\n> Next:"));
    let Some(footer_start) = footer_start else {
        return budget::apply_preserving_footer(output, budget_tokens);
    };

    let (body, footer) = output.split_at(footer_start);
    if body.trim().is_empty() || footer.trim().is_empty() {
        return budget::apply_preserving_footer(output, budget_tokens);
    }

    let budgeted_body = budget::apply(body.trim_end(), budget_tokens);
    format!("{}\n\n{}", budgeted_body.trim_end(), footer.trim_start())
}

fn append_token_footer(out: &mut String) {
    let tokens = estimate_tokens(out.len() as u64);
    let _ = write!(out, "\n\n(~{tokens} tokens)");
}

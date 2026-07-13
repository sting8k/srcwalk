use std::fmt::Write as _;
use std::path::Path;

use crate::evidence::direct_call::{ArgParamMapping, DirectCallEvidenceEdge, DirectCallUnknown};
use crate::search;

pub(crate) fn format_call_site(site: &search::callees::CallSite) -> String {
    let prefix = if site.is_return { "->ret " } else { "" };
    let call = format_call_with_args(site);
    match &site.return_var {
        Some(var) => format!("L{} {}{} = {}", site.line, prefix, var, call),
        None => format!("L{} {}{}", site.line, prefix, call),
    }
}

fn format_call_with_args(site: &search::callees::CallSite) -> String {
    if site.args.is_empty() {
        return site.call_text.clone();
    }

    let args = site
        .args
        .iter()
        .take(6)
        .enumerate()
        .map(|(idx, arg)| format!("arg{}={}", idx + 1, compact_arg(arg)))
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if site.args.len() > 6 { ", ..." } else { "" };
    let prefix = site.call_prefix.as_deref().unwrap_or(&site.callee);
    format!("{prefix}({args}{suffix})")
}

fn compact_arg(arg: &str) -> String {
    const LIMIT: usize = 120;
    const HEAD: usize = 72;
    const TAIL: usize = 40;

    let arg = arg.split_whitespace().collect::<Vec<_>>().join(" ");
    if arg.chars().count() <= LIMIT {
        return arg;
    }

    let head = arg.chars().take(HEAD).collect::<String>();
    let tail = arg
        .chars()
        .rev()
        .take(TAIL)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{head} … {tail}")
}

pub(crate) fn format_direct_call_edge(
    edge: &DirectCallEvidenceEdge,
    scope: &Path,
    indent: usize,
) -> String {
    let pad = " ".repeat(indent);
    let target = edge.target_anchor().display_relative_to(scope);
    let mut out = format!(
        "{pad}-> [fn] {} {target}\n{pad}   confidence: {}",
        edge.target_name(),
        edge.confidence().as_str()
    );
    for mapping in edge.arg_param_mappings() {
        let _ = write!(
            out,
            "\n{pad}   arg{} `{}` -> param{} `{}`",
            mapping.arg_index(),
            mapping.arg_display(),
            mapping.param_index(),
            mapping.param_name()
        );
    }
    if !edge.arg_param_mappings().is_empty() {
        let _ = write!(
            out,
            "\n{pad}   mapping confidence: {}",
            ArgParamMapping::confidence()
        );
    }
    if let Some(reason) = edge.mapping_unknown() {
        let _ = write!(
            out,
            "\n{pad}   arg→param mapping: unknown ({})",
            reason.as_str()
        );
    }
    if edge.omitted_arg_param_mappings() > 0 {
        let _ = write!(
            out,
            "\n{pad}   ... {} arg→param mappings omitted",
            edge.omitted_arg_param_mappings()
        );
    }
    out
}

pub(crate) fn format_direct_call_unknown(
    unknown: &DirectCallUnknown,
    scope: &Path,
    indent: usize,
) -> String {
    let pad = " ".repeat(indent);
    let mut out = format!(
        "{pad}-> direct target: unknown ({})",
        unknown.reason().as_str()
    );
    for candidate in unknown.candidates() {
        let _ = write!(
            out,
            "\n{pad}   candidate: {}",
            candidate.display_relative_to(scope)
        );
    }
    if unknown.omitted_candidates() > 0 {
        let _ = write!(
            out,
            "\n{pad}   ... {} candidates omitted",
            unknown.omitted_candidates()
        );
    }
    out
}

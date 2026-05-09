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

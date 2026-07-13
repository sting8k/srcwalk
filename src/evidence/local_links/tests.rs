use std::path::Path;

use super::*;
use crate::evidence::Anchor;
use crate::types::Lang;

fn subject(value: &str) -> LocalSubject {
    LocalSubject::new(value).unwrap()
}

fn link(kind: LocalLinkKind, from: &str, to: &str, line: u32) -> LocalLink {
    LocalLink::new(
        kind,
        subject(from),
        subject(to),
        Anchor::line(Path::new("src/lib.rs"), line),
        format!("{from}->{to}"),
    )
}

#[test]
fn graph_sorts_and_dedups_links_deterministically() {
    let graph = LocalLinkGraph::from_links(vec![
        link(LocalLinkKind::FieldRead, "b", "c", 3),
        link(LocalLinkKind::AssignmentAlias, "a", "b", 2),
        link(LocalLinkKind::AssignmentAlias, "a", "b", 2),
    ]);

    assert_eq!(graph.links().len(), 2);
    assert_eq!(graph.links()[0].from().identity(), "a");
    assert_eq!(graph.links()[1].from().identity(), "b");
}

#[test]
fn graph_dedups_semantic_edges_with_different_snippets() {
    let graph = LocalLinkGraph::from_links(vec![
        LocalLink::new(
            LocalLinkKind::CallResult,
            subject("GetToken(req)"),
            subject("token"),
            Anchor::line(Path::new("src/lib.rs"), 10),
            "let token = GetToken(req);",
        ),
        LocalLink::new(
            LocalLinkKind::CallResult,
            subject("GetToken(req)"),
            subject("token"),
            Anchor::line(Path::new("src/lib.rs"), 11),
            "GetToken(req)",
        ),
    ]);

    assert_eq!(graph.links().len(), 1);
    assert_eq!(
        graph
            .unique_chain("GetToken(req)", "token", 1)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn graph_dedups_multiline_call_result_edges_from_call_sites() {
    let rust = r#"
fn demo(req: Req, suffix: &str) {
    let path = build_path(
        req.path,
        suffix
    );
    OpenFile(path);
}
"#;
    let graph =
        collect_local_links_for_function(Path::new("src/lib.rs"), rust, Lang::Rust, "demo", 1, 7);
    let call_result_links: Vec<_> = graph
        .links()
        .iter()
        .filter(|link| link.kind() == LocalLinkKind::CallResult && link.to().identity() == "path")
        .collect();

    assert_eq!(call_result_links.len(), 1);
    assert_eq!(
        call_result_links[0].from().identity(),
        "build_path( req.path, suffix )"
    );
    assert_eq!(
        graph
            .unique_chain("build_path( req.path, suffix )", "path", 1)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn graph_finds_unique_chain_and_abstains_on_ambiguity() {
    let unique = LocalLinkGraph::from_links(vec![
        link(LocalLinkKind::AssignmentAlias, "path", "alias", 2),
        link(LocalLinkKind::AssignmentAlias, "req.path", "path", 1),
    ]);
    let chain = unique
        .unique_chain("req.path", "alias", DEFAULT_LOCAL_LINK_MAX_HOPS)
        .unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].kind(), LocalLinkKind::AssignmentAlias);

    let ambiguous = LocalLinkGraph::from_links(vec![
        link(LocalLinkKind::AssignmentAlias, "a", "b", 1),
        link(LocalLinkKind::AssignmentAlias, "b", "c", 2),
        link(LocalLinkKind::AssignmentAlias, "a", "c", 3),
    ]);
    assert!(ambiguous.unique_chain("a", "c", 2).is_none());
    let ambiguous_write = LocalLinkGraph::from_links(vec![
        link(LocalLinkKind::AssignmentAlias, "req.safe", "path", 1),
        link(LocalLinkKind::AssignmentAlias, "req.user", "path", 2),
        link(LocalLinkKind::AssignmentAlias, "path", "alias", 3),
    ]);
    assert!(ambiguous_write
        .unique_chain("req.safe", "path", 1)
        .is_none());
    assert!(ambiguous_write
        .unique_chain("req.safe", "alias", 2)
        .is_none());
    assert!(ambiguous_write.unique_chain("path", "alias", 1).is_none());
}

#[test]
fn graph_abstains_on_cycles_and_accepts_zero_hop_identity() {
    let graph = LocalLinkGraph::from_links(vec![
        link(LocalLinkKind::AssignmentAlias, "a", "b", 1),
        link(LocalLinkKind::AssignmentAlias, "b", "a", 2),
        link(LocalLinkKind::AssignmentAlias, "b", "c", 3),
    ]);

    assert_eq!(graph.unique_chain("a", "c", 3).unwrap().len(), 2);
    assert_eq!(graph.unique_chain("a", "a", 2).unwrap().len(), 0);
}

#[test]
fn graph_abstains_when_link_budget_is_exceeded() {
    let mut links = vec![link(LocalLinkKind::AssignmentAlias, "start", "end", 1)];
    for index in 0..MAX_LOCAL_LINKS {
        links.push(link(
            LocalLinkKind::AssignmentAlias,
            &format!("extra{index}"),
            &format!("other{index}"),
            index as u32 + 2,
        ));
    }
    let graph = LocalLinkGraph::from_links(links);

    assert!(graph.budget_exceeded());
    assert!(graph.unique_chain("start", "end", 1).is_none());
}

#[test]
fn extracts_rust_and_javascript_assignment_chains() {
    let rust = r#"
fn demo(req: Req) {
let path = req.path;
let alias = path;
ValidatePath(path);
OpenFile(alias);
}
"#;
    let rust_graph =
        collect_local_links_for_function(Path::new("src/lib.rs"), rust, Lang::Rust, "demo", 1, 7);
    assert!(rust_graph.unique_chain("req.path", "alias", 2).is_some());

    let js = r#"
function demo(req) {
  const path = req.path;
  const alias = path;
  ValidatePath(path);
  OpenFile(alias);
}
"#;
    let js_graph = collect_local_links_for_function(
        Path::new("src/lib.js"),
        js,
        Lang::JavaScript,
        "demo",
        1,
        6,
    );
    assert!(js_graph.unique_chain("req.path", "alias", 2).is_some());
}

#[test]
fn predecessor_chain_is_bounded_and_abstains_on_ambiguity() {
    let unique = LocalLinkGraph::from_links(vec![
        link(LocalLinkKind::AssignmentAlias, "req.path", "path", 1),
        link(LocalLinkKind::AssignmentAlias, "path", "alias", 2),
    ]);
    let chain = unique.unique_predecessor_chain("alias", 2).unwrap();
    assert_eq!(
        chain
            .iter()
            .map(|link| (link.from().identity(), link.to().identity()))
            .collect::<Vec<_>>(),
        vec![("req.path", "path"), ("path", "alias")]
    );

    let ambiguous = LocalLinkGraph::from_links(vec![
        link(LocalLinkKind::AssignmentAlias, "req.safe", "path", 1),
        link(LocalLinkKind::AssignmentAlias, "req.user", "path", 2),
        link(LocalLinkKind::AssignmentAlias, "path", "alias", 3),
    ]);
    assert!(ambiguous.unique_predecessor_chain("alias", 2).is_none());
}

#[test]
fn parenthesized_field_expression_is_not_a_call_result() {
    let rust = r#"
fn demo(req: Request) {
    let value = (req.value);
    sink(value);
}
"#;
    let graph =
        collect_local_links_for_function(Path::new("src/lib.rs"), rust, Lang::Rust, "demo", 1, 4);

    assert!(graph
        .links()
        .iter()
        .all(|link| link.kind() != LocalLinkKind::CallResult || link.to().identity() != "value"));
}

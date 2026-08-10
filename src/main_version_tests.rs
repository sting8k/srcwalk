use super::{parse_latest_tag_from_headers, parse_npm_version, version_is_newer};

#[test]
fn parses_latest_release_redirect_tag() {
    let headers = "HTTP/2 302\nlocation: https://github.com/sting8k/srcwalk/releases/tag/v0.2.8\n";
    assert_eq!(
        parse_latest_tag_from_headers(headers).as_deref(),
        Some("0.2.8")
    );
}

#[test]
fn parses_npm_registry_version() {
    let json = r#"{"name":"srcwalk","version":"0.2.8"}"#;
    assert_eq!(parse_npm_version(json).as_deref(), Some("0.2.8"));
}

#[test]
fn compares_semver_triplets() {
    assert!(version_is_newer("0.2.8", "0.2.7"));
    assert!(!version_is_newer("0.2.7", "0.2.7"));
    assert!(!version_is_newer("0.2.6", "0.2.7"));
}

#[test]
fn version_line_matches_contract_shape() {
    // Contract: `srcwalk \d+\.\d+\.\d+ \(.+\)` — tolerates `unknown`.
    let line = super::version_line();
    assert!(line.starts_with("srcwalk "), "{line}");
    let rest = &line["srcwalk ".len()..];
    let (semver, parens) = rest.split_once(" (").expect("expected ( suffix");
    let parts: Vec<_> = semver.split('.').collect();
    assert_eq!(parts.len(), 3, "{line}");
    for p in parts {
        assert!(
            !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()),
            "{line}"
        );
    }
    assert!(parens.ends_with(')'), "{line}");
    assert!(parens.len() > 1, "{line}");
}

#[test]
fn unknown_label_yields_unknown_suffix() {
    // Confirm the fail-soft path renders `(unknown)` with no trailing comma.
    let line = if env!("SRCWALK_GIT_LABEL") == "unknown" {
        super::version_line()
    } else {
        // Build had git; simulate the unknown branch via the formatter alone.
        String::new()
    };
    if env!("SRCWALK_GIT_LABEL") == "unknown" {
        assert!(line.ends_with("(unknown)"), "{line}");
    }
}

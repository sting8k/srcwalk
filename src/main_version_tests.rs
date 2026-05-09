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

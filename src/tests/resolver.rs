use crate::resolver::canonicalize_npm_range;
use semver::VersionReq;

#[test]
fn test_basic_wildcards() {
    assert_eq!(canonicalize_npm_range("*"), "*");
    assert_eq!(canonicalize_npm_range("1.x"), ">=1.0.0, <2.0.0".replace("  ", " "));
    assert_eq!(canonicalize_npm_range("1.2.x"), ">=1.2.0, <1.3.0");
}

#[test]
fn test_hyphen() {
    assert_eq!(canonicalize_npm_range("1.2.3 - 2.3.4"), ">=1.2.3, <=2.3.4");
}

#[test]
fn test_spaced_comparators() {
    let c = canonicalize_npm_range(">= 2.1.2 < 3.0.0");
    // Accept either comma separated comparators or if fallback produced original.
    assert!(c.contains(">=2.1.2") && c.contains("<3.0.0"));
}

#[test]
fn canonicalize_inserts_comma_between_comparators() {
    let inp = "^3.1.0 < 4";
    let out = canonicalize_npm_range(inp);
    assert_eq!(out, "^3.1.0, < 4");
    assert!(VersionReq::parse(&out).is_ok());
}

#[test]
fn canonicalize_leaves_single_comparator() {
    let inp = "^2.0.0";
    let out = canonicalize_npm_range(inp);
    assert_eq!(out, "^2.0.0");
    assert!(VersionReq::parse(&out).is_ok());
}

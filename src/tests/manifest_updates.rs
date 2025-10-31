use crate::cli::commands::install::manifest_updates::parse_spec;

#[test]
fn parses_scoped_with_range() {
    let (name, range) = parse_spec("@scope/pkg@^1.2.3");
    assert_eq!(name, "@scope/pkg");
    assert_eq!(range, "^1.2.3");
}

#[test]
fn parses_scoped_without_range() {
    let (name, range) = parse_spec("@scope/pkg");
    assert_eq!(name, "@scope/pkg");
    assert_eq!(range, "*");
}

#[test]
fn parses_unscoped_with_range() {
    let (name, range) = parse_spec("lodash@^4.17.0");
    assert_eq!(name, "lodash");
    assert_eq!(range, "^4.17.0");
}

#[test]
fn parses_unscoped_without_range() {
    let (name, range) = parse_spec("lodash");
    assert_eq!(name, "lodash");
    assert_eq!(range, "*");
}

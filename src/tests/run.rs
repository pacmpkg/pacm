use crate::cli::commands::run::{build_script_command, quote_arg_for_shell};

#[test]
fn quote_unix() {
    if cfg!(windows) {
        return;
    }
    assert_eq!(quote_arg_for_shell("abc"), "abc");
    assert_eq!(quote_arg_for_shell("a b"), "'a b'");
    assert_eq!(quote_arg_for_shell("it's"), "'it'\\''s'");
    let args = vec!["--watch".to_string()];
    assert_eq!(build_script_command("node build.js", &args), "node build.js '--watch'");
}

#[test]
fn quote_windows() {
    if !cfg!(windows) {
        return;
    }
    assert_eq!(quote_arg_for_shell("abc"), "abc");
    assert_eq!(quote_arg_for_shell("a b"), "\"a b\"");
    let args = vec!["--watch".to_string()];
    assert_eq!(build_script_command("node build.js", &args), "node build.js \"--watch\"");
}

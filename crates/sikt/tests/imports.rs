//! `use "file"`: rules from files, read through the builder's loader.

use sikt::Pattern;

fn files(path: &str) -> Result<String, String> {
    match path {
        "words.sikt" => Ok("let pair = 2 letter\nuse \"digits.sikt\"".into()),
        "digits.sikt" => Ok("let two = 2 digit".into()),
        "bad.sikt" => Ok("let x = digit\nfind x".into()),
        _ => Err("no such file".into()),
    }
}

fn texts(source: &str, input: &str) -> Vec<String> {
    let pattern = Pattern::builder().import_with(files).build(source).unwrap();
    pattern
        .find_iter(input)
        .map(|m| m.unwrap().as_str().to_owned())
        .collect()
}

#[test]
fn rules_come_from_files() {
    assert_eq!(
        texts(r#"use "words.sikt" find pair two"#, "ab12 c3 de45"),
        ["ab12", "de45"]
    );
}

#[test]
fn a_file_used_twice_is_read_once() {
    let source = r#"use "words.sikt" use "digits.sikt" find two"#;
    assert_eq!(texts(source, "12"), ["12"]);
}

#[test]
fn files_and_modules_together() {
    assert_eq!(
        texts(
            r#"use net use "digits.sikt" find ipv4 ":" two"#,
            "10.0.0.1:80"
        ),
        ["10.0.0.1:80"]
    );
}

#[test]
fn a_missing_file() {
    let error = Pattern::builder()
        .import_with(files)
        .build(r#"use "nope.sikt" find x"#)
        .unwrap_err();
    assert_eq!(error.message(), "can't read `nope.sikt`: no such file");
}

#[test]
fn files_hold_only_rules() {
    let error = Pattern::builder()
        .import_with(files)
        .build(r#"use "bad.sikt" find x"#)
        .unwrap_err();
    assert_eq!(error.message(), "a module can only have `let` and `use`");
}

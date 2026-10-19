//! Runs the `sikt` binary end to end.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

fn sikt() -> Command {
    Command::cargo_bin("sikt").unwrap()
}

/// A directory with a few text files, an ignored file and a binary file.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let write = |path: &str, text: &[u8]| {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write("a.txt", b"hello\nworld\n");
    write("b.txt", b"say hello there\n");
    write("sub/c.txt", b"nope\nhello again\n");
    write(".gitignore", b"ignored.txt\n");
    write("ignored.txt", b"hello\n");
    write("data.bin", b"hello\x00\x01\x02");
    // `ignore` only reads .gitignore inside a repository.
    fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

fn read(dir: &Path, path: &str) -> String {
    fs::read_to_string(dir.join(path)).unwrap()
}

#[test]
fn searches_stdin_line_by_line() {
    sikt()
        .arg(r#""TODO""#)
        .write_stdin("a TODO b\nnothing\nTODO 2\n")
        .assert()
        .success()
        .stdout("a TODO b\nTODO 2\n");
}

#[test]
fn no_match_exits_1() {
    sikt()
        .arg(r#""zzz""#)
        .write_stdin("abc\n")
        .assert()
        .code(1)
        .stdout("");
}

#[test]
fn compile_errors_exit_2_with_a_hint() {
    sikt()
        .arg("TODO")
        .write_stdin("TODO\n")
        .assert()
        .code(2)
        .stderr(contains("unknown word `TODO`").and(contains(r#"quote it: "TODO""#)));
}

#[test]
fn fixed_strings() {
    sikt()
        .args(["-F", "a.b*"])
        .write_stdin("xa.b*y\nab\n")
        .assert()
        .success()
        .stdout("xa.b*y\n");
}

#[test]
fn fixed_strings_with_quotes_and_backslashes() {
    sikt()
        .args(["-F", r#"say "hi" \o/"#])
        .write_stdin("they say \"hi\" \\o/\nno\n")
        .assert()
        .success()
        .stdout("they say \"hi\" \\o/\n");
}

#[test]
fn ignore_case() {
    sikt()
        .args(["-i", r#""error""#])
        .write_stdin("Error\nerror\nERR\n")
        .assert()
        .success()
        .stdout("Error\nerror\n");
}

#[test]
fn only_matching() {
    sikt()
        .args(["-o", "1+ digit"])
        .write_stdin("a1 b22\nc\n")
        .assert()
        .success()
        .stdout("1\n22\n");
}

#[test]
fn count_on_stdin() {
    sikt()
        .args(["-c", "digit"])
        .write_stdin("a1 b2\nc3\nd\n")
        .assert()
        .success()
        .stdout("3\n");
}

#[test]
fn print_extracts_like_awk() {
    let passwd = "root:x:0:0\nalice:x:1001:1001\nbob:x:1000:1000\n";
    sikt()
        .arg(r#"find user: until ":" ":" until ":" ":" uid: 1+ digit where uid > 1000 print "{user} {uid}""#)
        .write_stdin(passwd)
        .assert()
        .success()
        .stdout("alice 1001\n");
}

#[test]
fn replace_on_stdin_writes_the_text() {
    sikt()
        .arg(r#"find "colour" replace "color""#)
        .write_stdin("colour\nColour colours\nnone\n")
        .assert()
        .success()
        .stdout("color\nColour colors\nnone\n");
}

#[test]
fn replace_keeps_line_endings() {
    sikt()
        .arg(r#"find "a" replace "x""#)
        .write_stdin("a\r\nb\na")
        .assert()
        .success()
        .stdout("x\r\nb\nx");
}

#[test]
fn searches_a_tree_respecting_gitignore_and_skipping_binaries() {
    let dir = tree();
    sikt()
        .current_dir(dir.path())
        .args([r#""hello""#, "."])
        .assert()
        .success()
        .stdout("a.txt:1:hello\nb.txt:1:say hello there\nsub/c.txt:2:hello again\n");
}

#[test]
fn single_file_has_no_prefix() {
    let dir = tree();
    sikt()
        .current_dir(dir.path())
        .args([r#""hello""#, "a.txt"])
        .assert()
        .success()
        .stdout("hello\n");
}

#[test]
fn files_with_matches() {
    let dir = tree();
    sikt()
        .current_dir(dir.path())
        .args(["-l", r#""hello""#, "."])
        .assert()
        .success()
        .stdout("a.txt\nb.txt\nsub/c.txt\n");
}

#[test]
fn counts_per_file() {
    let dir = tree();
    sikt()
        .current_dir(dir.path())
        .args(["-c", r#""l""#, "."])
        .assert()
        .success()
        .stdout("a.txt:3\nb.txt:2\nsub/c.txt:2\n");
}

#[test]
fn replace_on_files_shows_a_diff_and_changes_nothing() {
    let dir = tree();
    sikt()
        .current_dir(dir.path())
        .args([r#"find "hello" replace "bye""#, "a.txt"])
        .assert()
        .success()
        .stdout(contains("--- a/a.txt\n+++ b/a.txt\n").and(contains("-hello\n+bye\n")));
    assert_eq!(read(dir.path(), "a.txt"), "hello\nworld\n");
}

#[test]
fn write_applies_replacements() {
    let dir = tree();
    sikt()
        .current_dir(dir.path())
        .args(["-w", r#"find "hello" replace "bye""#, "."])
        .assert()
        .success()
        .stdout("");
    assert_eq!(read(dir.path(), "a.txt"), "bye\nworld\n");
    assert_eq!(read(dir.path(), "sub/c.txt"), "nope\nbye again\n");
    assert_eq!(read(dir.path(), "ignored.txt"), "hello\n");
}

#[test]
fn json_records() {
    let output = sikt()
        .args(["--json", r#"k: 1+ letter "=" v: 1+ digit"#])
        .write_stdin("x\na=1 b=22\n")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let records: Vec<serde_json::Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        records,
        [
            serde_json::json!({"path": "<stdin>", "line": 2, "start": 0, "end": 3, "text": "a=1", "captures": {"k": "a", "v": "1"}}),
            serde_json::json!({"path": "<stdin>", "line": 2, "start": 4, "end": 8, "text": "b=22", "captures": {"k": "b", "v": "22"}}),
        ]
    );
}

#[test]
fn multiline_prints_the_lines_a_match_spans() {
    sikt()
        .args(["-U", r#""a" newline "b""#])
        .write_stdin("x\na\nb\ny\n")
        .assert()
        .success()
        .stdout("a\nb\n");
}

#[test]
fn warns_about_newline_in_line_mode() {
    sikt()
        .arg(r#""a" newline"#)
        .write_stdin("a\n")
        .assert()
        .code(1)
        .stderr(contains("never matches in line mode"));
}

#[test]
fn check_only_compiles() {
    sikt()
        .args(["--check", r#""a""#])
        .assert()
        .success()
        .stderr("");
    sikt()
        .args(["--check", r#""a" or "ab""#])
        .assert()
        .success()
        .stderr(contains("can never match"));
    sikt()
        .args(["--check", "digts"])
        .assert()
        .code(2)
        .stderr(contains("did you mean `digit`?"));
}

#[test]
fn explain_shows_the_closest_line() {
    sikt()
        .args(["--explain", r#""[" level: 1+ upper "] " digit"#])
        .write_stdin("plain\n2026-10-19 [WARN] x\n")
        .assert()
        .code(1)
        .stderr(contains("closest attempt on line 2").and(contains(
            r#"matched `level`, then expected digit but found "x""#,
        )));
}

#[test]
fn program_from_a_file() {
    let dir = tree();
    fs::write(
        dir.path().join("p.sikt"),
        "find \"hello\"\nreplace \"hi\"\n",
    )
    .unwrap();
    sikt()
        .current_dir(dir.path())
        .args(["-f", "p.sikt"])
        .write_stdin("hello you\n")
        .assert()
        .success()
        .stdout("hi you\n");
}

#[test]
fn missing_file_exits_2() {
    sikt()
        .args([r#""a""#, "does-not-exist.txt"])
        .assert()
        .code(2)
        .stderr(contains("does-not-exist.txt"));
}

#[test]
fn color_highlights_matches() {
    sikt()
        .args(["--color", "always", r#""b""#])
        .write_stdin("abc\n")
        .assert()
        .success()
        .stdout(contains("\x1b["));
}

/// Each output mode stays linear in the number of matching lines.
#[test]
fn many_matches_stay_fast() {
    let text = "a ERROR b\n".repeat(60_000);
    for args in [&[][..], &["-U"], &["--json"], &["-o"]] {
        sikt()
            .args(args)
            .arg(r#""ERROR""#)
            .write_stdin(text.clone())
            .timeout(std::time::Duration::from_secs(15))
            .assert()
            .success();
    }
}

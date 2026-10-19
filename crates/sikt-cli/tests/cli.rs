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
            serde_json::json!({"path": "<stdin>", "line": 2, "start": 2, "end": 5, "text": "a=1", "captures": {"k": "a", "v": "1"}}),
            serde_json::json!({"path": "<stdin>", "line": 2, "start": 6, "end": 10, "text": "b=22", "captures": {"k": "b", "v": "22"}}),
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

#[test]
fn multiline_match_over_several_lines() {
    sikt()
        .args(["-U", "-n", r#""b" newline "c" newline "d""#])
        .write_stdin("a\nb\nc\nd\ne\n")
        .assert()
        .success()
        .stdout("2:b\n3:c\n4:d\n");
}

#[test]
fn multiline_match_ending_in_a_line_break_shows_one_line() {
    sikt()
        .args(["-U", r#""b" newline"#])
        .write_stdin("a\nb\nc\n")
        .assert()
        .success()
        .stdout("b\n");
}

#[test]
fn line_numbers_skip_lines_without_matches() {
    sikt()
        .args(["-n", r#""x""#])
        .write_stdin("a\nx\nb\nc\nxx\n")
        .assert()
        .success()
        .stdout("2:x\n5:xx\n");
}

#[test]
fn searches_bytes_that_are_not_utf8() {
    sikt()
        .arg(r#""TODO""#)
        .write_stdin(&b"ok \xff TODO\nnope\n"[..])
        .assert()
        .success()
        .stdout(&b"ok \xff TODO\n"[..]);
}

#[test]
fn json_replaces_invalid_utf8() {
    sikt()
        .args(["--json", r#"w: (any "x")"#])
        .write_stdin(&b"\xffx\n"[..])
        .assert()
        .success()
        .stdout(contains("\"text\":\"\u{fffd}x\""));
}

#[test]
fn replace_skips_files_that_are_not_utf8() {
    let dir = tree();
    fs::write(dir.path().join("latin1.txt"), b"caf\xe9 hello\n").unwrap();
    sikt()
        .current_dir(dir.path())
        .args(["-w", r#"find "hello" replace "bye""#, "latin1.txt"])
        .assert()
        .code(1)
        .stderr(contains("skipping latin1.txt: not valid UTF-8"));
    assert_eq!(
        fs::read(dir.path().join("latin1.txt")).unwrap(),
        b"caf\xe9 hello\n"
    );
}

#[test]
fn json_has_typed_values() {
    let output = sikt()
        .args([
            "--json",
            r#"user: 1+ letter 1+ space pid: integer 1+ space cpu: number"#,
        ])
        .write_stdin("root 1 0.5\n")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let record: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        record["captures"],
        serde_json::json!({"user": "root", "pid": 1, "cpu": 0.5})
    );
}

#[test]
fn rows_are_just_the_captures() {
    let pattern = r#"line start optional (user: 1+ letter 1+ space) pid: integer 1+ space cmd: until text end"#;
    let output = sikt()
        .args(["--rows", pattern])
        .write_stdin("PID CMD\nroot 1 init\n42 bash -l\n")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "{\"user\":\"root\",\"pid\":1,\"cmd\":\"init\"}\n{\"pid\":42,\"cmd\":\"bash -l\"}\n"
    );
}

const PS: &str =
    "    PID TTY      %CPU CMD\n      1 ?         0.5 init\n     22 pts/1     9.0 bash -l\n";

#[test]
fn table_rows() {
    sikt()
        .args(["--rows", "table types pid integer cpu number"])
        .write_stdin(PS)
        .assert()
        .success()
        .stdout(concat!(
            "{\"pid\":1,\"tty\":\"?\",\"cpu\":0.5,\"cmd\":\"init\"}\n",
            "{\"pid\":22,\"tty\":\"pts/1\",\"cpu\":9.0,\"cmd\":\"bash -l\"}\n",
        ));
}

#[test]
fn table_where_and_print() {
    sikt()
        .arg(r#"table types cpu number where cpu > 5 print "{pid} {cmd}""#)
        .write_stdin(PS)
        .assert()
        .success()
        .stdout("22 bash -l\n");
}

#[test]
fn table_lines_keep_their_numbers() {
    sikt()
        .args(["-n", "table"])
        .write_stdin(PS)
        .assert()
        .success()
        .stdout(concat!(
            "2:      1 ?         0.5 init\n",
            "3:     22 pts/1     9.0 bash -l\n",
        ));
}

#[test]
fn table_reads_a_repeated_header_again() {
    sikt()
        .args(["--rows", "table"])
        .write_stdin("A B\n1 2\n\nA B\n3 4\nC\n5\n")
        .assert()
        .success()
        .stdout(
            "{\"a\":\"1\",\"b\":\"2\"}\n{\"a\":\"3\",\"b\":\"4\"}\n{\"a\":\"C\"}\n{\"a\":\"5\"}\n",
        );
}

#[test]
fn each_file_has_its_own_header() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "PID CMD\n1 init\n").unwrap();
    fs::write(dir.path().join("b.txt"), "CMD PID\nsh 2\n").unwrap();
    sikt()
        .current_dir(dir.path())
        .args(["--rows", "table types pid integer", "a.txt", "b.txt"])
        .assert()
        .success()
        .stdout("{\"pid\":1,\"cmd\":\"init\"}\n{\"cmd\":\"sh\",\"pid\":2}\n");
}

#[test]
fn table_with_a_missing_column_fails() {
    sikt()
        .arg("table where cpu > 5")
        .write_stdin("PID CMD\n1 init\n")
        .assert()
        .code(2)
        .stderr(contains("unknown capture `cpu`"));
}

#[test]
fn table_sizes_and_percentages() {
    sikt()
        .args([
            "--rows",
            "table columns (fs size use mount) types size size use percent where size > 1G",
        ])
        .write_stdin("Filesystem Size Use% Mounted on\ntmpfs 16G 48% /\nrun 7.3M 1% /run\n")
        .assert()
        .success()
        .stdout("{\"fs\":\"tmpfs\",\"size\":17179869184,\"use\":48.0,\"mount\":\"/\"}\n");
}

#[test]
fn use_reads_files_next_to_the_program() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("lib")).unwrap();
    fs::write(
        dir.path().join("lib/log.sikt"),
        "let level = \"[\" 1+ upper \"]\"\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("lib/find.sikt"),
        "use \"log.sikt\"\nfind level\n",
    )
    .unwrap();
    sikt()
        .args(["-o", "-f", "lib/find.sikt"])
        .current_dir(dir.path())
        .write_stdin("a [WARN] b\n")
        .assert()
        .success()
        .stdout("[WARN]\n");
}

#[test]
fn use_reads_files_from_the_working_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("log.sikt"),
        "let level = \"[\" 1+ upper \"]\"\n",
    )
    .unwrap();
    sikt()
        .args(["-o", r#"use "log.sikt" find level"#])
        .current_dir(dir.path())
        .write_stdin("a [WARN] b\n")
        .assert()
        .success()
        .stdout("[WARN]\n");
}

#[test]
fn table_header_pattern() {
    sikt()
        .args(["--rows", r#"table header ("PID") types pid integer"#])
        .write_stdin("booting\n  PID CMD\n  1 init\n")
        .assert()
        .success()
        .stdout("{\"pid\":1,\"cmd\":\"init\"}\n");
}

#[test]
fn table_replace_keeps_the_header() {
    sikt()
        .arg(r#"table replace "{cmd}:{pid}""#)
        .write_stdin("PID CMD\n1 init\n2 sh\n")
        .assert()
        .success()
        .stdout("PID CMD\ninit:1\nsh:2\n");
}

#[test]
fn table_explain() {
    sikt()
        .args(["--explain", "table types a integer where a > 5"])
        .write_stdin("A B\n1 2\n")
        .assert()
        .code(1)
        .stderr(contains(
            "the pattern matches, but `where` rejected every match",
        ));
}

#[test]
fn plain_output_keeps_lines_intact() {
    sikt()
        .arg(r#""x""#)
        .write_stdin("x1\nx2\nno\nx3\r\nx4\nx5")
        .assert()
        .success()
        .stdout("x1\nx2\nx3\nx4\nx5\n");
}

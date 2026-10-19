//! Table programs: the header names the columns.

use sikt::{Program, Value};

/// Rows of `text` under its first line, as `(column, value)` lists.
fn rows(source: &str, text: &str) -> Vec<Vec<(String, String)>> {
    let program = Program::new(source).unwrap();
    let table = program.table().expect("a table program");
    let (header, body) = text.split_once('\n').unwrap();
    let rows = table.for_header(header).unwrap();
    rows.find_iter(body)
        .map(|m| {
            let m = m.unwrap();
            rows.pattern()
                .capture_names()
                .filter_map(|n| {
                    let value = match m.value(n)? {
                        Value::Text(t) => format!("{t:?}"),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(x) => format!("{x:?}"),
                    };
                    Some((n.to_owned(), value))
                })
                .collect()
        })
        .collect()
}

fn row(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|&(n, v)| (n.to_owned(), v.to_owned()))
        .collect()
}

#[test]
fn columns_come_from_the_header() {
    let ps = "    PID TTY          TIME CMD\n   3622 ?        00:00:14 sync thing\n";
    assert_eq!(
        rows("table types pid integer", ps),
        [row(&[
            ("pid", "3622"),
            ("tty", "\"?\""),
            ("time", "\"00:00:14\""),
            ("cmd", "\"sync thing\"")
        ])]
    );
}

#[test]
fn header_names_become_capture_names() {
    let text = "%CPU USE% Mounted MATCH  \n1 2 3 4\n";
    let names: Vec<_> = rows("table", text)[0]
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    assert_eq!(names, ["cpu", "use", "mounted", "match_"]);
}

#[test]
fn duplicate_and_empty_names() {
    let text = "A A %\n1 2 3\n";
    let names: Vec<_> = rows("table", text)[0]
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    assert_eq!(names, ["a", "a_2", "column3"]);
}

#[test]
fn split_by_a_pattern() {
    let docker = "CONTAINER ID   STATUS        NAMES\nabc123         Up 3 hours    web\n";
    assert_eq!(
        rows("table split by (2+ space)", docker),
        [row(&[
            ("container_id", "\"abc123\""),
            ("status", "\"Up 3 hours\""),
            ("names", "\"web\"")
        ])]
    );
}

#[test]
fn columns_override_the_header() {
    let df = "Filesystem Size Mounted on\n/dev/sda1 20G /home/me\n";
    assert_eq!(
        rows("table columns (fs size mount)", df),
        [row(&[
            ("fs", "\"/dev/sda1\""),
            ("size", "\"20G\""),
            ("mount", "\"/home/me\"")
        ])]
    );
}

#[test]
fn short_rows_keep_what_they_have() {
    let text = "A B C\n1 2\n";
    assert_eq!(
        rows("table", text),
        [row(&[("a", "\"1\""), ("b", "\"2\"")])]
    );
}

#[test]
fn values_that_dont_parse_stay_text() {
    let text = "PID\n12\nabc\n";
    assert_eq!(
        rows("table types pid integer", text),
        [row(&[("pid", "12")]), row(&[("pid", "\"abc\"")])]
    );
}

#[test]
fn with_splits_a_column() {
    let text = "PID TIME CMD\n1 12:05 /bin/sh -c x\n";
    let source = r#"table
        with time: m: integer ":" s: integer
        with cmd: exe: until (space or text end)"#;
    let found = rows(source, text);
    assert_eq!(
        found,
        [row(&[
            ("pid", "\"1\""),
            ("time", "\"12:05\""),
            ("m", "12"),
            ("s", "5"),
            ("cmd", "\"/bin/sh -c x\""),
            ("exe", "\"/bin/sh\""),
        ])]
    );
}

#[test]
fn where_and_print_use_columns() {
    let program = Program::new(r#"table types cpu number where cpu > 5 print "{pid}""#).unwrap();
    let rows = program.table().unwrap().for_header("PID %CPU").unwrap();
    let printed: Vec<_> = rows
        .find_iter("1 0.5\n2 9.0\n")
        .map(|m| rows.render(&m.unwrap()).unwrap())
        .collect();
    assert_eq!(printed, ["2"]);
}

#[test]
fn a_table_program_matches_nothing_without_a_header() {
    let program = Program::new("table").unwrap();
    assert_eq!(program.find_iter("PID\n1\n").count(), 0);
}

#[test]
fn unknown_column_in_where_is_an_error_for_that_header() {
    let program = Program::new("table where cpu > 5").unwrap();
    let error = program.table().unwrap().for_header("PID TTY").unwrap_err();
    assert_eq!(error.message(), "unknown capture `cpu`");
}

#[test]
fn unknown_column_in_types() {
    let program = Program::new("table types cpu number").unwrap();
    let error = program.table().unwrap().for_header("PID TTY").unwrap_err();
    assert_eq!(error.message(), "this header has no column `cpu`");
}

#[test]
fn table_and_find_dont_mix() {
    let error = Program::new(r#"table find "x""#).unwrap_err();
    assert_eq!(error.message(), "a table program has no `find`");
}

/// `(header, rows, first row line)` for each section of `text`.
fn sections(source: &str, text: &str) -> Vec<(String, String, usize)> {
    let program = Program::new(source).unwrap();
    let table = program.table().unwrap();
    table
        .sections(text.as_bytes())
        .unwrap()
        .into_iter()
        .map(|s| {
            (
                text[s.header].to_owned(),
                text[s.rows].to_owned(),
                s.rows_line,
            )
        })
        .collect()
}

#[test]
fn a_repeated_header_starts_a_section() {
    assert_eq!(
        sections("table", "\nA B\n1 2\n\nA B\n3 4\n"),
        [
            ("A B".into(), "1 2\n\n".into(), 3),
            ("A B".into(), "3 4\n".into(), 6)
        ]
    );
}

#[test]
fn a_header_pattern_finds_headers_anywhere() {
    let text = "booting\n  PID CMD\n  1 init\nlater\n  PID CMD\n  7 sh\n";
    assert_eq!(
        sections(r#"table header ("PID")"#, text),
        [
            ("  PID CMD".into(), "  1 init\nlater\n".into(), 3),
            ("  PID CMD".into(), "  7 sh\n".into(), 6)
        ]
    );
}

#[test]
fn replace_in_a_table() {
    let program = Program::new(r#"table replace "{cmd}:{pid}""#).unwrap();
    let rows = program.table().unwrap().for_header("PID CMD").unwrap();
    assert_eq!(
        rows.replace_all("1 init\n2 sh -c x\n").unwrap(),
        "init:1\nsh -c x:2\n"
    );
}

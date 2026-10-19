//! Searching bytes that may not be valid UTF-8.

use sikt::{Pattern, Program};

#[test]
fn finds_in_bytes_with_captures() {
    let p = Pattern::new(r#"key: 1+ letter "=" value: 1+ digit"#).unwrap();
    let hay = b"\xff a=1 \xfe bb=22";
    let found: Vec<_> = p.find_iter_bytes(hay).map(|m| m.unwrap()).collect();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].as_bytes(), b"a=1");
    assert_eq!(found[1].get("key"), Some(&b"bb"[..]));
    assert_eq!(found[1].get("value"), Some(&b"22"[..]));
    assert_eq!(found[1].range(), 8..13);
}

#[test]
fn invalid_bytes_match_only_any() {
    let any = Pattern::new(r#""<" any ">""#).unwrap();
    let hay = b"<\xff> <a>";
    let texts: Vec<_> = any
        .find_iter_bytes(hay)
        .map(|m| m.unwrap().as_bytes().to_vec())
        .collect();
    assert_eq!(texts, [b"<\xff>".to_vec(), b"<a>".to_vec()]);

    let set = Pattern::new(r#""<" none of "x" ">""#).unwrap();
    let texts: Vec<_> = set
        .find_iter_bytes(hay)
        .map(|m| m.unwrap().as_bytes().to_vec())
        .collect();
    assert_eq!(texts, [b"<a>".to_vec()]);
}

#[test]
fn line_mode_on_bytes() {
    let p = Pattern::builder()
        .line_mode(true)
        .build(r#"text start 1+ digit"#)
        .unwrap();
    let hay = b"12\n\xff\n34\r\n";
    let ranges: Vec<_> = p.find_iter_bytes(hay).map(|m| m.unwrap().range()).collect();
    assert_eq!(ranges, [0..2, 5..7]);
}

#[test]
fn programs_filter_bytes() {
    let program = Program::new(
        r#"find "[" lvl: 1+ letter "] " n: 1+ digit where lvl in ("WARN" "ERROR") and n > 5"#,
    )
    .unwrap();
    let hay = b"[INFO] 9 \xff [WARN] 3 [ERROR] 7";
    let found: Vec<_> = program
        .find_iter_bytes(hay)
        .map(|m| m.unwrap().as_bytes().to_vec())
        .collect();
    assert_eq!(found, [b"[ERROR] 7".to_vec()]);
}

#[test]
fn case_insensitive_programs_on_bytes() {
    let program = Pattern::builder()
        .case_insensitive(true)
        .build_program(r#"find w: 1+ letter where w is "warn""#)
        .unwrap();
    let found: Vec<_> = program
        .find_iter_bytes(b"WARN \xff info")
        .map(|m| m.unwrap().as_bytes().to_vec())
        .collect();
    assert_eq!(found, [b"WARN".to_vec()]);
}

#[test]
fn patterns_and_programs_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Pattern>();
    check::<Program>();
    check::<sikt::Template>();
}

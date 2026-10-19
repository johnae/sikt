//! Snapshots of rendered compile errors. Message quality is a feature, so
//! changes to these should be deliberate.

use miette::{GraphicalReportHandler, GraphicalTheme};
use sikt::Pattern;

fn render(src: &str) -> String {
    let error = Pattern::new(src).expect_err("pattern should fail");
    let mut out = String::new();
    GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor())
        .render_report(&mut out, &error)
        .unwrap();
    out
}

#[test]
fn unknown_word() {
    insta::assert_snapshot!(render(r#"year: 4 digts "-" 2 digit"#));
}

#[test]
fn unterminated_string() {
    insta::assert_snapshot!(render(r#""[" until "]"#));
}

#[test]
fn backwards_range() {
    insta::assert_snapshot!(render("5..2 digit"));
}

#[test]
fn regex_habit() {
    insta::assert_snapshot!(render(r#"digit* "x""#));
}

#[test]
fn loops_forever() {
    insta::assert_snapshot!(render(r#"1+ (optional "-" 0+ digit)"#));
}

#[test]
fn multiline_pattern() {
    insta::assert_snapshot!(render("year: 4 digit\n\"-\"\nmonth: 2 dgit"));
}

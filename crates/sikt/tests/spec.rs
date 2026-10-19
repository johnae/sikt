//! Runs the data-driven cases in `tests/spec/*.toml`.
//!
//! A case has a `pattern` and either an `input` with the expected `matches`,
//! or the expected compile `error`. A match is its text, or a table with
//! `text`, `at` (byte offset), `captures` and `absent` (captures that didn't
//! participate). A capture given as a list checks every occurrence.
//!
//! With `replace`, the case checks the `output` of replacing every match with
//! that template, or the expected `template_error`. `warnings` lists the
//! expected warning messages.
//!
//! With `program = true`, the pattern is a program: the case checks `matches`
//! (after `where`), the `output` of `replace`, or the `printed` lines.

use std::collections::BTreeMap;
use std::path::Path;

use libtest_mimic::{Arguments, Failed, Trial};
use serde::Deserialize;
use sikt::{Pattern, Program, Template};

#[derive(Deserialize)]
struct File {
    case: Vec<Case>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    pattern: String,
    #[serde(default)]
    input: String,
    matches: Option<Vec<Expected>>,
    error: Option<String>,
    label: Option<String>,
    max_steps: Option<u64>,
    max_depth: Option<u32>,
    warnings: Option<Vec<String>>,
    replace: Option<String>,
    output: Option<String>,
    template_error: Option<String>,
    #[serde(default)]
    program: bool,
    printed: Option<Vec<String>>,
    #[serde(default)]
    case_insensitive: bool,
    match_error: Option<String>,
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum Expected {
    Text(String),
    Full {
        text: String,
        at: Option<usize>,
        #[serde(default)]
        captures: BTreeMap<String, Capture>,
        #[serde(default)]
        absent: Vec<String>,
    },
}

#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
enum Capture {
    Last(String),
    All(Vec<String>),
}

fn main() {
    let args = Arguments::from_args();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/spec");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();

    let mut trials = Vec::new();
    for path in paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
    {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).unwrap();
        let file: File =
            toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for case in file.case {
            trials.push(Trial::test(format!("{stem}::{}", case.name), move || {
                run(&case)
            }));
        }
    }
    libtest_mimic::run(&args, trials).exit();
}

fn run(case: &Case) -> Result<(), Failed> {
    let mut builder = Pattern::builder();
    if let Some(steps) = case.max_steps {
        builder = builder.max_steps(steps);
    }
    if let Some(depth) = case.max_depth {
        builder = builder.max_depth(depth);
    }
    builder = builder.case_insensitive(case.case_insensitive);
    if case.program {
        return run_program(case, builder.build_program(&case.pattern));
    }
    let compiled = builder.build(&case.pattern);

    if let Some(expected) = &case.error {
        let error = match compiled {
            Ok(_) => {
                return Err(
                    format!("expected error {expected:?}, but the pattern compiled").into(),
                );
            }
            Err(e) => e,
        };
        check("error", error.message(), expected)?;
        if let Some(label) = &case.label {
            check("label", error.label().unwrap_or(""), label)?;
        }
        return Ok(());
    }

    let pattern = compiled.map_err(|e| format!("{:?}", miette::Report::new(e)))?;
    if let Some(expected) = &case.warnings {
        let found: Vec<_> = pattern.warnings().iter().map(|w| w.message()).collect();
        if found != *expected {
            return Err(format!(
                "warnings differ\n  expected: {expected:?}\n     found: {found:?}"
            )
            .into());
        }
    }
    if let Some(source) = &case.replace {
        let template = Template::new(source, &pattern);
        if let Some(expected) = &case.template_error {
            let error = template.err().ok_or("expected a template error")?;
            check("template error", error.message(), expected)?;
            if let Some(label) = &case.label {
                check("label", error.label().unwrap_or(""), label)?;
            }
            return Ok(());
        }
        let template = template.map_err(|e| format!("{:?}", miette::Report::new(e)))?;
        let output = pattern
            .replace_all(&case.input, &template)
            .map_err(|e| e.to_string())?;
        let expected = case
            .output
            .as_deref()
            .ok_or("case with `replace` needs `output`")?;
        return check("output", &output, expected);
    }

    let mut found = Vec::new();
    for result in pattern.find_iter(&case.input) {
        match result {
            Ok(m) => found.push(m),
            Err(e) => {
                let Some(expected) = &case.match_error else {
                    return Err(format!("unexpected match error: {e}").into());
                };
                return check("match error", &e.to_string(), expected);
            }
        }
    }
    if case.match_error.is_some() {
        return Err("expected a match error".into());
    }

    let expected = case
        .matches
        .as_ref()
        .ok_or("case needs `matches` or `error`")?;
    let texts: Vec<_> = found.iter().map(|m| m.as_str()).collect();
    let expected_texts: Vec<_> = expected
        .iter()
        .map(|e| match e {
            Expected::Text(t) | Expected::Full { text: t, .. } => t.as_str(),
        })
        .collect();
    if texts != expected_texts {
        return Err(format!(
            "matches differ\n  expected: {expected_texts:?}\n     found: {texts:?}"
        )
        .into());
    }

    for (m, e) in found.iter().zip(expected) {
        let Expected::Full {
            at,
            captures,
            absent,
            ..
        } = e
        else {
            continue;
        };
        if let Some(at) = at
            && m.start() != *at
        {
            return Err(format!("{:?} at {}, expected at {at}", m.as_str(), m.start()).into());
        }
        for (name, capture) in captures {
            let (all, last) = match capture {
                Capture::Last(s) => (None, Some(s.as_str())),
                Capture::All(v) => (Some(v), v.last().map(String::as_str)),
            };
            if m.get(name) != last {
                return Err(format!(
                    "capture `{name}`: expected {last:?}, found {:?}",
                    m.get(name)
                )
                .into());
            }
            if let Some(all) = all {
                let found: Vec<_> = m.all(name).collect();
                if &found != all {
                    return Err(format!(
                        "capture `{name}` occurrences: expected {all:?}, found {found:?}"
                    )
                    .into());
                }
            }
        }
        for name in absent {
            if let Some(found) = m.get(name) {
                return Err(format!("capture `{name}` should be absent, found {found:?}").into());
            }
        }
    }
    Ok(())
}

fn run_program(case: &Case, compiled: Result<Program, sikt::Error>) -> Result<(), Failed> {
    let program = match (compiled, &case.error) {
        (Err(error), Some(expected)) => {
            check("error", error.message(), expected)?;
            if let Some(label) = &case.label {
                check("label", error.label().unwrap_or(""), label)?;
            }
            return Ok(());
        }
        (Ok(_), Some(expected)) => {
            return Err(format!("expected error {expected:?}, but it compiled").into());
        }
        (Err(error), None) => return Err(format!("{:?}", miette::Report::new(error)).into()),
        (Ok(program), None) => program,
    };
    let found = program
        .find_iter(&case.input)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if let Some(expected) = &case.output {
        let output = program
            .replace_all(&case.input)
            .map_err(|e| e.to_string())?;
        check("output", &output, expected)?;
    }
    if let Some(expected) = &case.printed {
        let printed: Vec<String> = found.iter().filter_map(|m| program.render(m)).collect();
        if printed != *expected {
            return Err(format!(
                "printed lines differ\n  expected: {expected:?}\n     found: {printed:?}"
            )
            .into());
        }
    }
    if let Some(expected) = &case.matches {
        let texts: Vec<_> = found.iter().map(|m| m.as_str()).collect();
        let expected: Vec<_> = expected
            .iter()
            .map(|e| match e {
                Expected::Text(t) | Expected::Full { text: t, .. } => t.as_str(),
            })
            .collect();
        if texts != expected {
            return Err(
                format!("matches differ\n  expected: {expected:?}\n     found: {texts:?}").into(),
            );
        }
    }
    Ok(())
}

fn check(what: &str, found: &str, expected: &str) -> Result<(), Failed> {
    if found == expected {
        Ok(())
    } else {
        Err(format!("{what} differs\n  expected: {expected}\n     found: {found}").into())
    }
}

//! Runs the examples in the README and the language guide. An example is a
//! `$ ` command in an `sh` block that reads its input from `echo` or `printf`,
//! followed by the output it prints.

use std::path::Path;
use std::process::Command;

/// Whether a command is complete, or continues on the next line inside an open
/// quote or after a trailing pipe.
fn is_complete(command: &str) -> bool {
    command.matches('\'').count().is_multiple_of(2) && !command.trim_end().ends_with('|')
}

/// `(command, expected output)` pairs from the `sh` blocks of a Markdown file.
fn examples(markdown: &str) -> Vec<(String, String)> {
    let mut examples = Vec::new();
    let mut in_block = false;
    let mut command: Option<String> = None;
    let mut output = String::new();
    let mut finish = |command: &mut Option<String>, output: &mut String| {
        let output = std::mem::take(output);
        if let Some(c) = command.take() {
            examples.push((c, output));
        }
    };
    for line in markdown.lines() {
        if line.starts_with("```") {
            finish(&mut command, &mut output);
            in_block = line == "```sh";
            continue;
        }
        if !in_block {
            continue;
        }
        if let Some(start) = line.strip_prefix("$ ") {
            finish(&mut command, &mut output);
            command = Some(start.to_owned());
        } else if let Some(c) = &mut command {
            if is_complete(c) {
                output.push_str(line);
                output.push('\n');
            } else {
                c.push('\n');
                c.push_str(line);
            }
        }
    }
    examples
}

#[test]
fn examples_print_what_the_docs_say() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bin = Path::new(env!("CARGO_BIN_EXE_sikt")).parent().unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut ran = 0;
    for file in ["README.md", "docs/language.md"] {
        let markdown = std::fs::read_to_string(root.join(file)).unwrap();
        for (command, expected) in examples(&markdown) {
            if !(command.starts_with("echo ") || command.starts_with("printf ")) {
                continue;
            }
            let out = Command::new("sh")
                .arg("-c")
                .arg(&command)
                .env("PATH", &path)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert_eq!(
                stdout,
                expected,
                "{file}: `{command}`\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            ran += 1;
        }
    }
    assert!(ran >= 15, "only {ran} examples ran");
}

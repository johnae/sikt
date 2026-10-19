mod output;

use std::fs;
use std::io::{self, BufWriter, IsTerminal, Read, StdoutLock, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme};
use sikt::{Action, Match, Program};

use crate::output::{Line, Style, lines, quote};

/// Search and replace text with readable patterns.
///
/// PROGRAM is a pattern like '"TODO"' or a program like
/// 'find "colour" replace "color"'. Patterns run on each line unless -U is given.
#[derive(Parser, Debug)]
#[command(name = "sikt", version)]
struct Args {
    /// The pattern or program; with -f, the first path.
    #[arg(required_unless_present = "file")]
    program: Option<String>,
    /// Files and directories to search. Without any, reads stdin, or searches
    /// the current directory when stdin is a terminal.
    paths: Vec<PathBuf>,
    /// Read the program from a file.
    #[arg(short = 'f', long = "file", value_name = "FILE")]
    file: Option<PathBuf>,
    /// Take PROGRAM as a literal string to find.
    #[arg(short = 'F', long)]
    fixed: bool,
    /// Match across lines instead of within each line.
    #[arg(short = 'U', long)]
    multiline: bool,
    /// Write replacements to the files instead of showing a diff.
    #[arg(short = 'w', long)]
    write: bool,
    /// Print only the matched text.
    #[arg(short = 'o', long)]
    only_matching: bool,
    /// Print the number of matches per file.
    #[arg(short = 'c', long)]
    count: bool,
    /// Print only the names of files with matches.
    #[arg(short = 'l', long = "files")]
    files_with_matches: bool,
    /// Match the whole program case-insensitively, `where` included.
    #[arg(short = 'i', long)]
    ignore_case: bool,
    /// Show line numbers.
    #[arg(short = 'n', long)]
    line_number: bool,
    /// Print one JSON record per match, with captures, file, line and span.
    #[arg(long)]
    json: bool,
    /// When nothing matches, show the closest attempt.
    #[arg(long)]
    explain: bool,
    /// Compile and lint the program, then stop.
    #[arg(long)]
    check: bool,
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto, value_name = "WHEN")]
    color: ColorChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ColorChoice {
    Auto,
    Always,
    Never,
}

impl ColorChoice {
    fn enabled(self, terminal: bool) -> bool {
        match self {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => terminal && std::env::var_os("NO_COLOR").is_none(),
        }
    }
}

fn main() -> ExitCode {
    let mut args = Args::parse();
    if args.file.is_some() {
        // With -f, the first positional argument is a path.
        if let Some(path) = args.program.take() {
            args.paths.insert(0, PathBuf::from(path));
        }
    }
    match run(&args) {
        Ok(status) => status,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sikt: {e}");
            ExitCode::from(2)
        }
    }
}

fn report(diagnostic: &dyn Diagnostic, color: bool) {
    let theme = if color {
        GraphicalTheme::unicode()
    } else {
        GraphicalTheme::unicode_nocolor()
    };
    let mut out = String::new();
    let _ = GraphicalReportHandler::new_themed(theme).render_report(&mut out, diagnostic);
    eprint!("{out}");
}

fn run(args: &Args) -> io::Result<ExitCode> {
    let stderr_color = args.color.enabled(io::stderr().is_terminal());
    let source = match (&args.file, &args.program) {
        (Some(file), _) => fs::read_to_string(file)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", file.display())))?,
        (None, Some(program)) if args.fixed => quote(program),
        (None, Some(program)) => program.clone(),
        (None, None) => unreachable!("clap requires a program"),
    };
    let program = match sikt::Pattern::builder()
        .case_insensitive(args.ignore_case)
        .line_mode(!args.multiline)
        .build_program(&source)
    {
        Ok(program) => program,
        Err(e) => {
            report(&e, stderr_color);
            return Ok(ExitCode::from(2));
        }
    };
    for warning in program.warnings() {
        report(warning, stderr_color);
    }
    if args.check {
        return Ok(ExitCode::SUCCESS);
    }

    let paths = if args.paths.is_empty() && io::stdin().is_terminal() {
        vec![PathBuf::from(".")]
    } else {
        args.paths.clone()
    };
    let mut search = Search {
        args,
        program: &program,
        style: Style {
            color: args.color.enabled(io::stdout().is_terminal()),
        },
        prefix: paths.len() > 1 || paths.iter().any(|p| p.is_dir()),
        out: BufWriter::new(io::stdout().lock()),
        found: false,
        failed: false,
    };
    if paths.is_empty() {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        search.text("<stdin>", None, &text)?;
    }
    for path in &paths {
        if path.is_dir() {
            let walk = ignore::WalkBuilder::new(path)
                .sort_by_file_name(|a, b| a.cmp(b))
                .build();
            for entry in walk {
                match entry {
                    Ok(entry) if entry.file_type().is_some_and(|t| t.is_file()) => {
                        search.file(entry.path(), true)?
                    }
                    Ok(_) => {}
                    Err(e) => search.fail(&e.to_string()),
                }
            }
        } else {
            search.file(path, false)?;
        }
    }
    search.out.flush()?;
    Ok(match (search.failed, search.found) {
        (true, _) => ExitCode::from(2),
        (false, true) => ExitCode::SUCCESS,
        (false, false) => ExitCode::from(1),
    })
}

struct Search<'a> {
    args: &'a Args,
    program: &'a Program,
    style: Style,
    /// Prefix output with file names, when searching more than one file.
    /// Without it there is one input, so `--explain` has a single text.
    prefix: bool,
    out: BufWriter<StdoutLock<'static>>,
    found: bool,
    failed: bool,
}

/// A match and the line it starts on.
struct Found<'t> {
    line: usize,
    /// Byte offset of the text the match was found in, within the file.
    base: usize,
    m: Match<'t>,
}

impl Search<'_> {
    fn fail(&mut self, message: &str) {
        eprintln!("sikt: {message}");
        self.failed = true;
    }

    fn file(&mut self, path: &Path, walked: bool) -> io::Result<()> {
        let name = path.strip_prefix(".").unwrap_or(path).display().to_string();
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.fail(&format!("{name}: {e}"));
                return Ok(());
            }
        };
        if walked && bytes[..bytes.len().min(8192)].contains(&0) {
            return Ok(());
        }
        match String::from_utf8(bytes) {
            Ok(text) => self.text(&name, Some(path), &text),
            Err(_) if self.program.action() == Action::Replace => {
                eprintln!("sikt: skipping {name}: not valid UTF-8");
                Ok(())
            }
            Err(e) => self.text(&name, Some(path), &String::from_utf8_lossy(e.as_bytes())),
        }
    }

    /// Searches one text: stdin or a file's contents.
    fn text(&mut self, name: &str, path: Option<&Path>, text: &str) -> io::Result<()> {
        let program = self.program;
        let mut found = Vec::new();
        let mut replaced = String::new();
        if self.args.multiline {
            // Line numbers count on from the previous match.
            let (mut counted, mut line) = (0, 1);
            for m in program.find_iter(text) {
                match m {
                    Ok(m) => {
                        line += text[counted..m.start()].matches('\n').count();
                        counted = m.start();
                        found.push(Found { line, base: 0, m });
                    }
                    Err(e) => {
                        self.fail(&format!("{name}: {e}"));
                        return Ok(());
                    }
                }
            }
            replaced = splice(program, text, found.iter().map(|f| &f.m));
        } else {
            for (i, line) in lines(text).enumerate() {
                let start = found.len();
                for m in program.find_iter(line.text) {
                    match m {
                        Ok(m) => found.push(Found {
                            line: i + 1,
                            base: line.start,
                            m,
                        }),
                        Err(e) => {
                            self.fail(&format!("{name}:{}: {e}", i + 1));
                            return Ok(());
                        }
                    }
                }
                replaced.push_str(&splice(
                    program,
                    line.text,
                    found[start..].iter().map(|f| &f.m),
                ));
                replaced.push_str(line.ending);
            }
        }
        self.found |= !found.is_empty();

        let args = self.args;
        if args.files_with_matches {
            if !found.is_empty() {
                writeln!(self.out, "{}", self.style.path(name))?;
            }
        } else if args.count {
            match (found.len(), self.prefix) {
                (0, true) => {}
                (n, true) => writeln!(self.out, "{}:{n}", self.style.path(name))?,
                (n, false) => writeln!(self.out, "{n}")?,
            }
        } else if args.json {
            for f in &found {
                self.json(name, f)?;
            }
        } else {
            match program.action() {
                Action::Print => {
                    for f in &found {
                        let line = program.render(&f.m).unwrap_or_default();
                        writeln!(self.out, "{line}")?;
                    }
                }
                Action::Replace => self.replace(name, path, text, &replaced)?,
                Action::Find if args.only_matching => {
                    for f in &found {
                        let lead = self.lead(name, f.line);
                        writeln!(self.out, "{lead}{}", self.style.matched(f.m.as_str()))?;
                    }
                }
                Action::Find => self.matching_lines(name, text, &found)?,
            }
        }

        if found.is_empty() && args.explain {
            self.explain(text)?;
        }
        Ok(())
    }

    /// `path:line:` or `line:`, as the options call for.
    fn lead(&self, name: &str, line: usize) -> String {
        if self.prefix {
            format!(
                "{}:{}:",
                self.style.path(name),
                self.style.line_number(line)
            )
        } else if self.args.line_number {
            format!("{}:", self.style.line_number(line))
        } else {
            String::new()
        }
    }

    /// Each line with a match, highlighted. In multiline mode, each line a
    /// match spans.
    fn matching_lines(&mut self, name: &str, text: &str, found: &[Found]) -> io::Result<()> {
        // Matches are sorted and don't overlap, so one cursor walks them along
        // with the lines.
        let spans: Vec<(usize, usize)> = found
            .iter()
            .map(|f| (f.base + f.m.start(), f.base + f.m.end()))
            .collect();
        let mut first = 0;
        for (i, line) in lines(text).enumerate() {
            // A match that ends right where this line starts belongs to the line before.
            while spans.get(first).is_some_and(|&(start, stop)| {
                stop < line.start || (stop == line.start && start < line.start)
            }) {
                first += 1;
            }
            if first == spans.len() {
                break;
            }
            let end = line.start + line.text.len();
            let hits: Vec<_> = spans[first..]
                .iter()
                .take_while(|&&(start, _)| start <= end)
                .map(|&(start, stop)| {
                    start.max(line.start) - line.start..stop.min(end) - line.start
                })
                .collect();
            if !hits.is_empty() {
                let lead = self.lead(name, i + 1);
                writeln!(self.out, "{lead}{}", self.style.highlight(line.text, &hits))?;
            }
        }
        Ok(())
    }

    fn json(&mut self, name: &str, f: &Found) -> io::Result<()> {
        let captures: serde_json::Map<String, serde_json::Value> = self
            .program
            .pattern()
            .capture_names()
            .map(|n| {
                (
                    n.to_owned(),
                    f.m.get(n).map_or(serde_json::Value::Null, Into::into),
                )
            })
            .collect();
        let record = serde_json::json!({
            "path": name,
            "line": f.line,
            "start": f.m.start(),
            "end": f.m.end(),
            "text": f.m.as_str(),
            "captures": captures,
        });
        writeln!(self.out, "{record}")
    }

    /// Writes the replaced text: to stdout for stdin, and for files a diff,
    /// or the file itself with `-w`.
    fn replace(
        &mut self,
        name: &str,
        path: Option<&Path>,
        text: &str,
        replaced: &str,
    ) -> io::Result<()> {
        match path {
            None => self.out.write_all(replaced.as_bytes()),
            Some(_) if replaced == text => Ok(()),
            Some(path) if self.args.write => {
                if let Err(e) = fs::write(path, replaced) {
                    self.fail(&format!("{name}: {e}"));
                }
                Ok(())
            }
            Some(_) => self
                .out
                .write_all(self.style.diff(name, text, replaced).as_bytes()),
        }
    }

    /// The closest attempt, on stderr. In line mode, the line that got
    /// farthest.
    fn explain(&mut self, text: &str) -> io::Result<()> {
        if self.prefix {
            self.fail("--explain works on one input at a time");
            return Ok(());
        }
        let pattern = self.program.pattern();
        let explanation = if self.args.multiline {
            pattern.explain(text)
        } else {
            let mut best: Option<sikt::Explanation> = None;
            let mut lines: Vec<Line> = lines(text).collect();
            if lines.is_empty() {
                lines.push(Line {
                    start: 0,
                    text: "",
                    ending: "",
                });
            }
            for (i, line) in lines.iter().enumerate() {
                match pattern.explain(line.text) {
                    Ok(Some(e)) => {
                        let progress = e.position() - e.start();
                        if best
                            .as_ref()
                            .is_none_or(|b| progress > b.position() - b.start())
                        {
                            best = Some(e.first_line(i + 1));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        self.fail(&e.to_string());
                        return Ok(());
                    }
                }
            }
            Ok(best)
        };
        match explanation {
            Ok(Some(e)) => eprintln!("{e}"),
            Ok(None) => eprintln!("the pattern matches, but `where` rejected every match"),
            Err(e) => self.fail(&e.to_string()),
        }
        Ok(())
    }
}

/// `text` with each match replaced by the program's output for it.
fn splice<'b, 'm: 'b>(
    program: &Program,
    text: &str,
    matches: impl Iterator<Item = &'b Match<'m>>,
) -> String {
    let mut out = String::new();
    let mut last = 0;
    for m in matches {
        out.push_str(&text[last..m.start()]);
        out.push_str(&program.render(m).unwrap_or_default());
        last = m.end();
    }
    out.push_str(&text[last..]);
    out
}

mod output;

use std::fs;
use std::io::{self, BufWriter, IsTerminal, Read, StdoutLock, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme};
use sikt::{Action, Match, Program, Table, Value};

use crate::output::{Style, quote};

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
    /// Print one JSON record per match: file, line, byte offsets into the file,
    /// text and captures.
    #[arg(long)]
    json: bool,
    /// Print each match's captures as one JSON object per line, with numbers
    /// for `integer` and `number` and without captures that didn't take part.
    #[arg(long, conflicts_with = "json")]
    rows: bool,
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
    // `use "path"` reads files next to the program file, or from the
    // working directory for a program on the command line.
    let base = args
        .file
        .as_deref()
        .and_then(Path::parent)
        .unwrap_or(Path::new(""))
        .to_owned();
    let program = match sikt::Pattern::builder()
        .case_insensitive(args.ignore_case)
        .line_mode(!args.multiline)
        .import_with(move |path| fs::read_to_string(base.join(path)).map_err(|e| e.to_string()))
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
        stderr_color,
        prefix: paths.len() > 1 || paths.iter().any(|p| p.is_dir()),
        out: BufWriter::new(io::stdout().lock()),
        found: false,
        failed: false,
        spans: Vec::new(),
    };
    if paths.is_empty() {
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes)?;
        search.input("<stdin>", None, &bytes)?;
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
    stderr_color: bool,
    /// Prefix output with file names, when searching more than one file.
    /// Without it there is one input, so `--explain` has a single text.
    prefix: bool,
    out: BufWriter<StdoutLock<'static>>,
    found: bool,
    failed: bool,
    /// Where the matches in a text are, kept to reuse its memory.
    spans: Vec<(usize, usize)>,
}

impl Search<'_> {
    fn fail(&mut self, message: &str) {
        eprintln!("sikt: {message}");
        self.failed = true;
    }

    fn file(&mut self, path: &Path, walked: bool) -> io::Result<()> {
        let name = path.strip_prefix(".").unwrap_or(path).display().to_string();
        // Only a file that is searched as bytes is mapped. One that becomes a
        // `str` or may be rewritten is read.
        let bytes = match Contents::load(path, !self.needs_text()) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.fail(&format!("{name}: {e}"));
                return Ok(());
            }
        };
        if walked && bytes[..bytes.len().min(8192)].contains(&0) {
            return Ok(());
        }
        self.input(&name, Some(path), &bytes)
    }

    /// One input: stdin or a file's contents. Searching works on the bytes;
    /// only `print` and `replace` need the text to be UTF-8.
    fn input(&mut self, name: &str, path: Option<&Path>, bytes: &[u8]) -> io::Result<()> {
        if let Some(table) = self.program.table() {
            return self.table(table, name, bytes);
        }
        if !self.needs_text() {
            return self.search(self.program, name, bytes, Position { byte: 0, line: 1 });
        }
        match std::str::from_utf8(bytes) {
            Ok(text) => self.transform(self.program, name, path, text),
            Err(_) if self.program.action() == Action::Replace => {
                eprintln!("sikt: skipping {name}: not valid UTF-8");
                Ok(())
            }
            Err(_) => self.transform(self.program, name, path, &String::from_utf8_lossy(bytes)),
        }
    }

    /// A `table` program's input: each header line starts a section of rows,
    /// searched with the program for that header. A section ends where the
    /// header comes again, as in concatenated output.
    fn table(&mut self, table: &Table, name: &str, bytes: &[u8]) -> io::Result<()> {
        // The line at `at`, without its newline, and where the next one starts.
        let line = |at: Position| {
            let end = memchr::memchr(b'\n', &bytes[at.byte..]).map_or(bytes.len(), |i| at.byte + i);
            let next = Position {
                byte: (end + 1).min(bytes.len()),
                line: at.line + 1,
            };
            (&bytes[at.byte..end], next)
        };
        let mut at = Position { byte: 0, line: 1 };
        while at.byte < bytes.len() {
            let (header, rows_start) = line(at);
            if header.trim_ascii().is_empty() {
                at = rows_start;
                continue;
            }
            let rows = match table.for_header(&String::from_utf8_lossy(header)) {
                Ok(rows) => rows,
                Err(e) => {
                    report(&e, self.stderr_color);
                    self.failed = true;
                    return Ok(());
                }
            };
            let mut stop = rows_start;
            while stop.byte < bytes.len() {
                let (row, next) = line(stop);
                if row == header {
                    break;
                }
                stop = next;
            }
            let section = &bytes[rows_start.byte..stop.byte];
            if rows.action() == Action::Print && !self.reports_only() {
                self.transform(&rows, name, None, &String::from_utf8_lossy(section))?;
            } else {
                self.search(&rows, name, section, rows_start)?;
            }
            at = stop;
        }
        Ok(())
    }

    /// Whether the input must be UTF-8 text: for `print` and `replace`, unless
    /// the output is only counts, names or JSON.
    fn needs_text(&self) -> bool {
        self.program.action() != Action::Find && !self.reports_only()
    }

    /// The output is counts, names or JSON, never `print` or `replace`.
    fn reports_only(&self) -> bool {
        let args = self.args;
        args.files_with_matches || args.count || args.json || args.rows
    }

    /// Finds the matches in `bytes` and reports them: matching lines, only
    /// the matches, counts, file names or JSON. Matches stream to the output
    /// as they're found.
    fn search(
        &mut self,
        program: &Program,
        name: &str,
        bytes: &[u8],
        at: Position,
    ) -> io::Result<()> {
        let args = self.args;
        let numbered = args.json || args.only_matching;
        let mut count = 0;
        // Line numbers count on from the previous match.
        let (mut counted, mut line) = (0, at.line);
        // Reused from file to file.
        let mut spans = std::mem::take(&mut self.spans);
        spans.clear();
        for m in program.find_iter_bytes(bytes) {
            let m = match m {
                Ok(m) => m,
                Err(e) => {
                    self.fail(&format!("{name}: {e}"));
                    return Ok(());
                }
            };
            count += 1;
            if args.files_with_matches {
                break;
            }
            if args.count {
                continue;
            }
            if numbered {
                line += memchr::memchr_iter(b'\n', &bytes[counted..m.start()]).count();
                counted = m.start();
            }
            if args.json {
                self.json(name, line, at.byte, &m)?;
            } else if args.rows {
                let row = serde_json::Value::Object(captures(&m, false));
                writeln!(self.out, "{row}")?;
            } else if args.only_matching {
                self.lead(name, line)?;
                self.style.matched(&mut self.out, m.as_bytes())?;
                writeln!(self.out)?;
            } else {
                spans.push((m.start(), m.end()));
            }
        }
        self.found |= count > 0;

        if args.files_with_matches {
            if count > 0 {
                self.style.path(&mut self.out, name)?;
                writeln!(self.out)?;
            }
        } else if args.count {
            if self.prefix && count > 0 {
                self.style.path(&mut self.out, name)?;
                write!(self.out, ":")?;
            }
            if !self.prefix || count > 0 {
                writeln!(self.out, "{count}")?;
            }
        } else if !(args.json || args.rows || args.only_matching) {
            self.matching_lines(name, bytes, &spans, at.line)?;
        }
        self.spans = spans;
        if count == 0 && args.explain {
            self.explain(&String::from_utf8_lossy(bytes))?;
        }
        Ok(())
    }

    /// Runs `print` or `replace` over `text`.
    fn transform(
        &mut self,
        program: &Program,
        name: &str,
        path: Option<&Path>,
        text: &str,
    ) -> io::Result<()> {
        let mut count = 0;
        let mut replaced = String::new();
        let mut replaced_up_to = 0;
        for m in program.find_iter(text) {
            let m = match m {
                Ok(m) => m,
                Err(e) => {
                    self.fail(&format!("{name}: {e}"));
                    return Ok(());
                }
            };
            count += 1;
            let output = program.render(&m).unwrap_or_default();
            if program.action() == Action::Print {
                writeln!(self.out, "{output}")?;
            } else {
                replaced.push_str(&text[replaced_up_to..m.start()]);
                replaced.push_str(&output);
                replaced_up_to = m.end();
            }
        }
        self.found |= count > 0;
        if program.action() == Action::Replace {
            replaced.push_str(&text[replaced_up_to..]);
            self.replace(name, path, text, &replaced)?;
        }
        if count == 0 && self.args.explain {
            self.explain(text)?;
        }
        Ok(())
    }

    /// `path:line:` or `line:`, as the options call for.
    fn lead(&mut self, name: &str, line: usize) -> io::Result<()> {
        if self.prefix {
            self.style.path(&mut self.out, name)?;
            write!(self.out, ":")?;
        }
        if self.prefix || self.args.line_number {
            self.style.line_number(&mut self.out, line)?;
            write!(self.out, ":")?;
        }
        Ok(())
    }

    /// Each line with a match, highlighted; a match across lines shows each
    /// line it covers. Jumps from match to match, so lines without one cost
    /// nothing but counting.
    fn matching_lines(
        &mut self,
        name: &str,
        bytes: &[u8],
        spans: &[(usize, usize)],
        first_line: usize,
    ) -> io::Result<()> {
        let (mut counted, mut number) = (0, first_line);
        let mut first = 0;
        let mut line_start = 0;
        while let Some(&(start, _)) = spans.get(first) {
            if start >= line_start {
                line_start = memchr::memrchr(b'\n', &bytes[line_start..start])
                    .map_or(line_start, |i| line_start + i + 1);
            }
            let newline = memchr::memchr(b'\n', &bytes[line_start..]).map(|i| line_start + i);
            let (end, next) = match newline {
                Some(nl) if nl > line_start && bytes[nl - 1] == b'\r' => (nl - 1, nl + 1),
                Some(nl) => (nl, nl + 1),
                None => (bytes.len(), bytes.len() + 1),
            };
            number += memchr::memchr_iter(b'\n', &bytes[counted..line_start]).count();
            counted = line_start;
            if start <= end {
                let hits = spans[first..]
                    .iter()
                    .take_while(|&&(start, _)| start <= end)
                    .map(|&(start, stop)| {
                        start.max(line_start) - line_start..stop.min(end) - line_start
                    });
                self.lead(name, number)?;
                self.style
                    .highlight(&mut self.out, &bytes[line_start..end], hits)?;
                writeln!(self.out)?;
            }
            // Matches that end on this line are done; one that runs on
            // shows again on the next line.
            while spans
                .get(first)
                .is_some_and(|&(start, stop)| start <= end && stop <= next)
            {
                first += 1;
            }
            line_start = next;
            if line_start > bytes.len() {
                break;
            }
        }
        Ok(())
    }

    /// Text that isn't valid UTF-8 gets replacement characters, since JSON
    /// strings must be Unicode.
    fn json(&mut self, name: &str, line: usize, base: usize, m: &Match<[u8]>) -> io::Result<()> {
        let captures = captures(m, true);
        let record = serde_json::json!({
            "path": name,
            "line": line,
            "start": base + m.start(),
            "end": base + m.end(),
            "text": String::from_utf8_lossy(m.as_bytes()),
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
        if self.program.table().is_some() {
            self.fail("--explain doesn't work with `table` yet");
            return Ok(());
        }
        match self.program.pattern().explain(text) {
            Ok(Some(e)) => eprintln!("{e}"),
            Ok(None) => eprintln!("the pattern matches, but `where` rejected every match"),
            Err(e) => self.fail(&e.to_string()),
        }
        Ok(())
    }
}

/// Files at least this large are mapped instead of read.
const MAP_AT: u64 = 1 << 16;

/// A file's bytes: mapped when it's large and only searched, read otherwise.
enum Contents {
    Read(Vec<u8>),
    Mapped(memmap2::Mmap),
}

impl Contents {
    fn load(path: &Path, map: bool) -> io::Result<Contents> {
        let mut file = fs::File::open(path)?;
        if map && file.metadata()?.len() >= MAP_AT {
            // SAFETY: the map is only read, and only searched as bytes, never
            // as `str`. If another process changes the file meanwhile, the
            // search can see a mix of old and new bytes, as with ripgrep.
            return Ok(Contents::Mapped(unsafe { memmap2::Mmap::map(&file)? }));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Contents::Read(bytes))
    }
}

impl std::ops::Deref for Contents {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Contents::Read(bytes) => bytes,
            Contents::Mapped(map) => map,
        }
    }
}

/// Where a text starts in its input: a byte offset and a 1-based line.
#[derive(Debug, Clone, Copy)]
struct Position {
    byte: usize,
    line: usize,
}

/// The captures of `m` as JSON, typed. Captures that didn't take part are
/// `null` with `keep_missing`, otherwise left out.
fn captures(m: &Match<[u8]>, keep_missing: bool) -> serde_json::Map<String, serde_json::Value> {
    m.values()
        .filter_map(|(name, value)| {
            let value = match value {
                None if keep_missing => serde_json::Value::Null,
                None => return None,
                Some(Value::Text(text)) => String::from_utf8_lossy(text).into(),
                Some(Value::Integer(i)) => i.into(),
                Some(Value::Number(x)) => {
                    serde_json::Number::from_f64(x).map_or(serde_json::Value::Null, Into::into)
                }
            };
            Some((name.to_owned(), value))
        })
        .collect()
}

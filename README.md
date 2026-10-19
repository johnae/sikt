Sikt is a pattern language for searching, extracting and replacing text, written so a
pattern reads like a sentence. It does what regular expressions do, with named captures,
typed values and error messages that explain themselves. It comes as a command-line tool
and a Rust library.

```sh
$ sikt 'find time: (2 digit ":" 2 digit ":" 2 digit) " [" level: 1+ upper "] "
        until (1+ digit "ms") ms: integer "ms"
        where ms > 1000
        print "{time} {level} {ms}"' app.log
15:13:07 WARN 1840
15:14:11 ERROR 30000
```

Literals are quoted and everything else is a word, so `"."` is a dot and `any` is any
character. Captures have names, `until` replaces greedy and lazy quantifiers, and
alternatives are tried in order.

## Why not regex?

The regex for the line above is `(\d{2}:\d{2}:\d{2}) \[([A-Z]+)\] .*?(\d+)ms`, and it
can't filter on the number. Sikt also tells you what went wrong:

```text
$ sikt 'year: 4 digts' dates.txt
  × unknown word `digts`
   ╭─[pattern:1:9]
 1 │ year: 4 digts
   ·         ──┬──
   ·           ╰── did you mean `digit`?
   ╰────
  help: to match the text, quote it: "digts"
```

It warns about patterns that can never match:

```text
$ sikt --check '0+ any "x"'
  ⚠ `0+ any` takes everything, so `"x"` can never match after it
   ╭─[pattern:1:8]
 1 │ 0+ any "x"
   ·        ───
   ╰────
  help: use `until "x"` to match up to it
```

And when nothing matches, it can show how close it got:

```text
$ sikt --explain '"[" level: 1+ upper "] " source: 1+ letter ":"' app.log
no match. closest attempt on line 1:
  matched `level`, `source`, then expected ":" but found " "
  2026-10-19 15:13:02 [WARN] db - slow
                               ^
```

## Installation

Requires Rust 1.90 or later.

```sh
cargo install --path crates/sikt-cli
```

## Using the command

`sikt PATTERN [PATH...]` searches files, directories (respecting `.gitignore`) or stdin,
one line at a time by default, like `grep`:

```sh
$ sikt '"[" ("WARN" or "ERROR") "]"' app.log
2026-10-19 15:13:07 [WARN] db - slow query took 1840ms
2026-10-19 15:14:11 [ERROR] api - timeout after 30000ms
```

Replace text like `sed`. On files, sikt shows a diff and only writes with `-w`:

```sh
$ echo 'Released 2026-10-19, patched 2026-11-02.' |
  sikt 'find year: 4 digit "-" month: 2 digit "-" day: 2 digit replace "{day}/{month}/{year}"'
Released 19/10/2026, patched 02/11/2026.
```

Turn lines into JSON with `--rows`, one object of captures per match:

```sh
$ sikt --rows 'date: until " " " " time: until " " " [" level: 1+ upper "] "
               source: 1+ letter " - " message: until text end' app.log
{"date":"2026-10-19","time":"15:13:02","level":"INFO","source":"api","message":"started in 312ms"}
{"date":"2026-10-19","time":"15:13:07","level":"WARN","source":"db","message":"slow query took 1840ms"}
```

Tables with a header row get their columns from it. `integer`, `number`, `size`,
`percent` and `duration` give captures real values:

```sh
$ df -h | sikt --rows 'table columns (fs size used avail use mount)
                       types size size used size use percent'
{"fs":"tmpfs","size":17179869184,"used":8267812045,"avail":"8.4G","use":48.0,"mount":"/"}
```

Ready-made rules for IP addresses, URLs, emails, dates, quoted strings and more come
with `use`: `use net find addr: ipv4`.

Longer programs can live in a file and run with `-f program.sikt`. Other options:
`-o` prints only matches, `-c` counts, `-l` lists files, `-i` ignores case, `-U` matches
across lines, `--json` prints full match records. See `sikt --help`.

## Using the library

It isn't on crates.io yet, so depend on it by path or git:

```toml
[dependencies]
sikt = { path = "../sikt/crates/sikt" }
```

```rust
use sikt::{Pattern, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pattern = Pattern::new(r#"key: 1+ letter "=" value: integer"#)?;
    for m in pattern.find_iter("retries=3 timeout=30") {
        let m = m?;
        if let Some(Value::Integer(n)) = m.value("value") {
            println!("{} is {n}", m.get("key").unwrap());
        }
    }
    Ok(())
}
// retries is 3
// timeout is 30
```

Patterns and programs compile once and are `Send + Sync`. They search `&str` or `&[u8]`
(`find_iter_bytes`) without allocating per match. Errors and warnings carry spans and
implement [`miette::Diagnostic`](https://docs.rs/miette).

## Learn the language

[The language guide](docs/language.md) covers everything: characters and sets,
repetition, `until`, captures, rules, modules, programs, templates and tables.

## Status

Sikt is young. The language and the API may still change before a first release.

## License

MIT or Apache-2.0, at your option.

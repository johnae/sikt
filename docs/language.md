# The sikt language

A sikt pattern describes text the way you'd say it: `4 digit "-" 2 digit` is four digits,
a dash and two digits. This guide covers the whole language. Every example runs as shown.

## A first pattern

Literal text goes in quotes, everything else is a word. Items next to each other match
one after the other:

```sh
$ echo 'order 1234-56 shipped' | sikt -o '4 digit "-" 2 digit'
1234-56
```

Whitespace and line breaks between items don't matter, and `#` starts a comment, so longer
patterns can be laid out however reads best.

## Literals

`"text"` and `'text'` match exactly that text. Inside quotes, `\n`, `\t`, `\r`, `\\`,
`\"`, `\'` and `\u{1F600}` are escapes. Nothing else is special: `"."` is a dot and `"*"`
is a star.

## Characters

| Word | Matches one |
| --- | --- |
| `any` | character, line breaks included |
| `digit` | ASCII digit, `0` to `9` |
| `hex` | hex digit, `0-9 a-f A-F` |
| `letter` | letter in any script |
| `upper`, `lower` | uppercase or lowercase letter |
| `wordchar` | letter, digit or `_` |
| `space` | whitespace character other than a line break |
| `punct` | ASCII punctuation |
| `newline` | line break, `\n` or `\r\n` |

`one of` and `none of` make a set from strings, kinds and ranges:

```sh
$ echo '#3e7, 0xff, zz' | sikt -o '1+ one of (digit "a".."f")'
3e7
0
ff
```

A string in a set stands for each of its characters, so `one of "aeiou"` is one vowel.

## Sequence and choice

`or` tries alternatives in order and takes the first that matches. It binds looser than a
sequence, so `"a" "b" or "c"` is `("a" "b") or "c"`. Use parentheses to group:

```sh
$ echo 'GET /a POST /b PATCH /c' | sikt -o '("GET" or "POST") " /" letter'
GET /a
POST /b
```

Since the first match wins, put the longer option first: in `"in" or "int"`, `"int"`
never gets a chance. Sikt warns about that.

## Repetition

The count comes before the thing, like in English:

| Pattern | Means |
| --- | --- |
| `3 digit` | exactly three |
| `1+ digit` | one or more |
| `0+ digit` | zero or more |
| `2..5 digit` | two to five |
| `optional "u"` | zero or one |

```sh
$ echo 'color colour' | sikt -o '"colo" optional "u" "r"'
color
colour
```

Repetition takes as much as it can and never gives any back. So `0+ any "x"` can never
match: `0+ any` has taken the `x` too. When you mean "everything up to", say so with
`until`.

## Until and through

`until x` matches everything up to `x`, without `x`. `through x` matches the same and `x`
as well. Both fail if `x` never comes, which is why `other=thing` doesn't match below:

```sh
$ echo 'key=value; other=thing' | sikt -o '1+ letter "=" until ";"'
key=value
$ echo '<b>bold</b> text' | sikt -o '"<" through ">"'
<b>
</b>
```

Without `-U`, sikt matches line by line, so `until` never runs past the end of a line.

## Captures

`name: item` captures the text that `item` matches under a name. A capture takes one
item, so put a sequence in parentheses. Names show up in `--rows`, templates and `where`:

```sh
$ echo 'ssh alice@db.example.org' | sikt --rows 'user: 1+ wordchar "@" host: (1+ one of (wordchar "."))'
{"user":"alice","host":"db.example.org"}
```

The same name can appear in several alternatives. Inside a repetition, a capture records
every occurrence and its value is the last one.

## Typed captures

Five words match numbers and give a capture of them a value instead of text:

| Word | Matches | Value |
| --- | --- | --- |
| `integer` | `42`, `-3`, `+4` | the integer |
| `number` | `1.5`, `.5`, `-0.25`, `7` | the number |
| `size` | `16G`, `4.0K`, `1.5GiB`, `512` | bytes |
| `percent` | `48%`, `1.5%` | the number before `%` |
| `duration` | `1h30m`, `250ms`, `26:58`, `1-02:03:04` | seconds |

For sizes, `K` `M` `G` `T` `P` `E` and `Ki` or `KiB` count in 1024s, the way `ls -h` and
`df -h` print them. `KB`, `MB` and `kB` count in 1000s. No unit means bytes.

Durations use `ns`, `us`, `ms`, `s`, `m`, `h`, `d` and `w`, or a clock: two parts are
minutes and seconds like `ps` prints them, three are hours, minutes and seconds, and a
`3-` in front counts days.

```sh
$ echo 'disk 1.5G used 48% of 3G' | sikt --rows 'used: size " used " pct: percent'
{"used":1610612736,"pct":48.0}
$ echo 'job a took 1m30s, job b took 2:05' | sikt -o 'find "took " t: duration where t > 100s'
took 2:05
```

## Anchors

`line start`, `line end`, `text start`, `text end`, `word start` and `word end` match
a position, not a character:

```sh
$ echo 'cat concat cats' | sikt -o 'word start "cat" word end'
cat
```

## Lookaround

`followed by x` and `after x` check what comes next or before without matching it.
`not followed by x` and `not after x` check that it isn't there. What comes `after` must
have a fixed number of characters:

```sh
$ echo 'width: 12px height: 3em' | sikt -o '1+ digit followed by "px"'
12
$ echo 'cost $12 or 34 kr' | sikt -o 'after "$" 1+ digit'
12
```

## Case

`nocase x` matches `x` in any case. `-i` makes the whole program case-insensitive, `where`
included:

```sh
$ echo 'Error ERROR error err' | sikt -o 'nocase "error"'
Error
ERROR
error
```

## Rules

`let name = pattern` names a pattern to use by name. A rule's body runs up to the next
`let` or `find`, and `find` gives the pattern to search for:

```sh
$ echo 'from 10.0.0.1 to 192.168.1.20' | sikt -o 'let octet = 1..3 digit
                                                find octet "." octet "." octet "." octet'
10.0.0.1
192.168.1.20
```

Rules can be recursive, as in balanced parentheses:

```sh
$ echo 'f(a, (b + c)) g(d' | sikt -o 'let group = "(" 0+ (none of "()" or group) ")"
                                       find group'
(a, (b + c))
```

A rule may not call itself before matching anything, as in `let sum = sum "+" digit`.
Sikt reports that as an error.

## Modules

`use` brings in rules from a module, so common things don't need writing again. Module
rules capture nothing, so name what you capture yourself:

```sh
$ echo '2026-10-19T15:13:02Z login from 10.0.0.7 as ada@example.org' |
  sikt --rows 'use net use time
               find when: datetime " login from " addr: ipv4 " as " user: email'
{"when":"2026-10-19T15:13:02Z","addr":"10.0.0.7","user":"ada@example.org"}
```

The built-in modules:

| Module | Rules |
| --- | --- |
| `net` | `ipv4`, `ipv6`, `hostname`, `url`, `email`, `mac` |
| `time` | `date` (`2026-10-19`), `clock` (`15:13:02.250`), `zone` (`Z`, `+02:00`), `datetime` |
| `text` | `quoted`, a string in `"` or `'` where `\` escapes the next character |
| `id` | `uuid`, `semver` (`1.24.0-rc.1+build.5`) |

`use "rules.sikt"` brings in the rules of a file of your own, which may hold only `let`
and `use`. The `sikt` command reads it next to the program file given with `-f`, or from
the working directory. In the library, `PatternBuilder::import_with` says how to read
files.

## Programs

A program wraps a pattern in statements. `find` gives the pattern, `where` keeps matches
that pass a condition, and `replace` or `print` say what to do with each match:

```sh
$ printf 'root:x:0:0\nalice:x:1001:1001\n' |
  sikt 'find user: until ":" ":" until ":" ":" uid: integer
        where uid > 1000
        print "{user} has uid {uid}"'
alice has uid 1001
```

Conditions in `where`:

| Condition | True when |
| --- | --- |
| `level is "WARN"`, `level is not "WARN"` | the capture is, or isn't, that text |
| `level in ("WARN" "ERROR")`, `not in (...)` | it's one of the strings, or none of them |
| `ms > 1000`, `>=`, `<`, `<=` | it's a number that compares so; units work for `size` and `duration` captures, as in `size > 1.5G` or `t > 1h30m` |
| `msg contains "timeout"` | the text contains the string |
| `path matches (until ".rs" ".rs")` | the pattern matches the whole capture |

Combine them with `and`, `or`, `not` and parentheses. `match` is the whole match. A
capture that didn't take part counts as empty text. A match that `where` rejects still
uses up its text; the search goes on after it.

`replace` rewrites each match. On stdin the result goes to stdout; on files sikt shows a
diff, and `-w` writes the files:

```sh
$ echo 'see https://example.org and http://old.example.org' |
  sikt 'find "http" optional "s" "://" host: until (space or text end) replace "<{host}>"'
see <example.org> and <old.example.org>
```

## Templates

`replace` and `print` take a template. `{name}` is a capture and `{match}` the whole
match. Filters follow a `|`: `lower`, `upper`, `trim`, `len`, and `default 'text'` for a
capture that didn't take part. `{{` and `}}` are literal braces:

```sh
$ echo 'name=Ada lang=' | sikt 'find key: 1+ letter "=" optional (value: 1+ letter)
                                print "{key | upper}: {value | default \"none\"}"'
NAME: Ada
LANG: none
```

## Tables

A `table` program reads the first line as a header and turns each line under it into
captures named after the columns:

```sh
$ printf 'NAME   CPU%%  MEM\nweb    12.5  1.5G\ndb     3.0   4G\n' |
  sikt --rows 'table types cpu number mem size'
{"name":"web","cpu":12.5,"mem":1610612736}
{"name":"db","cpu":3.0,"mem":4294967296}
```

Header names become capture names in lowercase, with other characters as `_` between
words: `%CPU` is `cpu`, `Mounted on` is `mounted_on`, and a repeated name gets `_2`.
These statements go with `table`:

| Statement | Does |
| --- | --- |
| `split by (2+ space)` | splits columns at a pattern instead of whitespace |
| `columns (fs size mount)` | names the columns when the header can't be split cleanly |
| `types pid integer cpu number` | gives columns a type |
| `with time: m: integer ":" s: integer` | runs a pattern on a column and adds its captures |

The last column takes the rest of its line, spaces included. A row with fewer columns
keeps the ones it has. A line equal to the header starts the table again, and `where`
and `print` work as in any program.

## Lines

The `sikt` command matches each line on its own, as if each line were a text of its own:
`text start` is the start of the line and nothing matches across a line break. `-U`
matches the whole input instead. The library does the same with `line_mode`.

## When things go wrong

Mistakes in a pattern are errors with the place marked and, where it can tell, a
suggestion. Patterns that work but probably don't do what you meant get a warning, like
an option that can never be reached or a repetition that leaves nothing for what follows.
`sikt --check` shows them without searching.

`--explain` shows, when nothing matched, the attempt that got furthest: what had matched
and what was expected instead.

## Coming from regex

| Regex | Sikt |
| --- | --- |
| `abc`, `\.` | `"abc"`, `"."` |
| `.` | `any` |
| `\d`, `\w`, `\s` | `digit`, `wordchar`, `space` |
| `[a-f0-9]`, `[^,;]` | `one of ("a".."f" digit)`, `none of ",;"` |
| `x*`, `x+`, `x?`, `x{2,5}` | `0+ x`, `1+ x`, `optional x`, `2..5 x` |
| `.*?x` | `until x` |
| `a\|b` | `a or b` |
| `(?<year>\d{4})` | `year: 4 digit` |
| `^`, `$`, `\b` | `line start`, `line end`, `word start` / `word end` |
| `(?=x)`, `(?!x)`, `(?<=x)` | `followed by x`, `not followed by x`, `after x` |
| `(?i)` | `nocase x`, or `-i` |

Two differences matter. Alternatives are tried in order and the first match wins, and
repetition never gives characters back, so there is no backtracking to surprise you.

//! Compares sikt with the `regex` crate on generated patterns where PEG and
//! regex semantics agree: a variable repetition is either last or followed by
//! a required item that can't start with anything the repetition could give
//! back. Then regex backtracking can never find a match the PEG misses.

use proptest::prelude::*;
use sikt::Pattern;

#[derive(Debug, Clone)]
enum Atom {
    Literal(String),
    Digit,
    Letter,
}

impl Atom {
    fn sikt(&self) -> String {
        match self {
            Atom::Literal(s) => format!("{s:?}"),
            Atom::Digit => "digit".into(),
            Atom::Letter => "letter".into(),
        }
    }

    fn regex(&self) -> String {
        match self {
            Atom::Literal(s) => format!("(?:{})", regex::escape(s)),
            Atom::Digit => "[0-9]".into(),
            Atom::Letter => "[a-zA-Z]".into(),
        }
    }

    /// Characters a match of this atom can start with.
    fn starts_with(&self, c: char) -> bool {
        match self {
            Atom::Literal(s) => s.starts_with(c),
            Atom::Digit => c.is_ascii_digit(),
            Atom::Letter => c.is_ascii_alphabetic(),
        }
    }
}

#[derive(Debug, Clone)]
struct Item {
    atom: Atom,
    min: u32,
    max: Option<u32>,
}

impl Item {
    fn variable(&self) -> bool {
        self.max != Some(self.min)
    }

    fn sikt(&self) -> String {
        let atom = self.atom.sikt();
        match self.max {
            Some(max) if max == self.min => format!("{max} {atom}"),
            Some(max) => format!("{}..{max} {atom}", self.min),
            None => format!("{}+ {atom}", self.min),
        }
    }

    fn regex(&self) -> String {
        let atom = self.atom.regex();
        match self.max {
            Some(max) => format!("{atom}{{{},{max}}}", self.min),
            None => format!("{atom}{{{},}}", self.min),
        }
    }
}

/// Makes a generated sequence safe to compare: non-empty, and every variable
/// repetition is last or followed by a disjoint required item.
fn repair(mut items: Vec<Item>) -> Vec<Item> {
    if items.iter().all(|i| i.min == 0) {
        items[0].min = 1;
        items[0].max = items[0].max.map(|m| m.max(1));
    }
    const ALPHABET: &str = "ab12.-xA9 ";
    for i in 0..items.len().saturating_sub(1) {
        let next = &items[i + 1];
        let overlaps = ALPHABET
            .chars()
            .any(|c| items[i].atom.starts_with(c) && next.atom.starts_with(c));
        if items[i].variable() && (next.min == 0 || overlaps) {
            let n = items[i].min.max(1);
            items[i].min = n;
            items[i].max = Some(n);
        }
    }
    items
}

fn atom() -> impl Strategy<Value = Atom> {
    prop_oneof![
        "[ab12.-]{1,3}".prop_map(Atom::Literal),
        Just(Atom::Digit),
        Just(Atom::Letter),
    ]
}

fn item() -> impl Strategy<Value = Item> {
    let quantity = prop_oneof![
        (1u32..3).prop_map(|n| (n, Some(n))),
        (0u32..2).prop_map(|n| (n, None)),
        (0u32..2, 1u32..3).prop_map(|(min, extra)| (min, Some(min + extra))),
    ];
    (atom(), quantity).prop_map(|(atom, (min, max))| Item { atom, min, max })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn agrees_with_regex(items in prop::collection::vec(item(), 1..5), input in "[ab12.\\-xA9 ]{0,30}") {
        let items = repair(items);
        let sikt_src = items.iter().map(Item::sikt).collect::<Vec<_>>().join(" ");
        let regex_src = items.iter().map(Item::regex).collect::<String>();

        let p = Pattern::new(&sikt_src).unwrap();
        let r = regex::Regex::new(&regex_src).unwrap();
        let sikt: Vec<_> = p.find_iter(&input).map(|m| m.unwrap().range()).collect();
        let regex: Vec<_> = r.find_iter(&input).map(|m| m.range()).collect();
        prop_assert_eq!(sikt, regex, "sikt `{}` vs regex `{}`", sikt_src, regex_src);
    }

    /// Arbitrary token soup must compile or fail cleanly, and matching must
    /// end without panicking.
    #[test]
    fn never_panics(
        tokens in prop::collection::vec(
            prop_oneof![
                Just("\"a\""), Just("'b'"), Just("\"\""), Just("digit"), Just("any"),
                Just("letter"), Just("newline"), Just("or"), Just("("), Just(")"),
                Just("1+"), Just("0+"), Just("2..3"), Just("3"), Just("optional"),
                Just("until"), Just("through"), Just("n:"), Just("m:"), Just("#"),
                Just("\n"), Just(".."), Just("+"), Just(":"), Just("\""),
            ],
            0..12,
        ),
        input in "[ab1\n]{0,20}",
    ) {
        let src = tokens.join(" ");
        if let Ok(p) = Pattern::builder().max_steps(100_000).build(&src) {
            for m in p.find_iter(&input).flatten() {
                for name in ["n", "m"] {
                    let _ = m.get(name);
                }
            }
        }
    }
}

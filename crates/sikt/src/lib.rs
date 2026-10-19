//! Sikt is a readable pattern language for searching, extracting and replacing
//! text. Literals are quoted, everything else is a word, and captures have
//! names:
//!
//! ```
//! let p = sikt::Pattern::new(r#"year: 4 digit "-" month: 2 digit"#)?;
//! let m = p.find("released 2026-10")?.unwrap();
//! assert_eq!(m.as_str(), "2026-10");
//! assert_eq!(m.get("year"), Some("2026"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Matching uses PEG semantics: `or` takes the first alternative that matches,
//! and repetition is possessive, never giving characters back.

mod class;
mod compile;
mod error;
mod explain;
mod lint;
mod pattern;
mod prefilter;
mod program;
mod syntax;
mod template;
mod vm;

pub use error::{Error, MatchError, Warning};
pub use explain::Explanation;
pub use pattern::{FindIter, Match, Pattern, PatternBuilder};
pub use program::{Action, Program};
pub use template::Template;

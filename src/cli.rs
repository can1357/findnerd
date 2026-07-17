use clap::Parser;

use crate::{category_flags::CategoryFlags, search::SearchMode};

/// Command-line contract for interactive and machine-readable searches.
#[derive(Debug, Parser)]
#[command(
	name = "findnerd",
	version,
	about = "Semantic search for every Nerd Font glyph",
	long_about = "Search a pre-embedded Nerd Fonts database. Run without a query for the \
	              interactive alternate-screen UI; pass a query for one-shot output."
)]
pub struct Cli {
	/// Query words for one-shot mode.
	#[arg(value_name = "QUERY", num_args = 0.., conflicts_with = "query")]
	pub(crate) terms: Vec<String>,

	/// Explicit query, including an empty query for filtered listing.
	#[arg(short, long, value_name = "TEXT")]
	pub(crate) query: Option<String>,

	/// Restrict results; supports field:value and a leading ! for negation.
	#[arg(short, long, value_name = "EXPR")]
	pub(crate) filter: Vec<String>,

	/// Restrict results to a category slug or alias; repeat or comma-separate
	/// for OR.
	#[arg(short, long, value_name = "CATEGORY", value_delimiter = ',')]
	pub(crate) category: Vec<String>,
	#[command(flatten)]
	category_flags:      CategoryFlags,

	/// Return structured JSON instead of a formatted list.
	#[arg(long)]
	pub(crate) json: bool,

	/// Use `SQLite` FTS5 BM25 ranking.
	#[arg(long, conflicts_with_all = ["semantic", "match_mode"])]
	pub(crate) bm25: bool,

	/// Use dense semantic-vector ranking.
	#[arg(long, conflicts_with_all = ["bm25", "match_mode"])]
	pub(crate) semantic: bool,

	/// Use typo-tolerant fuzzy name matching.
	#[arg(long = "match", conflicts_with_all = ["bm25", "semantic"])]
	pub(crate) match_mode: bool,

	/// Force the interactive UI and use any query as its initial text.
	#[arg(short, long, conflicts_with_all = ["json", "list_categories"])]
	pub(crate) interactive: bool,

	/// Print available category slugs and exit.
	#[arg(long)]
	pub(crate) list_categories: bool,

	/// Maximum one-shot results.
	#[arg(short, long, default_value_t = 20, value_parser = parse_limit)]
	pub(crate) limit: usize,

	/// Disable ANSI color in formatted one-shot output.
	#[arg(long)]
	pub(crate) no_color: bool,
}

impl Cli {
	/// Combines unquoted positional terms while preserving explicit empty
	/// queries.
	pub(crate) fn query_text(&self) -> Option<String> {
		self
			.query
			.clone()
			.or_else(|| (!self.terms.is_empty()).then(|| self.terms.join(" ")))
	}

	/// Combines generic category selectors with direct positive switches.
	pub(crate) fn included_categories(&self) -> Vec<String> {
		let mut selected = self.category.clone();
		for category in self.category_flags.included() {
			if !selected.contains(&category) {
				selected.push(category);
			}
		}
		selected
	}

	/// Returns direct negative category switches; exclusions take precedence.
	pub(crate) fn excluded_categories(&self) -> Vec<String> {
		self.category_flags.excluded()
	}

	/// Chooses the mode shortcut, defaulting to rank fusion.
	pub(crate) const fn mode(&self) -> SearchMode {
		if self.bm25 {
			SearchMode::Bm25
		} else if self.semantic {
			SearchMode::Semantic
		} else if self.match_mode {
			SearchMode::Match
		} else {
			SearchMode::Hybrid
		}
	}

	/// Enters the TUI when forced or when no one-shot signal is present.
	pub(crate) fn use_interactive(&self) -> bool {
		self.interactive || (self.query_text().is_none() && !self.json && !self.list_categories)
	}
}

fn parse_limit(value: &str) -> Result<usize, String> {
	let limit = value
		.parse::<usize>()
		.map_err(|_| "limit must be an integer".to_owned())?;
	if (1..=500).contains(&limit) {
		Ok(limit)
	} else {
		Err("limit must be between 1 and 500".to_owned())
	}
}

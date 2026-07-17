use std::{collections::HashSet, fmt, sync::LazyLock};

use anyhow::{Result, bail, ensure};
use clap::ValueEnum;
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use serde::Serialize;

use crate::{
	catalog::{Catalog, Category, Icon},
	semantic::{SemanticEncoder, cosine, has_expansion, tokens},
};

/// Selects the ranking strategy used by one-shot and interactive searches.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
	/// Reciprocal-rank fusion over semantic, BM25, and fuzzy matching.
	#[default]
	Hybrid,
	/// Dense-vector retrieval from the embedded semantic index.
	Semantic,
	/// `SQLite` FTS5 ranking over names, labels, and aliases.
	Bm25,
	/// Typo-tolerant subsequence matching over icon names.
	Match,
}

impl SearchMode {
	/// Returns the next strategy shown by the interactive mode switcher.
	pub(crate) const fn next(self) -> Self {
		match self {
			Self::Hybrid => Self::Semantic,
			Self::Semantic => Self::Bm25,
			Self::Bm25 => Self::Match,
			Self::Match => Self::Hybrid,
		}
	}

	/// Returns the previous strategy shown by the interactive mode switcher.
	pub(crate) const fn previous(self) -> Self {
		match self {
			Self::Hybrid => Self::Match,
			Self::Semantic => Self::Hybrid,
			Self::Bm25 => Self::Semantic,
			Self::Match => Self::Bm25,
		}
	}
}

impl fmt::Display for SearchMode {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		let value = match self {
			Self::Hybrid => "hybrid",
			Self::Semantic => "semantic",
			Self::Bm25 => "bm25",
			Self::Match => "match",
		};
		formatter.write_str(value)
	}
}

/// All constraints for one search operation.
#[derive(Debug)]
pub struct SearchRequest {
	pub(crate) query:               String,
	pub(crate) filters:             Vec<String>,
	pub(crate) categories:          Vec<String>,
	pub(crate) excluded_categories: Vec<String>,
	pub(crate) mode:                SearchMode,
	pub(crate) limit:               usize,
}

/// One ranked glyph in a search response.
#[derive(Clone, Copy, Debug)]
pub struct SearchHit {
	pub(crate) icon:  usize,
	pub(crate) score: f64,
}

/// Ranked results plus the number available before applying the limit.
#[derive(Debug)]
pub struct SearchResponse {
	pub(crate) hits:  Vec<SearchHit>,
	pub(crate) total: usize,
}

/// Searches the embedded catalog without network or filesystem access.
pub struct SearchEngine {
	catalog: Catalog,
	encoder: LazyLock<std::result::Result<SemanticEncoder, String>>,
}

impl SearchEngine {
	/// Loads the catalog and precomputed vectors embedded in the executable.
	pub(crate) fn open() -> Result<Self> {
		Ok(Self { catalog: Catalog::open()?, encoder: LazyLock::new(load_encoder) })
	}

	/// Executes category and field filters before applying the selected ranker.
	pub(crate) fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
		ensure!(request.limit > 0, "result limit must be greater than zero");
		let candidates = self.candidates(request)?;
		if request.query.trim().is_empty() {
			let total = candidates.len();
			let hits = candidates
				.into_iter()
				.take(request.limit)
				.map(|icon| SearchHit { icon, score: 1.0 })
				.collect();
			return Ok(SearchResponse { hits, total });
		}

		let ranked = match request.mode {
			SearchMode::Hybrid => self.hybrid(&request.query, &candidates)?,
			SearchMode::Semantic => self.semantic(&request.query, &candidates)?,
			SearchMode::Bm25 => self.bm25(&request.query, &candidates)?,
			SearchMode::Match => self.fuzzy(&request.query, &candidates),
		};
		let total = ranked.len();
		let hits = normalize_hits(ranked, request.limit);
		Ok(SearchResponse { hits, total })
	}

	/// Returns a glyph by the stable index carried in [`SearchHit`].
	pub(crate) fn icon(&self, index: usize) -> &Icon {
		&self.catalog.icons[index]
	}

	/// Returns category metadata for a glyph or category selector.
	pub(crate) fn category(&self, index: usize) -> &Category {
		&self.catalog.categories[index]
	}

	/// Lists all category selectors in database order.
	pub(crate) fn categories(&self) -> &[Category] {
		&self.catalog.categories
	}

	fn candidates(&self, request: &SearchRequest) -> Result<Vec<usize>> {
		let selected_categories = self.category_indexes(&request.categories)?;
		let excluded_categories = self.category_indexes(&request.excluded_categories)?;
		let filters = request
			.filters
			.iter()
			.map(|filter| Filter::parse(filter))
			.collect::<Result<Vec<_>>>()?;

		Ok(self
			.catalog
			.icons
			.iter()
			.enumerate()
			.filter(|(_, icon)| {
				(selected_categories.is_empty() || selected_categories.contains(&icon.category))
					&& !excluded_categories.contains(&icon.category)
					&& filters
						.iter()
						.all(|filter| filter.matches(icon, &self.catalog.categories[icon.category]))
			})
			.map(|(index, _)| index)
			.collect())
	}

	fn category_indexes(&self, values: &[String]) -> Result<HashSet<usize>> {
		values
			.iter()
			.map(|category| {
				self.catalog.category_index(category).ok_or_else(|| {
					anyhow::anyhow!(
						"unknown category {category:?}; use one of: {}",
						self
							.catalog
							.categories
							.iter()
							.map(|entry| entry.slug.as_str())
							.collect::<Vec<_>>()
							.join(", ")
					)
				})
			})
			.collect()
	}

	fn semantic(&self, query: &str, candidates: &[usize]) -> Result<Vec<(usize, f64)>> {
		let encoder = LazyLock::force(&self.encoder)
			.as_ref()
			.map_err(|error| anyhow::anyhow!("{error}"))?;
		let Some(query_vector) = encoder.encode_query(query, &self.catalog.query_expansions) else {
			return Ok(Vec::new());
		};
		let mut ranked = candidates
			.iter()
			.map(|index| {
				let similarity = cosine(&query_vector, &self.catalog.icons[*index].embedding);
				(*index, f64::midpoint(f64::from(similarity), 1.0))
			})
			.collect::<Vec<_>>();
		self.sort(&mut ranked);
		Ok(ranked)
	}

	fn bm25(&self, query: &str, candidates: &[usize]) -> Result<Vec<(usize, f64)>> {
		let allowed = candidate_mask(self.catalog.icons.len(), candidates);
		let mut ranked = self
			.catalog
			.bm25(&tokens(query))?
			.into_iter()
			.filter(|(index, _)| allowed[*index])
			.collect::<Vec<_>>();
		self.sort(&mut ranked);
		Ok(ranked)
	}

	fn fuzzy(&self, query: &str, candidates: &[usize]) -> Vec<(usize, f64)> {
		let matcher = SkimMatcherV2::default();
		let needle = query.trim().to_ascii_lowercase();
		let compact_needle = tokens(&needle).join("_");
		let mut ranked = candidates
			.iter()
			.filter_map(|index| {
				let icon = &self.catalog.icons[*index];
				let name_score = matcher.fuzzy_match(&icon.name, &compact_needle);
				let label_score = matcher.fuzzy_match(&icon.label, &needle);
				let mut score = name_score.max(label_score)?;
				if icon.label == needle || icon.name == needle {
					score += 10_000;
				} else if icon.label.starts_with(&needle)
					|| icon
						.name
						.strip_prefix(icon_category_prefix(&icon.name))
						.is_some_and(|name| name.trim_start_matches('-').starts_with(&compact_needle))
				{
					score += 2_000;
				}
				Some((*index, score_to_f64(score)))
			})
			.collect::<Vec<_>>();
		self.sort(&mut ranked);
		ranked
	}

	fn hybrid(&self, query: &str, candidates: &[usize]) -> Result<Vec<(usize, f64)>> {
		let semantic = self.semantic(query, candidates)?;
		let bm25 = self.bm25(query, candidates)?;
		let fuzzy = self.fuzzy(query, candidates);
		let mut fused = vec![0.0; self.catalog.icons.len()];

		let semantic_weight = if has_expansion(query, &self.catalog.query_expansions) {
			4.0
		} else {
			1.25
		};
		add_rrf(&mut fused, &semantic, semantic_weight);
		add_rrf(&mut fused, &bm25, 1.0);
		add_rrf(&mut fused, &fuzzy, 0.9);

		let mut ranked = candidates
			.iter()
			.filter_map(|index| (fused[*index] > 0.0).then_some((*index, fused[*index])))
			.collect::<Vec<_>>();
		self.sort(&mut ranked);
		Ok(ranked)
	}

	fn sort(&self, ranked: &mut [(usize, f64)]) {
		ranked.sort_unstable_by(|left, right| {
			right.1.total_cmp(&left.1).then_with(|| {
				self.catalog.icons[left.0]
					.name
					.cmp(&self.catalog.icons[right.0].name)
			})
		});
	}
}
fn load_encoder() -> std::result::Result<SemanticEncoder, String> {
	SemanticEncoder::open().map_err(|error| format!("{error:#}"))
}

fn icon_category_prefix(name: &str) -> &str {
	name.split_once('-').map_or("", |(prefix, _)| prefix)
}

fn score_to_f64(score: i64) -> f64 {
	f64::from(i32::try_from(score).expect("fuzzy score fits i32"))
}

fn candidate_mask(icon_count: usize, candidates: &[usize]) -> Vec<bool> {
	let mut mask = vec![false; icon_count];
	for candidate in candidates {
		mask[*candidate] = true;
	}
	mask
}

fn add_rrf(fused: &mut [f64], ranking: &[(usize, f64)], weight: f64) {
	for (rank, (index, _)) in ranking.iter().take(2_000).enumerate() {
		let rank = u32::try_from(rank + 1).expect("rank fits u32");
		fused[*index] += weight / (60.0 + f64::from(rank));
	}
}

fn normalize_hits(ranked: Vec<(usize, f64)>, limit: usize) -> Vec<SearchHit> {
	let maximum = ranked.first().map_or(1.0, |(_, score)| *score);
	ranked
		.into_iter()
		.take(limit)
		.map(|(icon, score)| SearchHit {
			icon,
			score: if maximum > 0.0 { score / maximum } else { 0.0 },
		})
		.collect()
}

#[derive(Debug)]
struct Filter {
	negative: bool,
	field:    FilterField,
}

impl Filter {
	fn parse(raw: &str) -> Result<Self> {
		let raw = raw.trim();
		ensure!(!raw.is_empty(), "filter cannot be empty");
		let (negative, expression) = raw
			.strip_prefix('!')
			.map_or((false, raw), |expression| (true, expression));
		ensure!(!expression.is_empty(), "filter cannot be only !");

		let field = if let Some((field, value)) = expression.split_once(':') {
			ensure!(!value.is_empty(), "filter value cannot be empty");
			match field.to_ascii_lowercase().as_str() {
				"name" => FilterField::Text(TextField::Name, value.to_ascii_lowercase()),
				"label" => FilterField::Text(TextField::Label, value.to_ascii_lowercase()),
				"alias" | "aliases" => {
					FilterField::Text(TextField::Aliases, value.to_ascii_lowercase())
				},
				"category" | "cat" => {
					FilterField::Text(TextField::Category, value.to_ascii_lowercase())
				},
				"glyph" => FilterField::Glyph(value.to_owned()),
				"code" | "codepoint" | "unicode" => FilterField::Code(parse_codepoint(value)?),
				unknown => bail!(
					"unknown filter field {unknown:?}; use name, label, alias, category, glyph, or code"
				),
			}
		} else {
			FilterField::Text(TextField::Any, expression.to_ascii_lowercase())
		};

		Ok(Self { negative, field })
	}

	fn matches(&self, icon: &Icon, category: &Category) -> bool {
		let matched = match &self.field {
			FilterField::Text(field, value) => match field {
				TextField::Any => icon.search_text.contains(value),
				TextField::Name => icon.name.contains(value),
				TextField::Label => icon.label.contains(value),
				TextField::Aliases => icon.aliases.contains(value),
				TextField::Category => {
					category.slug.contains(value)
						|| category.name.to_ascii_lowercase().contains(value)
						|| category.aliases.contains(value)
				},
			},
			FilterField::Glyph(glyph) => icon.glyph == *glyph,
			FilterField::Code(codepoint) => icon.codepoint == *codepoint,
		};
		matched != self.negative
	}
}

#[derive(Debug)]
enum FilterField {
	Text(TextField, String),
	Glyph(String),
	Code(u32),
}

#[derive(Debug)]
enum TextField {
	Any,
	Name,
	Label,
	Aliases,
	Category,
}

fn parse_codepoint(value: &str) -> Result<u32> {
	let normalized = value
		.trim()
		.strip_prefix("U+")
		.or_else(|| value.trim().strip_prefix("0x"))
		.unwrap_or_else(|| value.trim());
	u32::from_str_radix(normalized, 16)
		.map_err(|_| anyhow::anyhow!("invalid hexadecimal codepoint {value:?}"))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn engine() -> SearchEngine {
		SearchEngine::open().expect("embedded index should load")
	}

	fn request(query: &str, mode: SearchMode) -> SearchRequest {
		SearchRequest {
			query: query.to_owned(),
			filters: Vec::new(),
			categories: Vec::new(),
			excluded_categories: Vec::new(),
			mode,
			limit: 10,
		}
	}

	#[test]
	fn semantic_search_connects_settings_and_cogs() {
		let engine = engine();
		let response = engine
			.search(&request("configuration preferences", SearchMode::Semantic))
			.expect("semantic search should succeed");

		assert!(response.hits.iter().any(|hit| {
			let name = &engine.icon(hit.icon).name;
			name.contains("cog") || name.contains("gear") || name.contains("settings")
		}));
	}

	#[test]
	fn hybrid_search_interprets_view_as_an_eye_icon() {
		let engine = engine();
		let response = engine
			.search(&request("view", SearchMode::Hybrid))
			.expect("hybrid search should succeed");

		assert!(
			engine.icon(response.hits[0].icon).label.starts_with("eye"),
			"top result was {}",
			engine.icon(response.hits[0].icon).name
		);
	}

	#[test]
	fn filters_and_categories_are_applied_before_ranking() {
		let engine = engine();
		let mut request = request("database", SearchMode::Hybrid);
		request.categories.push("cod".to_owned());
		request.filters.push("!name:remote".to_owned());
		let response = engine
			.search(&request)
			.expect("filtered search should succeed");

		assert!(!response.hits.is_empty());
		assert!(response.hits.iter().all(|hit| {
			let icon = engine.icon(hit.icon);
			engine.category(icon.category).slug == "cod" && !icon.name.contains("remote")
		}));
	}

	#[test]
	fn codepoint_filter_selects_the_exact_glyph() {
		let engine = engine();
		let request = SearchRequest {
			query:               String::new(),
			filters:             vec!["code:eb99".to_owned()],
			categories:          Vec::new(),
			excluded_categories: Vec::new(),
			mode:                SearchMode::Hybrid,
			limit:               10,
		};
		let response = engine
			.search(&request)
			.expect("codepoint filter should parse");

		assert_eq!(response.total, 1);
		assert_eq!(engine.icon(response.hits[0].icon).name, "cod-account");
	}
}

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use model2vec_rs::model::StaticModel;

pub const DIMENSIONS: usize = 256;

#[cfg(feature = "embed-model")]
static TOKENIZER: &[u8] = include_bytes!("../assets/model/tokenizer.json");
#[cfg(feature = "embed-model")]
static MODEL: &[u8] = include_bytes!("../assets/model/model.safetensors");
#[cfg(feature = "embed-model")]
static CONFIG: &[u8] = include_bytes!("../assets/model/config.json");

/// Runs the embedded `Model2Vec` query encoder used by semantic retrieval.
pub struct SemanticEncoder {
	model: StaticModel,
}

impl SemanticEncoder {
	/// Loads the pretrained potion-base-8M model entirely from executable bytes.
	#[cfg(feature = "embed-model")]
	pub(crate) fn open() -> Result<Self> {
		let model = StaticModel::from_bytes(TOKENIZER, MODEL, CONFIG, None)
			.context("loading embedded potion-base-8M model")?;
		Ok(Self { model })
	}

	/// Loads the pretrained potion-base-8M model from the user cache,
	/// downloading the pinned revision on first use.
	#[cfg(all(feature = "fetch-model", not(feature = "embed-model")))]
	pub(crate) fn open() -> Result<Self> {
		let files = crate::model_cache::ensure()?;
		let model = StaticModel::from_bytes(files.tokenizer, files.model, files.config, None)
			.context("loading cached potion-base-8M model")?;
		Ok(Self { model })
	}

	/// Encodes a query after enriching known icon intents with related language.
	pub(crate) fn encode_query(
		&self,
		query: &str,
		expansions: &HashMap<String, String>,
	) -> Option<Vec<f32>> {
		let query = query.trim();
		if query.is_empty() {
			return None;
		}

		let expanded = expand_query(query, expansions);
		let mut embeddings = self.model.encode(&[expanded]);
		let embedding = embeddings.pop()?;
		debug_assert_eq!(embedding.len(), DIMENSIONS);
		Some(embedding)
	}
}

/// Splits search text into the alphanumeric tokens indexed by `SQLite` FTS5.
pub fn tokens(text: &str) -> Vec<String> {
	let mut result = Vec::new();
	let mut token = String::new();

	for character in text.chars().flat_map(char::to_lowercase) {
		if character.is_ascii_alphanumeric() {
			token.push(character);
		} else if !token.is_empty() {
			result.push(std::mem::take(&mut token));
		}
	}
	if !token.is_empty() {
		result.push(token);
	}

	result
}

/// Calculates cosine similarity against a normalized, signed-byte icon vector.
pub fn cosine(query: &[f32], icon: &[u8]) -> f32 {
	query
		.iter()
		.zip(icon)
		.map(|(left, right)| left * decode(*right))
		.sum::<f32>()
		.clamp(-1.0, 1.0)
}

/// Reports whether a query contains a concept known by the semantic expansion
/// table.
pub fn has_expansion(query: &str, expansions: &HashMap<String, String>) -> bool {
	tokens(query)
		.iter()
		.any(|token| expansions.contains_key(token))
}

fn expand_query(query: &str, expansions: &HashMap<String, String>) -> String {
	let mut seen = tokens(query).into_iter().collect::<HashSet<_>>();
	let mut expanded = query.to_owned();

	for token in tokens(query) {
		let Some(related) = expansions.get(&token) else {
			continue;
		};
		for term in related.split_ascii_whitespace() {
			if seen.insert(term.to_owned()) {
				expanded.push(' ');
				expanded.push_str(term);
			}
		}
	}
	expanded
}

fn decode(value: u8) -> f32 {
	f32::from(i8::from_ne_bytes([value])) / 127.0
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn tokenization_matches_index_rules() {
		assert_eq!(tokens("GitHub / arrow_left + C++"), ["github", "arrow", "left", "c"]);
	}

	#[test]
	fn query_expansion_adds_related_terms_once() {
		let expansions =
			HashMap::from([("view".to_owned(), "eye preview show view visible".to_owned())]);

		assert_eq!(expand_query("view preview", &expansions), "view preview eye show visible");
	}
}

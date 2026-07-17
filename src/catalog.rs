use std::{collections::HashMap, io};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, MAIN_DB, params};

use crate::semantic::DIMENSIONS;

static DATABASE: &[u8] = include_bytes!("../assets/icons.db");

/// A Nerd Font glyph and its precomputed search metadata.
#[derive(Debug)]
pub struct Icon {
	pub(crate) name:        String,
	pub(crate) label:       String,
	pub(crate) glyph:       String,
	pub(crate) codepoint:   u32,
	pub(crate) category:    usize,
	pub(crate) aliases:     String,
	pub(crate) search_text: String,
	pub(crate) embedding:   Box<[u8]>,
}

/// A glyph collection exposed as a category filter.
#[derive(Debug)]
pub struct Category {
	pub(crate) slug:       String,
	pub(crate) name:       String,
	pub(crate) aliases:    String,
	pub(crate) icon_count: usize,
}

/// The immutable `SQLite` catalog and its hot in-memory vector data.
pub struct Catalog {
	connection:                  Connection,
	pub(crate) icons:            Vec<Icon>,
	pub(crate) categories:       Vec<Category>,
	pub(crate) query_expansions: HashMap<String, String>,
}

impl Catalog {
	/// Opens the database directly from bytes embedded in the executable.
	pub(crate) fn open() -> Result<Self> {
		let mut connection = Connection::open_in_memory().context("opening in-memory catalog")?;
		connection
			.deserialize_bytes(MAIN_DB, DATABASE)
			.context("loading embedded Nerd Fonts catalog")?;

		let schema_version: String = connection
			.query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0))
			.context("reading catalog schema version")?;
		ensure!(schema_version == "2", "unsupported embedded catalog schema {schema_version}");

		let dimensions: usize = connection
			.query_row("SELECT value FROM meta WHERE key = 'dimensions'", [], |row| {
				let value: String = row.get(0)?;
				value.parse().map_err(|error| {
					rusqlite::Error::FromSqlConversionFailure(
						0,
						rusqlite::types::Type::Text,
						Box::new(error),
					)
				})
			})
			.context("reading embedding dimensions")?;
		ensure!(
			dimensions == DIMENSIONS,
			"embedded vectors have {dimensions} dimensions, expected {DIMENSIONS}"
		);

		let categories = load_categories(&connection)?;
		let category_indexes = categories
			.iter()
			.enumerate()
			.map(|(index, category)| (category.slug.as_str(), index))
			.collect::<HashMap<_, _>>();
		let icons = load_icons(&connection, &category_indexes)?;
		let query_expansions = load_query_expansions(&connection)?;

		connection
			.pragma_update(None, "query_only", true)
			.context("making embedded catalog read-only")?;

		Ok(Self { connection, icons, categories, query_expansions })
	}

	/// Returns BM25-ranked icon indexes for normalized query tokens.
	pub(crate) fn bm25(&self, query_tokens: &[String]) -> Result<Vec<(usize, f64)>> {
		if query_tokens.is_empty() {
			return Ok(Vec::new());
		}

		let strict = fts_expression(query_tokens, " AND ");
		let mut ranked = self.run_fts(&strict)?;
		if ranked.is_empty() && query_tokens.len() > 1 {
			let relaxed = fts_expression(query_tokens, " OR ");
			ranked = self.run_fts(&relaxed)?;
		}
		Ok(ranked)
	}

	/// Resolves a category slug, display name, or documented alias.
	pub(crate) fn category_index(&self, value: &str) -> Option<usize> {
		let needle = value.trim().to_ascii_lowercase();
		self.categories.iter().position(|category| {
			category.slug == needle
				|| category.name.to_ascii_lowercase() == needle
				|| category
					.aliases
					.split_ascii_whitespace()
					.any(|alias| alias == needle)
		})
	}

	fn run_fts(&self, expression: &str) -> Result<Vec<(usize, f64)>> {
		let mut statement = self
			.connection
			.prepare_cached(
				"SELECT icon.id, -bm25(icon_fts, 8.0, 5.0, 2.0, 0.5) AS score
                 FROM icon_fts
                 JOIN icon ON icon.id = icon_fts.rowid
                 WHERE icon_fts MATCH ?1
                 ORDER BY score DESC, icon.name ASC",
			)
			.context("preparing BM25 search")?;
		let rows = statement
			.query_map(params![expression], |row| {
				let id: usize = row.get(0)?;
				let score: f64 = row.get(1)?;
				Ok((id - 1, score))
			})
			.context("running BM25 search")?;

		rows
			.collect::<rusqlite::Result<Vec<_>>>()
			.context("reading BM25 results")
	}
}

fn load_categories(connection: &Connection) -> Result<Vec<Category>> {
	let mut statement = connection
		.prepare("SELECT slug, name, aliases, icon_count FROM category ORDER BY rowid")
		.context("preparing category load")?;
	let rows = statement.query_map([], |row| {
		Ok(Category {
			slug:       row.get(0)?,
			name:       row.get(1)?,
			aliases:    row.get(2)?,
			icon_count: row.get(3)?,
		})
	})?;

	rows
		.collect::<rusqlite::Result<Vec<_>>>()
		.context("loading categories")
}

fn load_icons(connection: &Connection, categories: &HashMap<&str, usize>) -> Result<Vec<Icon>> {
	let mut statement = connection
		.prepare(
			"SELECT id, name, label, glyph, codepoint, category, aliases, search_text, embedding
             FROM icon ORDER BY id",
		)
		.context("preparing icon load")?;
	let rows = statement.query_map([], |row| {
		let id: usize = row.get(0)?;
		let category_slug: String = row.get(5)?;
		let category = categories
			.get(category_slug.as_str())
			.copied()
			.ok_or_else(|| {
				rusqlite::Error::InvalidColumnType(5, category_slug, rusqlite::types::Type::Text)
			})?;
		let embedding: Vec<u8> = row.get(8)?;
		if embedding.len() != DIMENSIONS {
			return Err(invalid_embedding_size(8, embedding.len()));
		}
		if id == 0 {
			return Err(rusqlite::Error::IntegralValueOutOfRange(0, 0));
		}
		Ok((id, Icon {
			name: row.get(1)?,
			label: row.get(2)?,
			glyph: row.get(3)?,
			codepoint: row.get(4)?,
			category,
			aliases: row.get(6)?,
			search_text: row.get::<_, String>(7)?.to_ascii_lowercase(),
			embedding: embedding.into_boxed_slice(),
		}))
	})?;

	let mut icons = Vec::new();
	for row in rows {
		let (id, icon) = row.context("reading icon row")?;
		ensure!(id == icons.len() + 1, "embedded icon IDs are not contiguous");
		icons.push(icon);
	}
	Ok(icons)
}

fn load_query_expansions(connection: &Connection) -> Result<HashMap<String, String>> {
	let mut statement = connection
		.prepare("SELECT term, expansion FROM query_expansion")
		.context("preparing semantic expansions load")?;
	let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;

	rows
		.collect::<rusqlite::Result<HashMap<_, _>>>()
		.context("loading semantic expansions")
}

fn invalid_embedding_size(column: usize, actual: usize) -> rusqlite::Error {
	rusqlite::Error::FromSqlConversionFailure(
		column,
		rusqlite::types::Type::Blob,
		Box::new(io::Error::new(
			io::ErrorKind::InvalidData,
			format!("expected {DIMENSIONS} embedding bytes, found {actual}"),
		)),
	)
}

fn fts_expression(tokens: &[String], separator: &str) -> String {
	tokens
		.iter()
		.map(|token| format!("\"{token}\"*"))
		.collect::<Vec<_>>()
		.join(separator)
}

use std::{
	env,
	io::{self, BufWriter, IsTerminal, Write},
};

use anyhow::Result;
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

use crate::search::{SearchEngine, SearchMode, SearchRequest, SearchResponse};

const RESET: &str = "\x1b[0m";
const AMBER: &str = "\x1b[38;2;245;186;66m";
const BLUE: &str = "\x1b[38;2;110;168;254m";
const MUTED: &str = "\x1b[38;2;130;139;160m";
const BRIGHT: &str = "\x1b[38;2;232;235;242m";

/// Writes one-shot results as stable JSON for scripts and editor integrations.
pub fn json(
	engine: &SearchEngine,
	request: &SearchRequest,
	response: &SearchResponse,
) -> Result<()> {
	let results = response
		.hits
		.iter()
		.map(|hit| {
			let icon = engine.icon(hit.icon);
			let category = engine.category(icon.category);
			JsonHit {
				glyph:         &icon.glyph,
				name:          &icon.name,
				label:         &icon.label,
				category:      &category.slug,
				category_name: &category.name,
				codepoint:     format!("U+{:04X}", icon.codepoint),
				aliases:       icon.aliases.split_ascii_whitespace().collect(),
				score:         rounded_score(hit.score),
			}
		})
		.collect();
	let document = JsonResponse {
		query: &request.query,
		mode: request.mode,
		filters: &request.filters,
		categories: &request.categories,
		excluded_categories: &request.excluded_categories,
		total: response.total,
		count: response.hits.len(),
		results,
	};
	let stdout = io::stdout();
	let mut writer = BufWriter::new(stdout.lock());
	serde_json::to_writer_pretty(&mut writer, &document)?;
	writeln!(writer)?;
	Ok(())
}

/// Writes a compact, aligned list that includes each rendered glyph.
pub fn pretty(
	engine: &SearchEngine,
	request: &SearchRequest,
	response: &SearchResponse,
	no_color: bool,
) -> Result<()> {
	let stdout = io::stdout();
	let color = stdout.is_terminal() && !no_color && env::var_os("NO_COLOR").is_none();
	let mut writer = BufWriter::new(stdout.lock());

	if response.hits.is_empty() {
		writeln!(writer, "No glyphs matched {:?} with {} ranking.", request.query, request.mode)?;
		return Ok(());
	}

	writeln!(
		writer,
		"{}  {} result{} for {}  {} {}{}",
		paint("findnerd", BLUE, color),
		response.hits.len(),
		if response.hits.len() == 1 { "" } else { "s" },
		paint(&format!("“{}”", request.query), BRIGHT, color),
		paint("·", MUTED, color),
		paint(&request.mode.to_string(), AMBER, color),
		if response.total > response.hits.len() {
			format!("  {}", paint(&format!("{} total", response.total), MUTED, color))
		} else {
			String::new()
		}
	)?;
	writeln!(writer)?;

	let name_width = response
		.hits
		.iter()
		.map(|hit| engine.icon(hit.icon).name.len())
		.max()
		.unwrap_or(16)
		.clamp(16, 36);
	let category_width = response
		.hits
		.iter()
		.map(|hit| engine.category(engine.icon(hit.icon).category).name.len())
		.max()
		.unwrap_or(12)
		.clamp(12, 24);

	for hit in &response.hits {
		let icon = engine.icon(hit.icon);
		let category = engine.category(icon.category);
		let glyph = pad(&icon.glyph, 2);
		let name = pad(&truncate(&icon.name, name_width), name_width);
		let category_name = pad(&truncate(&category.name, category_width), category_width);
		writeln!(
			writer,
			"  {}  {}  {}  {}  {:>3.0}%",
			paint(&glyph, AMBER, color),
			paint(&name, BRIGHT, color),
			paint(&category_name, MUTED, color),
			paint(&format!("U+{:04X}", icon.codepoint), BLUE, color),
			hit.score * 100.0
		)?;
	}
	Ok(())
}

/// Writes category metadata in JSON or human-readable form.
pub fn categories(engine: &SearchEngine, as_json: bool, no_color: bool) -> Result<()> {
	let stdout = io::stdout();
	let color = stdout.is_terminal() && !no_color && env::var_os("NO_COLOR").is_none();
	let mut writer = BufWriter::new(stdout.lock());

	if as_json {
		let categories = engine
			.categories()
			.iter()
			.map(|category| JsonCategory {
				slug:       &category.slug,
				name:       &category.name,
				aliases:    category.aliases.split_ascii_whitespace().collect(),
				icon_count: category.icon_count,
			})
			.collect::<Vec<_>>();
		serde_json::to_writer_pretty(&mut writer, &categories)?;
		writeln!(writer)?;
		return Ok(());
	}

	writeln!(writer, "{}  embedded categories", paint("findnerd", BLUE, color))?;
	writeln!(writer)?;
	for category in engine.categories() {
		writeln!(
			writer,
			"  {}  {:<28}  {:>5} glyphs",
			paint(&format!("{:<12}", category.slug), AMBER, color),
			category.name,
			category.icon_count
		)?;
	}
	Ok(())
}

fn paint(text: &str, color: &str, enabled: bool) -> String {
	if enabled {
		format!("{color}{text}{RESET}")
	} else {
		text.to_owned()
	}
}

fn truncate(text: &str, width: usize) -> String {
	if UnicodeWidthStr::width(text) <= width {
		return text.to_owned();
	}
	let target = width.saturating_sub(1);
	let mut result = String::new();
	for character in text.chars() {
		if UnicodeWidthStr::width(result.as_str())
			+ unicode_width::UnicodeWidthChar::width(character).unwrap_or(0)
			> target
		{
			break;
		}
		result.push(character);
	}
	result.push('…');
	result
}

fn pad(text: &str, width: usize) -> String {
	let padding = width.saturating_sub(UnicodeWidthStr::width(text));
	format!("{text}{}", " ".repeat(padding))
}

fn rounded_score(score: f64) -> f64 {
	(score * 1_000_000.0).round() / 1_000_000.0
}

#[derive(Serialize)]
struct JsonResponse<'a> {
	query:               &'a str,
	mode:                SearchMode,
	filters:             &'a [String],
	categories:          &'a [String],
	excluded_categories: &'a [String],
	total:               usize,
	count:               usize,
	results:             Vec<JsonHit<'a>>,
}

#[derive(Serialize)]
struct JsonHit<'a> {
	glyph:         &'a str,
	name:          &'a str,
	label:         &'a str,
	category:      &'a str,
	category_name: &'a str,
	codepoint:     String,
	aliases:       Vec<&'a str>,
	score:         f64,
}

#[derive(Serialize)]
struct JsonCategory<'a> {
	slug:       &'a str,
	name:       &'a str,
	aliases:    Vec<&'a str>,
	icon_count: usize,
}

mod catalog;
mod category_flags;
mod cli;
#[cfg(all(feature = "fetch-model", not(feature = "embed-model")))]
mod model_cache;
mod output;
mod search;
mod semantic;
mod tui;

#[cfg(not(any(feature = "fetch-model", feature = "embed-model")))]
compile_error!("enable the `fetch-model` (default) or `embed-model` feature");

use std::{
	io::{self, IsTerminal, Write},
	process::ExitCode,
};

use anyhow::{Result, bail};
use clap::Parser;

use crate::{
	cli::Cli,
	search::{SearchEngine, SearchRequest},
	tui::TuiOptions,
};

fn main() -> ExitCode {
	match run() {
		Ok(code) => code,
		Err(error) => {
			let _ = writeln!(io::stderr(), "findnerd: {error:#}");
			ExitCode::from(2)
		},
	}
}

fn run() -> Result<ExitCode> {
	let cli = Cli::parse();
	let engine = SearchEngine::open()?;

	if cli.list_categories {
		output::categories(&engine, cli.json, cli.no_color)?;
		return Ok(ExitCode::SUCCESS);
	}

	let query = cli.query_text().unwrap_or_default();
	let mode = cli.mode();
	let categories = cli.included_categories();
	let excluded_categories = cli.excluded_categories();
	if cli.use_interactive() {
		if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
			bail!("interactive mode requires a terminal; pass a query for one-shot output");
		}
		let selection = tui::run(engine, TuiOptions {
			query,
			filters: cli.filter,
			mode,
			categories,
			excluded_categories,
		})?;
		if let Some(selection) = selection {
			println!("{}  {}", selection.glyph, selection.name);
		}
		return Ok(ExitCode::SUCCESS);
	}

	let request = SearchRequest {
		query,
		filters: cli.filter,
		mode,
		categories,
		excluded_categories,
		limit: cli.limit,
	};
	let response = engine.search(&request)?;
	if cli.json {
		output::json(&engine, &request, &response)?;
	} else {
		output::pretty(&engine, &request, &response, cli.no_color)?;
	}

	Ok(if response.hits.is_empty() {
		ExitCode::from(1)
	} else {
		ExitCode::SUCCESS
	})
}

use std::process::{Command, Output};

use serde_json::Value;

fn findnerd(arguments: &[&str]) -> Output {
	Command::new(env!("CARGO_BIN_EXE_findnerd"))
		.args(arguments)
		.output()
		.expect("findnerd should execute")
}

#[test]
fn positional_query_prints_ranked_glyphs() {
	let output = findnerd(&["blank", "--limit", "3", "--no-color"]);

	assert!(output.status.success());
	let stdout = String::from_utf8(output.stdout).expect("output should be UTF-8");
	assert!(stdout.contains("cod-blank"));
	assert!(stdout.contains(''));
	assert!(!stdout.contains("\x1b["), "redirected output must not contain ANSI escapes");
}

#[test]
fn json_mode_combines_query_category_filter_and_strategy() {
	let output = findnerd(&[
		"--query",
		"database",
		"--json",
		"--semantic",
		"--category",
		"cod",
		"--filter",
		"!name:remote",
		"--limit",
		"4",
	]);

	assert!(output.status.success());
	let document: Value = serde_json::from_slice(&output.stdout).expect("output should be JSON");
	assert_eq!(document["query"], "database");
	assert_eq!(document["mode"], "semantic");
	assert_eq!(document["categories"][0], "cod");
	assert_eq!(document["filters"][0], "!name:remote");
	let results = document["results"]
		.as_array()
		.expect("results should be an array");
	assert_eq!(results.len(), 4);
	assert!(results.iter().all(|result| {
		result["category"] == "cod"
			&& result["name"]
				.as_str()
				.is_some_and(|name| !name.contains("remote"))
			&& result["glyph"]
				.as_str()
				.is_some_and(|glyph| !glyph.is_empty())
	}));
}

#[test]
fn each_explicit_ranker_is_available_to_machine_callers() {
	for (flag, expected) in [("--bm25", "bm25"), ("--semantic", "semantic"), ("--match", "match")] {
		let output = findnerd(&["--query", "git branch", flag, "--json", "--limit", "1"]);
		assert!(output.status.success(), "{flag} should succeed");
		let document: Value =
			serde_json::from_slice(&output.stdout).expect("ranker output should be JSON");
		assert_eq!(document["mode"], expected);
		assert_eq!(document["count"], 1);
		assert!(document["results"][0]["name"].is_string());
	}
}

#[test]
fn exact_codepoint_filter_supports_empty_queries() {
	let output = findnerd(&["--query", "", "--filter", "code:eb99", "--json"]);

	assert!(output.status.success());
	let document: Value = serde_json::from_slice(&output.stdout).expect("output should be JSON");
	assert_eq!(document["total"], 1);
	assert_eq!(document["results"][0]["name"], "cod-account");
	assert_eq!(document["results"][0]["codepoint"], "U+EB99");
}

#[test]
fn positive_category_switches_opt_into_only_those_collections() {
	let output = findnerd(&["--query", "view", "--cod", "--json", "--limit", "10"]);

	assert!(output.status.success());
	let document: Value = serde_json::from_slice(&output.stdout).expect("output should be JSON");
	assert_eq!(document["categories"], serde_json::json!(["cod"]));
	assert!(document["results"].as_array().is_some_and(|results| {
		!results.is_empty() && results.iter().all(|result| result["category"] == "cod")
	}));
}

#[test]
fn negative_category_switches_exclude_from_the_full_catalog() {
	let output = findnerd(&["--query", "eye", "--no-fa", "--json", "--limit", "10"]);

	assert!(output.status.success());
	let document: Value = serde_json::from_slice(&output.stdout).expect("output should be JSON");
	assert_eq!(document["categories"], serde_json::json!([]));
	assert_eq!(document["excluded_categories"], serde_json::json!(["fa"]));
	assert!(document["results"].as_array().is_some_and(|results| {
		!results.is_empty() && results.iter().all(|result| result["category"] != "fa")
	}));
}

#[test]
fn no_results_are_valid_json_with_a_distinct_exit_status() {
	let output =
		findnerd(&["--query", "anything", "--filter", "name:this-icon-does-not-exist", "--json"]);

	assert_eq!(output.status.code(), Some(1));
	let document: Value = serde_json::from_slice(&output.stdout).expect("output should be JSON");
	assert_eq!(document["count"], 0);
	assert_eq!(document["results"], Value::Array(Vec::new()));
}

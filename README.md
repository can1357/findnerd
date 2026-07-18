# findnerd

Fast, offline search for Nerd Fonts glyphs from your terminal.

`findnerd` searches a bundled glyph catalog by concept, name, label, alias, category, glyph, or Unicode codepoint. It combines semantic retrieval, SQLite FTS5, and typo-tolerant matching, with both an interactive terminal UI and machine-readable one-shot output. The glyph catalog is embedded in the binary; the semantic model is fetched once on first run (or embedded too, with the `embed-model` feature), after which searches are fully offline.

## Features

- Interactive search with live previews and keyboard or mouse navigation
- Hybrid ranking across semantic, full-text, and fuzzy-name results
- Dedicated semantic, BM25, and fuzzy matching modes
- Filters and category include/exclude controls
- Structured JSON output for scripts
- Fully local catalog; embedding model cached after a one-time download

## Requirements

- A terminal configured with a [Nerd Font](https://www.nerdfonts.com/font-downloads) to render glyphs
- Rust 1.94 or newer when building from source

## Install

From crates.io:

```sh
cargo install findnerd
```

On first run, findnerd downloads the pinned [potion-base-8M](https://huggingface.co/minishlab/potion-base-8M) model (~29 MB, checksum-verified) into the user cache directory and reuses it thereafter.

For a fully offline, self-contained binary, download a prebuilt release from [GitHub Releases](https://github.com/can1357/findnerd/releases), or build with the model embedded from a git checkout:

```sh
git clone https://github.com/can1357/findnerd.git
cd findnerd
cargo install --path . --no-default-features --features embed-model
```

## Usage

Run without a query to open the interactive UI:

```sh
findnerd
```

Pass query words for one-shot output:

```sh
findnerd rust crab
findnerd terminal --semantic --limit 10
findnerd git --category oct --json
```

Force the interactive UI with an initial query:

```sh
findnerd --interactive folder
```

### Interactive controls

| Key | Action |
| --- | --- |
| Type | Edit the search query |
| `Up` / `Down` or mouse | Move through results |
| `Enter` or double-click | Select a glyph and exit |
| `Ctrl-Y` | Copy the selected glyph through OSC 52 |
| `Tab` / `F2` | Cycle ranking mode |
| `Shift-Tab` | Cycle ranking mode backward |
| `F3` / `Ctrl-G` | Cycle category |
| `Esc` | Clear the query, then quit |
| `Ctrl-C` / `Ctrl-Q` | Quit |

### Ranking modes

Hybrid ranking is the default.

| Option | Ranking |
| --- | --- |
| none | Reciprocal-rank fusion over all three strategies |
| `--semantic` | Dense semantic-vector similarity |
| `--bm25` | SQLite FTS5 BM25 over names, labels, and aliases |
| `--match` | Typo-tolerant fuzzy matching over icon names |

### Filters

Repeat `--filter` to combine constraints. Prefix an expression with `!` to negate it.

```sh
findnerd cloud --filter label:rain
findnerd folder --filter '!category:weather'
findnerd --query '' --filter code:U+E7A8
```

Supported fields are `name`, `label`, `alias`, `category`, `glyph`, and `code`. The aliases `aliases`, `cat`, `codepoint`, and `unicode` are also accepted. A filter without a field searches all text metadata.

Restrict a search to one or more category slugs:

```sh
findnerd git --category oct,dev
findnerd git --oct --no-dev
findnerd --list-categories
```

Included categories are combined with OR. Exclusions take precedence.

### JSON output

Use `--json` for structured results containing glyph metadata, category details, codepoints, aliases, and normalized scores:

```sh
findnerd branch --category oct --limit 5 --json
```

One-shot searches exit with status `0` when results are found, `1` when no results match, and `2` on an error.

## Development

```sh
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo run -- rust
```

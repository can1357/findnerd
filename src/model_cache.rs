//! On-demand download of the pinned potion-base-8M encoder.
//!
//! Default builds keep the 29 MB `Model2Vec` weights out of the executable and
//! the crates.io package: the pinned Hugging Face revision is downloaded once
//! into the user cache directory, checksum-verified, and reused thereafter.
//! Build with the `embed-model` feature (from a git checkout) to compile the
//! files into the binary instead and skip this module entirely.

use std::{
	fs,
	io::Read,
	path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

const REPOSITORY: &str = "minishlab/potion-base-8M";
/// Must stay in lockstep with `MODEL_REVISION` in `scripts/build_index.py`;
/// the embedding index in `assets/icons.db` was built with this exact model.
const REVISION: &str = "bf8b056651a2c21b8d2565580b8569da283cab23";

/// SHA-256 pins mirror `MODEL_FILES` in `scripts/build_index.py`, guarding
/// both transport integrity and upstream revision drift.
const FILES: [(&str, &str); 3] = [
	("config.json", "2a6ac0e9aaa356a68a5688070db78fc3a464fefe85d2f06a1905ce3718687553"),
	("tokenizer.json", "e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50"),
	("model.safetensors", "f65d0f325faadc1e121c319e2faa41170d3fa07d8c89abd48ca5358d9a223de2"),
];

/// In-memory contents of the three `Model2Vec` artifacts, in `FILES` order.
pub struct ModelFiles {
	pub config:    Vec<u8>,
	pub tokenizer: Vec<u8>,
	pub model:     Vec<u8>,
}

/// Loads the pinned model from the user cache, downloading it on first use.
///
/// Fails when no cache directory can be resolved, the download fails, or a
/// downloaded file does not match its pinned checksum.
pub fn ensure() -> Result<ModelFiles> {
	let directory = cache_directory()?;
	fs::create_dir_all(&directory)
		.with_context(|| format!("creating model cache at {}", directory.display()))?;

	let mut contents = FILES.map(|(name, checksum)| {
		let path = directory.join(name);
		match fs::read(&path) {
			Ok(bytes) if digest(&bytes) == checksum => Some(bytes),
			// Missing, unreadable, or corrupted: re-download below.
			_ => None,
		}
	});

	if contents.iter().any(Option::is_none) {
		eprintln!(
			"findnerd: downloading {REPOSITORY} model to {} (one-time, ~29 MB)",
			directory.display()
		);
		for (slot, (name, checksum)) in contents.iter_mut().zip(FILES) {
			if slot.is_none() {
				*slot = Some(download(&directory, name, checksum)?);
			}
		}
	}

	let [config, tokenizer, model] = contents.map(|bytes| bytes.expect("all slots filled above"));
	Ok(ModelFiles { config, tokenizer, model })
}

/// Revision-scoped cache directory, e.g. `~/.cache/findnerd/model/bf8b0566`.
fn cache_directory() -> Result<PathBuf> {
	let base = dirs::cache_dir().context(
		"no user cache directory available; install with `cargo install findnerd \
		 --no-default-features --features embed-model` from a git checkout for a self-contained \
		 binary",
	)?;
	Ok(base.join("findnerd").join("model").join(&REVISION[..8]))
}

/// Downloads one file from the pinned revision, verifying its checksum before
/// atomically publishing it into the cache.
fn download(directory: &Path, name: &str, checksum: &str) -> Result<Vec<u8>> {
	let url = format!("https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{name}");
	let mut response = ureq::get(&url)
		.call()
		.with_context(|| format!("downloading {url}"))?;
	let mut bytes = Vec::new();
	response
		.body_mut()
		.as_reader()
		.read_to_end(&mut bytes)
		.with_context(|| format!("reading {url}"))?;
	ensure!(
		digest(&bytes) == checksum,
		"checksum mismatch for {name} from {url}; refusing to cache"
	);

	let temporary = directory.join(format!(".{name}.partial"));
	fs::write(&temporary, &bytes)
		.and_then(|()| fs::rename(&temporary, directory.join(name)))
		.with_context(|| format!("caching {name} in {}", directory.display()))?;
	Ok(bytes)
}

fn digest(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}

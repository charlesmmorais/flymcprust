//! Downloads the pinned schema files of a revision and writes a SHA-256 lock.
//!
//! Downloads use the system `curl` (shipped with Windows 10+, macOS and every
//! CI image) to keep xtask free of TLS dependencies.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::revisions::{Revision, SPEC_FILES};

const RAW_BASE: &str =
    "https://raw.githubusercontent.com/modelcontextprotocol/modelcontextprotocol";

/// Contents of `spec/<revision>/LOCK.toml`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Lock {
    /// Pinned commit the files were fetched from.
    pub(crate) commit: String,
    /// SHA-256 (hex) of each fetched file.
    pub(crate) files: BTreeMap<String, String>,
}

pub(crate) fn run(root: &Path, revision: Revision) -> Result<()> {
    let dir = root.join("spec").join(revision.name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut files = BTreeMap::new();
    for file in SPEC_FILES {
        let url = format!(
            "{RAW_BASE}/{}/schema/{}/{file}",
            revision.commit, revision.name
        );
        let bytes = download(&url)?;
        files.insert((*file).to_owned(), sha256_hex(&bytes));
        let path = dir.join(file);
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        println!("fetched {url}");
    }

    let lock = Lock {
        commit: revision.commit.to_owned(),
        files,
    };
    let text = toml::to_string_pretty(&lock)?;
    std::fs::write(dir.join("LOCK.toml"), text)?;
    println!("wrote {}", dir.join("LOCK.toml").display());
    Ok(())
}

/// Verifies that the files on disk match the lock and the pinned commit.
pub(crate) fn verify(root: &Path, revision: Revision) -> Result<()> {
    let dir = root.join("spec").join(revision.name);
    let lock_path = dir.join("LOCK.toml");
    let lock: Lock = toml::from_str(&std::fs::read_to_string(&lock_path).with_context(|| {
        format!(
            "reading {}; run `cargo xtask fetch-spec {}`",
            lock_path.display(),
            revision.name
        )
    })?)?;
    if lock.commit != revision.commit {
        bail!(
            "{} pins commit {} but xtask expects {}; run `cargo xtask fetch-spec {}`",
            lock_path.display(),
            lock.commit,
            revision.commit,
            revision.name
        );
    }
    for file in SPEC_FILES {
        let path = dir.join(file);
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let actual = sha256_hex(&bytes);
        match lock.files.get(*file) {
            Some(expected) if *expected == actual => {}
            Some(expected) => bail!(
                "{} has SHA-256 {actual}, lock expects {expected}",
                path.display()
            ),
            None => bail!("{} is missing from {}", file, lock_path.display()),
        }
    }
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>> {
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--proto",
            "=https",
        ])
        .arg(url)
        .output()
        .context("running curl (is it installed?)")?;
    if !output.status.success() {
        bail!(
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        // Writing to a String cannot fail.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

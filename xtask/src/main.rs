//! Development tasks for MCPSDKRust.
//!
//! - `cargo xtask fetch-spec <revision>` downloads the pinned schema files of a
//!   spec revision into `spec/<revision>/` and writes a SHA-256 lock.
//! - `cargo xtask codegen [--check]` generates the protocol types of every
//!   known revision into `crates/mcpsdkrust-spec/src/`. With `--check`, it
//!   fails if the committed output is stale instead of writing it.
#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod codegen;
mod fetch;
mod revisions;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage:
  cargo xtask fetch-spec <revision>   download pinned schema files
  cargo xtask codegen [--check]       generate protocol types";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("fetch-spec") => {
            let name = args.get(1).context(USAGE)?;
            let revision = revisions::find(name)?;
            fetch::run(&workspace_root()?, revision)
        }
        Some("codegen") => {
            let check = args.iter().skip(1).any(|a| a == "--check");
            codegen::run(&workspace_root()?, check)
        }
        _ => bail!("{USAGE}"),
    }
}

/// The workspace root is the parent of this crate's manifest directory.
fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(PathBuf::from)
        .context("xtask must live one level below the workspace root")
}

use std::fs;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

fn main() -> Result<()> {
    let pkg = fs::read_to_string("../package.json")?;
    let pkg: Value = serde_json::from_str(&pkg)?;

    let matches_window_title = pkg
        .get("contributes")
        .and_then(|v| v.get("configuration"))
        .and_then(|v| v.get("properties"))
        .and_then(|v| v.get("ctrl-oem3.matches-window-title"))
        .and_then(|v| v.get("default"))
        .and_then(|v| v.as_str())
        .context("Cannot read `ctrl-oem3.matches-window-title` from package.json")?;

    Regex::new(matches_window_title)
        .expect("`ctrl-oem3.matches-window-title` should be valid regular expression");

    println!(
        "cargo::rustc-env=CtrlOEM3_MatchesWindowTitle={}",
        matches_window_title
    );

    Ok(())
}

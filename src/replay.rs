use crate::model::Tick;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Read canonical ticks from an NDJSON replay file (one JSON tick per line).
///
/// This is the deterministic "websocket replay" source: the same bytes yield
/// the same ticks on every run, in Rust and in Python, which is what makes the
/// benchmarks reproducible and the cross-language equivalence test meaningful.
pub fn read_ticks<P: AsRef<Path>>(path: P) -> Result<Vec<Tick>> {
    let path = path.as_ref();
    let file = File::open(path).with_context(|| format!("opening replay file {path:?}"))?;
    let mut ticks = Vec::new();
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let tick: Tick = serde_json::from_str(trimmed)
            .with_context(|| format!("parsing tick on line {}", i + 1))?;
        ticks.push(tick);
    }
    Ok(ticks)
}

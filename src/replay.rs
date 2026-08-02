use crate::model::{BookLevel, Quote, Tick};
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Read newline-delimited JSON records from a replay file (one object per line).
///
/// This is the deterministic "websocket replay" source: the same bytes yield
/// the same records on every run, in Rust and in Python, which is what makes
/// the benchmarks reproducible and the cross-language equivalence test
/// meaningful.
fn read_ndjson<T: DeserializeOwned, P: AsRef<Path>>(path: P) -> Result<Vec<T>> {
    let path = path.as_ref();
    let file = File::open(path).with_context(|| format!("opening replay file {path:?}"))?;
    let mut out = Vec::new();
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: T = serde_json::from_str(trimmed)
            .with_context(|| format!("parsing record on line {}", i + 1))?;
        out.push(record);
    }
    Ok(out)
}

/// Read canonical trade ticks from an NDJSON replay file.
pub fn read_ticks<P: AsRef<Path>>(path: P) -> Result<Vec<Tick>> {
    read_ndjson(path)
}

/// Read canonical top-of-book quotes from an NDJSON replay file.
pub fn read_quotes<P: AsRef<Path>>(path: P) -> Result<Vec<Quote>> {
    read_ndjson(path)
}

/// Read canonical L2 order-book levels from an NDJSON depth replay file.
pub fn read_book<P: AsRef<Path>>(path: P) -> Result<Vec<BookLevel>> {
    read_ndjson(path)
}

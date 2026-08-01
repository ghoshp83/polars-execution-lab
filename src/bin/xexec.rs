use anyhow::{anyhow, Result};
use xexec::execution::{bars, session_twap, session_vwap, summary};
use xexec::quote::quote_metrics;
use xexec::replay::{read_quotes, read_ticks};

fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

const USAGE: &str = "usage: xexec <summary|vwap|twap|bars|book> --input <ndjson> [--bucket-ms N]";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    let input = arg_value(&args, "--input").ok_or_else(|| anyhow!("--input required\n{USAGE}"))?;
    let bucket_ms: i64 = arg_value(&args, "--bucket-ms")
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(1000);
    let bucket_ns = bucket_ms * 1_000_000;

    // `book` reads the top-of-book quote schema, not trades.
    if cmd == "book" {
        let quotes = read_quotes(&input)?;
        if quotes.is_empty() {
            return Err(anyhow!("no quotes in {input}"));
        }
        let product = quotes[0].product.clone();
        println!(
            "{}",
            serde_json::to_string(&quote_metrics(&quotes, &product)?)?
        );
        return Ok(());
    }

    let ticks = read_ticks(&input)?;
    if ticks.is_empty() {
        return Err(anyhow!("no ticks in {input}"));
    }
    let product = ticks[0].product.clone();

    match cmd {
        "summary" => println!(
            "{}",
            serde_json::to_string(&summary(&ticks, &product, bucket_ns)?)?
        ),
        "vwap" => println!("{}", session_vwap(&ticks)?),
        "twap" => println!("{}", session_twap(&ticks)?),
        "bars" => {
            for b in bars(&ticks, bucket_ns)? {
                println!("{}", serde_json::to_string(&b)?);
            }
        }
        other => return Err(anyhow!("unknown command {other:?}\n{USAGE}")),
    }
    Ok(())
}

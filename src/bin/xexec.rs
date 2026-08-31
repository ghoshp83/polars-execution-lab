use anyhow::{anyhow, Result};
use xexec::calibrate::{calibrate_impact, calibrate_impact_robust};
use xexec::curve::sweep_curve;
use xexec::depth::{depth_metrics, queue_metrics};
use xexec::execution::{bars, session_twap, session_vwap, summary};
use xexec::impact::impact_curve;
use xexec::quote::quote_metrics;
use xexec::replay::{read_book, read_calibration, read_impact, read_quotes, read_ticks};
use xexec::sweep::sweep_cost;

fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

const USAGE: &str =
    "usage: xexec <summary|vwap|twap|bars|book|depth|queue|sweep|curve|impact|calibrate> --input <ndjson> [--bucket-ms N] [--side buy|sell] [--size N] [--sizes N,N,N] [--coef-bps N] [--perm-coef-bps N] [--huber-delta N] [--ridge-lambda N] [--max-iters N]";

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

    // `depth` reads the L2 book-level schema, not trades.
    if cmd == "depth" {
        let levels = read_book(&input)?;
        if levels.is_empty() {
            return Err(anyhow!("no book levels in {input}"));
        }
        let product = levels[0].product.clone();
        println!(
            "{}",
            serde_json::to_string(&depth_metrics(&levels, &product)?)?
        );
        return Ok(());
    }

    // `queue` reads the L2 book-level schema and reports top-of-book queue size.
    if cmd == "queue" {
        let levels = read_book(&input)?;
        if levels.is_empty() {
            return Err(anyhow!("no book levels in {input}"));
        }
        let product = levels[0].product.clone();
        println!(
            "{}",
            serde_json::to_string(&queue_metrics(&levels, &product)?)?
        );
        return Ok(());
    }

    // `sweep` reads the L2 book-level schema and prices a marketable order
    // against the resting levels it would have to consume.
    if cmd == "sweep" {
        let side = arg_value(&args, "--side").unwrap_or_else(|| "buy".to_string());
        let size: f64 = arg_value(&args, "--size")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(1.0);
        let levels = read_book(&input)?;
        if levels.is_empty() {
            return Err(anyhow!("no book levels in {input}"));
        }
        let product = levels[0].product.clone();
        println!(
            "{}",
            serde_json::to_string(&sweep_cost(&levels, &product, &side, size)?)?
        );
        return Ok(());
    }

    // `curve` sweeps the book at a ladder of sizes and fits the impact
    // coefficient to the costs the book itself charges.
    if cmd == "curve" {
        let side = arg_value(&args, "--side").unwrap_or_else(|| "buy".to_string());
        let sizes: Vec<f64> = arg_value(&args, "--sizes")
            .unwrap_or_else(|| "0.5,1.0,2.0".to_string())
            .split(',')
            .map(|s| s.trim().parse::<f64>())
            .collect::<Result<_, _>>()?;
        let levels = read_book(&input)?;
        if levels.is_empty() {
            return Err(anyhow!("no book levels in {input}"));
        }
        let product = levels[0].product.clone();
        println!(
            "{}",
            serde_json::to_string(&sweep_curve(&levels, &product, &side, &sizes)?)?
        );
        return Ok(());
    }

    // `impact` reads the execution-schedule slice schema, not trades.
    if cmd == "impact" {
        let coef_bps: f64 = arg_value(&args, "--coef-bps")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(10.0);
        let perm_coef_bps: f64 = arg_value(&args, "--perm-coef-bps")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(0.0);
        let slices = read_impact(&input)?;
        if slices.is_empty() {
            return Err(anyhow!("no impact slices in {input}"));
        }
        let product = slices[0].product.clone();
        println!(
            "{}",
            serde_json::to_string(&impact_curve(&slices, &product, coef_bps, perm_coef_bps)?)?
        );
        return Ok(());
    }

    // `calibrate` reads realised-fill samples and fits the impact coefficients.
    // `--huber-delta` and/or `--ridge-lambda` switch to the robust fit.
    if cmd == "calibrate" {
        let samples = read_calibration(&input)?;
        if samples.is_empty() {
            return Err(anyhow!("no calibration samples in {input}"));
        }
        let product = samples[0].product.clone();
        let huber_delta: Option<f64> = arg_value(&args, "--huber-delta")
            .map(|s| s.parse())
            .transpose()?;
        let ridge_lambda: f64 = arg_value(&args, "--ridge-lambda")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(0.0);
        let max_iters: usize = arg_value(&args, "--max-iters")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(8);
        let summary = if huber_delta.is_some() || ridge_lambda != 0.0 {
            calibrate_impact_robust(&samples, &product, huber_delta, ridge_lambda, max_iters)?
        } else {
            calibrate_impact(&samples, &product)?
        };
        println!("{}", serde_json::to_string(&summary)?);
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

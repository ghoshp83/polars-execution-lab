use crate::model::ImpactSlice;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' impact curves compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// Session market-impact cost curve over a participation schedule.
///
/// Each slice's temporary impact follows the square-root (Almgren-Chriss-style)
/// law `impact_bps = coef_bps * sqrt(participation)`, where `participation` is
/// the fraction of the available volume the slice consumes and `coef_bps` is the
/// calibration constant -- the impact, in basis points, of taking the entire
/// available volume in one slice, fit from a desk's own fills. The curve is
/// concave: the marginal cost of size falls as participation grows, which is the
/// behaviour a linear model misses. It summarises the schedule as:
/// - `avg_impact_bps` -- mean per-slice impact.
/// - `max_impact_bps` -- the costliest slice (the largest participation).
/// - `total_impact_bps` -- summed impact across the schedule.
#[derive(Debug, Serialize)]
pub struct ImpactSummary {
    pub product: String,
    pub slices: usize,
    pub coef_bps: f64,
    pub avg_impact_bps: f64,
    pub max_impact_bps: f64,
    pub total_impact_bps: f64,
}

/// Compute the session impact curve from participation slices with the Polars
/// engine. Mirrors `impact_curve` in `python/xexeclab/engine.py` expression for
/// expression, including the sort that fixes the summation order so the
/// cross-language means and totals are bit-for-bit identical.
pub fn impact_curve(slices: &[ImpactSlice], product: &str, coef_bps: f64) -> Result<ImpactSummary> {
    if slices.is_empty() {
        return Err(anyhow!("no impact slices"));
    }
    let ts_ns: Vec<i64> = slices.iter().map(|s| s.ts_ns).collect();
    let participation: Vec<f64> = slices.iter().map(|s| s.participation).collect();

    let out = df!(
        "ts_ns" => ts_ns,
        "participation" => participation,
    )?
    .lazy()
    .sort_by_exprs([col("ts_ns")], SortMultipleOptions::default())
    .with_columns([(col("participation").sqrt() * lit(coef_bps)).alias("impact_bps")])
    .select([
        col("impact_bps").mean().alias("avg_impact_bps"),
        col("impact_bps").max().alias("max_impact_bps"),
        col("impact_bps").sum().alias("total_impact_bps"),
    ])
    .collect()?;

    let f = |name: &str| -> Result<f64> {
        out.column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    Ok(ImpactSummary {
        product: product.to_string(),
        slices: slices.len(),
        coef_bps: r8(coef_bps),
        avg_impact_bps: r8(f("avg_impact_bps")?),
        max_impact_bps: r8(f("max_impact_bps")?),
        total_impact_bps: r8(f("total_impact_bps")?),
    })
}

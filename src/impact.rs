use crate::model::ImpactSlice;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' impact curves compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// Session market-impact cost curve over a participation schedule, priced under
/// the two-term Almgren-Chriss model.
///
/// Each slice pays two costs. The **temporary** impact follows the concave
/// square-root law `temp_bps = coef_bps * sqrt(participation)` -- a transient
/// cost that dissipates after the slice, where `coef_bps` is the impact in basis
/// points of taking the entire available volume at once. The **permanent**
/// impact is a lasting shift of the mid, linear in size,
/// `perm_bps = perm_coef_bps * participation` -- the price move the schedule
/// leaves behind for everyone, which does *not* recover. Both coefficients are
/// calibration constants (fit externally from a desk's own fills). The summary
/// separates the two so a schedule's transient and lasting costs are visible:
/// - `avg_impact_bps` / `max_impact_bps` / `total_impact_bps` -- the temporary
///   (square-root) cost: mean, costliest slice, and sum.
/// - `avg_perm_impact_bps` / `total_perm_impact_bps` -- the permanent (linear)
///   cost: mean per slice and sum.
/// - `total_cost_bps` -- the full round-trip cost, temporary + permanent.
///
/// With `perm_coef_bps == 0` the permanent fields are zero and `total_cost_bps`
/// collapses to `total_impact_bps`, so the pure square-root curve is the default.
#[derive(Debug, Serialize)]
pub struct ImpactSummary {
    pub product: String,
    pub slices: usize,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    pub avg_impact_bps: f64,
    pub max_impact_bps: f64,
    pub total_impact_bps: f64,
    pub avg_perm_impact_bps: f64,
    pub total_perm_impact_bps: f64,
    pub total_cost_bps: f64,
}

/// Compute the session impact curve from participation slices with the Polars
/// engine. Mirrors `impact_curve` in `python/xexeclab/engine.py` expression for
/// expression, including the sort that fixes the summation order so the
/// cross-language means and totals are bit-for-bit identical.
pub fn impact_curve(
    slices: &[ImpactSlice],
    product: &str,
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<ImpactSummary> {
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
    .with_columns([
        (col("participation").sqrt() * lit(coef_bps)).alias("impact_bps"),
        (col("participation") * lit(perm_coef_bps)).alias("perm_impact_bps"),
    ])
    .select([
        col("impact_bps").mean().alias("avg_impact_bps"),
        col("impact_bps").max().alias("max_impact_bps"),
        col("impact_bps").sum().alias("total_impact_bps"),
        col("perm_impact_bps").mean().alias("avg_perm_impact_bps"),
        col("perm_impact_bps").sum().alias("total_perm_impact_bps"),
    ])
    .collect()?;

    let f = |name: &str| -> Result<f64> {
        out.column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    let total_impact = f("total_impact_bps")?;
    let total_perm = f("total_perm_impact_bps")?;
    Ok(ImpactSummary {
        product: product.to_string(),
        slices: slices.len(),
        coef_bps: r8(coef_bps),
        perm_coef_bps: r8(perm_coef_bps),
        avg_impact_bps: r8(f("avg_impact_bps")?),
        max_impact_bps: r8(f("max_impact_bps")?),
        total_impact_bps: r8(total_impact),
        avg_perm_impact_bps: r8(f("avg_perm_impact_bps")?),
        total_perm_impact_bps: r8(total_perm),
        total_cost_bps: r8(total_impact + total_perm),
    })
}

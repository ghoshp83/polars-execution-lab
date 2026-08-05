use crate::model::CalibrationSample;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' calibration fits compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// Impact coefficients recovered from a desk's own realised fills.
///
/// `impact_curve` takes the two Almgren-Chriss coefficients as inputs; this is
/// the other half -- where they come from. Given realised fills, each tagged
/// with the fraction of volume it took and the cost it actually paid, the fit
/// regresses cost back onto the model's own basis functions and reports the
/// coefficients plus how well the model explains the observed costs:
/// - `coef_bps` -- the fitted **temporary** (square-root) coefficient.
/// - `perm_coef_bps` -- the fitted **permanent** (linear) coefficient.
/// - `rmse_bps` -- root-mean-square residual, the typical bps the fit misses by.
/// - `r_squared` -- fraction of the cost variance the two terms explain, in
///   `(-inf, 1]` (1 = perfect fit).
#[derive(Debug, Serialize)]
pub struct CalibrationSummary {
    pub product: String,
    pub samples: usize,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    pub rmse_bps: f64,
    pub r_squared: f64,
}

/// Fit the two-term Almgren-Chriss coefficients from realised fills.
///
/// The impact model says a fill's cost is `coef_bps * sqrt(participation) +
/// perm_coef_bps * participation`. Treating `sqrt(participation)` and
/// `participation` as two regressors and the realised cost as the response, the
/// coefficients are the ordinary-least-squares fit through the origin (a fill of
/// zero size costs nothing, so there is no intercept). Every quantity the normal
/// equations need is a sum, so the whole fit is a Polars aggregation followed by
/// a 2x2 solve -- and it mirrors `calibrate_impact` in
/// `python/xexeclab/engine.py` sum for sum and operation for operation, so the
/// two engines recover bit-for-bit identical coefficients. The `.sort("ts_ns")`
/// fixes the summation order that makes that identity hold.
pub fn calibrate_impact(
    samples: &[CalibrationSample],
    product: &str,
) -> Result<CalibrationSummary> {
    if samples.is_empty() {
        return Err(anyhow!("no calibration samples"));
    }
    let ts_ns: Vec<i64> = samples.iter().map(|s| s.ts_ns).collect();
    let participation: Vec<f64> = samples.iter().map(|s| s.participation).collect();
    let realised_bps: Vec<f64> = samples.iter().map(|s| s.realised_bps).collect();

    // Sufficient statistics for the origin-through OLS of `y` on the two
    // regressors x1 = sqrt(participation) and x2 = participation. Because
    // x1^2 == participation, S11 is just the sum of participation.
    let sums = df!(
        "ts_ns" => ts_ns,
        "participation" => participation,
        "realised_bps" => realised_bps,
    )?
    .lazy()
    .sort_by_exprs([col("ts_ns")], SortMultipleOptions::default())
    .with_columns([col("participation").sqrt().alias("x1")])
    .select([
        col("participation").sum().alias("s11"),
        (col("x1") * col("participation")).sum().alias("s12"),
        (col("participation") * col("participation"))
            .sum()
            .alias("s22"),
        (col("x1") * col("realised_bps")).sum().alias("b1"),
        (col("participation") * col("realised_bps"))
            .sum()
            .alias("b2"),
        (col("realised_bps") * col("realised_bps"))
            .sum()
            .alias("syy"),
        col("realised_bps").sum().alias("sy"),
    ])
    .collect()?;

    let g = |name: &str| -> Result<f64> {
        sums.column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    let s11 = g("s11")?;
    let s12 = g("s12")?;
    let s22 = g("s22")?;
    let b1 = g("b1")?;
    let b2 = g("b2")?;
    let syy = g("syy")?;
    let sy = g("sy")?;
    let n = samples.len() as f64;

    // Solve [S11 S12; S12 S22] [coef; perm] = [b1; b2]. A design that is
    // (near-)singular -- one distinct participation level cannot separate the
    // two basis functions -- has no unique fit, so refuse rather than emit a
    // blown-up coefficient.
    let scale = s11 * s22;
    let det = scale - s12 * s12;
    if det.abs() <= 1e-12 * scale {
        return Err(anyhow!(
            "singular design: need at least two distinct participation levels to fit both terms"
        ));
    }
    let coef = (s22 * b1 - s12 * b2) / det;
    let perm = (s11 * b2 - s12 * b1) / det;

    // Residual and explained variance, both from the sufficient statistics:
    // SS_res = Sum (y - yhat)^2 expanded onto the sums above.
    let ss_res_raw = syy - 2.0 * coef * b1 - 2.0 * perm * b2
        + coef * coef * s11
        + 2.0 * coef * perm * s12
        + perm * perm * s22;
    let ss_res = if ss_res_raw > 0.0 { ss_res_raw } else { 0.0 };
    let ybar = sy / n;
    let ss_tot = syy - n * ybar * ybar;
    let rmse = (ss_res / n).sqrt();
    let r_squared = if ss_tot == 0.0 {
        0.0
    } else {
        1.0 - ss_res / ss_tot
    };

    Ok(CalibrationSummary {
        product: product.to_string(),
        samples: samples.len(),
        coef_bps: r8(coef),
        perm_coef_bps: r8(perm),
        rmse_bps: r8(rmse),
        r_squared: r8(r_squared),
    })
}

use crate::hlsweep::hl_sweep;
use crate::model::Tick;
use anyhow::{anyhow, Result};
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' grids compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// One cap's row of the grid: the half-life sweep at that cap, reduced to the
/// figures that say whether its verdict survived the half-life.
#[derive(Debug, Serialize)]
pub struct StabilityRow {
    pub cap: f64,
    /// `improvement_bps` at each of the report's `half_lives`, in that order.
    pub improvement_bps: Vec<f64>,
    /// `improvement_bps` at `half_life = 0`, the equal-weight pool.
    pub flat_improvement_bps: f64,
    pub improvement_span_bps: f64,
    /// False when this cap's row holds both a gain and a loss.
    pub sign_stable: bool,
}

/// **Does the cap ladder survive the half-life?**
///
/// [`crate::capsweep`] fixes the half-life and sweeps the cap;
/// [`crate::hlsweep`] fixes the cap and sweeps the half-life. A reader who wants
/// to know whether a verdict read off the cap ladder would hold at a different
/// half-life has had to run one half-life sweep per cap by hand. This runs them
/// all, one row per cap, every row exactly the [`crate::hlsweep::HlSweepReport`]
/// that `pov-hl-sweep` would print at that cap.
///
/// The question is the one `sign_stable` asks, asked of every cap at once:
/// `stable_caps` counts the rows on which pooling paid at every half-life or
/// cost at every half-life, `unstable_caps` the rows on which the half-life
/// decided. `all_sign_stable` is the one-word answer to the heading.
///
/// **There is deliberately no best cell.** A two-parameter grid scored on the
/// session it is fitted to is the place a report stops describing and starts
/// fitting: pick the cell with the largest `improvement_bps` and you have chosen
/// a cap *and* a half-life with the answer already in hand, with twice the
/// freedom [`crate::hlsweep::HlSweepReport::best_half_life`] already warns
/// about. That field is a one-dimensional shape; its two-dimensional twin would
/// read as a recommendation however it was documented, so it is not computed.
/// A test pins its absence.
///
/// Like both sweeps it takes no `shortfall_bps`, and every cell is the `capped`
/// (forward-carry) arm.
#[derive(Debug, Serialize)]
pub struct StabilityReport {
    pub product: String,
    pub bucket_ns: i64,
    pub buckets: usize,
    /// History sessions pooled into the forecast.
    pub sessions: usize,
    pub parent_qty: f64,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    /// The half-life grid every row is swept across, in order.
    pub half_lives: Vec<f64>,
    pub rows: Vec<StabilityRow>,
    pub stable_caps: usize,
    pub unstable_caps: usize,
    pub all_sign_stable: bool,
}

/// Run [`hl_sweep`] at every cap on a grid and line the rows up.
///
/// Mirrors `stability` in `python/xexeclab/engine.py` operation for operation,
/// so the two engines report bit-for-bit identical grids.
#[allow(clippy::too_many_arguments)]
pub fn stability(
    history: &[Vec<Tick>],
    exec: &[Tick],
    bucket_ns: i64,
    parent_qty: f64,
    cap_grid: &[f64],
    hl_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<StabilityReport> {
    // The same rules as `cap_sweep`, so a grid one command accepts the other
    // does too. The half-life grid is validated by `hl_sweep` itself.
    if cap_grid.len() < 2 {
        return Err(anyhow!(
            "cap_grid needs at least 2 points, got {}",
            cap_grid.len()
        ));
    }
    for v in cap_grid {
        if !v.is_finite() || *v <= 0.0 || *v > 1.0 {
            return Err(anyhow!("cap_grid values must be in (0, 1], got {v}"));
        }
    }
    for w in cap_grid.windows(2) {
        if w[1] <= w[0] {
            return Err(anyhow!(
                "cap_grid must be strictly increasing, got {} then {}",
                w[0],
                w[1]
            ));
        }
    }

    let mut rows: Vec<StabilityRow> = Vec::with_capacity(cap_grid.len());
    let mut product = String::new();
    let mut buckets = 0usize;
    let mut sessions = 0usize;
    for c in cap_grid {
        let s = hl_sweep(
            history,
            exec,
            bucket_ns,
            parent_qty,
            *c,
            hl_grid,
            coef_bps,
            perm_coef_bps,
        )?;
        product = s.product;
        buckets = s.buckets;
        sessions = s.sessions;
        rows.push(StabilityRow {
            cap: s.cap,
            improvement_bps: s.points.iter().map(|p| p.improvement_bps).collect(),
            flat_improvement_bps: s.flat_improvement_bps,
            improvement_span_bps: s.improvement_span_bps,
            sign_stable: s.sign_stable,
        });
    }

    let stable_caps = rows.iter().filter(|r| r.sign_stable).count();
    Ok(StabilityReport {
        product,
        bucket_ns,
        buckets,
        sessions,
        parent_qty: r8(parent_qty),
        coef_bps: r8(coef_bps),
        perm_coef_bps: r8(perm_coef_bps),
        half_lives: hl_grid.iter().map(|h| r8(*h)).collect(),
        unstable_caps: rows.len() - stable_caps,
        all_sign_stable: stable_caps == rows.len(),
        stable_caps,
        rows,
    })
}

use crate::model::Tick;
use crate::stability::{stability, StabilityRow};
use anyhow::{anyhow, Result};
use serde::Serialize;

/// What one cap's row says once its direction is read as well as its stability.
///
/// `sign_stable` alone cannot tell a row on which pooling paid at every
/// half-life from one on which it cost at every half-life: both are "stable".
/// Across folds that difference is the whole question, so the verdict names it.
fn verdict(row: &StabilityRow) -> &'static str {
    if row.inert {
        return "inert";
    }
    if !row.sign_stable {
        return "unstable";
    }
    if row.improvement_bps.iter().any(|v| *v > 0.0) {
        return "gain";
    }
    if row.improvement_bps.iter().any(|v| *v < 0.0) {
        return "loss";
    }
    // Every grid point is zero but the row is not inert, so the flat pool is
    // the only figure that moved and it carries the direction.
    if row.flat_improvement_bps > 0.0 {
        "gain"
    } else {
        "loss"
    }
}

/// One session's turn as the held-out one: the stability grid run with it as
/// the input and every other session, in the order given, as the history.
#[derive(Debug, Serialize)]
pub struct HoldoutFold {
    /// Position of the held-out session in the order the sessions were given.
    pub held_out: usize,
    /// One of `inert`, `gain`, `loss` or `unstable` per cap, in `caps` order.
    pub verdicts: Vec<String>,
    /// `improvement_bps` at `half_life = 0` per cap, in `caps` order.
    pub flat_improvement_bps: Vec<f64>,
    pub inert_caps: usize,
    pub stable_caps: usize,
    pub unstable_caps: usize,
}

/// **Does the verdict survive the choice of held-out session?**
///
/// [`crate::stability`] asks whether a cap's verdict survives the half-life,
/// but it asks it of one input session. Which session is held out is itself a
/// choice nobody fitted, and the grid has never been run with a different one.
/// This runs it once per session: each takes a turn as the input while the
/// others, in the order given, are the history. The fold that holds out the
/// last session is exactly the report `pov-stability` prints for the same
/// arguments.
///
/// Each fold reduces every cap to one verdict -- `inert`, `gain`, `loss` or
/// `unstable` -- because two folds that are both sign-stable can still
/// disagree on which sign. `consensus` is the verdict a cap was given in
/// every fold, or `mixed` when the folds disagree; `agreeing_caps` counts the
/// caps that are not `mixed`, and `all_agree` is the one-word answer. A
/// consensus of `inert` is agreement that nothing was measured, which is why
/// the verdict is reported and not just a flag.
///
/// **This is a rotation, not a backtest.** A fold that holds out an early
/// session forecasts it from sessions that came after it, which no desk could
/// have done. The rotation says how much the verdict depends on the session it
/// was read from; it does not say what any of these plans would have earned.
///
/// **There is deliberately no pooled figure and no best fold.** Averaging
/// `improvement_bps` across folds would turn a handful of sessions into one
/// number that reads as an estimate, and picking the fold with the kindest
/// verdict is the fitting [`crate::stability::StabilityReport`] already
/// refuses. A test pins the absence of both.
#[derive(Debug, Serialize)]
pub struct HoldoutReport {
    pub product: String,
    pub bucket_ns: i64,
    pub buckets: usize,
    /// Sessions rotated through, so each fold pools `sessions - 1` of them.
    pub sessions: usize,
    pub parent_qty: f64,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    pub caps: Vec<f64>,
    pub half_lives: Vec<f64>,
    pub folds: Vec<HoldoutFold>,
    /// Per cap, in `caps` order: the verdict every fold gave it, or `mixed`.
    pub consensus: Vec<String>,
    pub agreeing_caps: usize,
    pub all_agree: bool,
}

/// Run [`stability`] once per session, holding that session out as the input.
///
/// Mirrors `holdout` in `python/xexeclab/engine.py` operation for operation,
/// so the two engines report bit-for-bit identical rotations.
pub fn holdout(
    sessions: &[Vec<Tick>],
    bucket_ns: i64,
    parent_qty: f64,
    cap_grid: &[f64],
    hl_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<HoldoutReport> {
    // Each fold pools the other sessions, and a pool of one is not a pool.
    if sessions.len() < 3 {
        return Err(anyhow!(
            "holdout needs at least 3 sessions, got {}",
            sessions.len()
        ));
    }

    let mut folds: Vec<HoldoutFold> = Vec::with_capacity(sessions.len());
    let mut product = String::new();
    let mut buckets = 0usize;
    let mut parent = 0.0;
    let mut coef = 0.0;
    let mut perm = 0.0;
    let mut caps: Vec<f64> = Vec::new();
    let mut half_lives: Vec<f64> = Vec::new();
    for held_out in 0..sessions.len() {
        let history: Vec<Vec<Tick>> = sessions
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != held_out)
            .map(|(_, s)| s.clone())
            .collect();
        let grid = stability(
            &history,
            &sessions[held_out],
            bucket_ns,
            parent_qty,
            cap_grid,
            hl_grid,
            coef_bps,
            perm_coef_bps,
        )?;
        folds.push(HoldoutFold {
            held_out,
            verdicts: grid.rows.iter().map(|r| verdict(r).to_string()).collect(),
            flat_improvement_bps: grid.rows.iter().map(|r| r.flat_improvement_bps).collect(),
            inert_caps: grid.inert_caps,
            stable_caps: grid.stable_caps,
            unstable_caps: grid.unstable_caps,
        });
        product = grid.product;
        buckets = grid.buckets;
        parent = grid.parent_qty;
        coef = grid.coef_bps;
        perm = grid.perm_coef_bps;
        caps = grid.rows.iter().map(|r| r.cap).collect();
        half_lives = grid.half_lives;
    }

    let consensus: Vec<String> = (0..caps.len())
        .map(|c| {
            if folds.iter().all(|f| f.verdicts[c] == folds[0].verdicts[c]) {
                folds[0].verdicts[c].clone()
            } else {
                "mixed".to_string()
            }
        })
        .collect();
    let agreeing_caps = consensus.iter().filter(|v| *v != "mixed").count();
    Ok(HoldoutReport {
        product,
        bucket_ns,
        buckets,
        sessions: sessions.len(),
        parent_qty: parent,
        coef_bps: coef,
        perm_coef_bps: perm,
        caps,
        half_lives,
        folds,
        all_agree: agreeing_caps == consensus.len(),
        consensus,
        agreeing_caps,
    })
}

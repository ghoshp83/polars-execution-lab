use crate::holdout::verdict;
use crate::model::Tick;
use crate::stability::stability;
use anyhow::{anyhow, Result};
use serde::Serialize;

/// One step of the walk: the stability grid run with session `input` as the
/// input and only the sessions before it, oldest first, as the history.
#[derive(Debug, Serialize)]
pub struct WalkForwardFold {
    /// Position of the input session.
    pub input: usize,
    /// Sessions `history_from..input` are its history: `0` on an expanding
    /// walk, `input - window` on a rolling one.
    pub history_from: usize,
    /// One of `inert`, `gain`, `loss` or `unstable` per cap, in `caps` order.
    pub verdicts: Vec<String>,
    /// `improvement_bps` at `half_life = 0` per cap, in `caps` order.
    pub flat_improvement_bps: Vec<f64>,
    pub inert_caps: usize,
    pub stable_caps: usize,
    pub unstable_caps: usize,
}

/// **Does the verdict hold when each session is forecast only from its past?**
///
/// [`crate::holdout`] rotates the held-out session, and says of itself that it
/// is not a backtest: a fold that holds out an early session forecasts it from
/// sessions that came after it. This is the version a desk could have run.
/// Session `i` is the input and sessions `0..i` are the history, for every `i`
/// from 2 up, so no fold reads a session later than the one it scores. The
/// sessions must be given oldest first and must not overlap in time, and that
/// is checked against their timestamps instead of taken on trust.
///
/// The last fold is the report `pov-stability` prints for the same arguments,
/// and the last fold of [`crate::holdout`] on the same sessions. Every earlier
/// fold differs from the rotation's, because the rotation hands it the future.
///
/// **The folds are not like for like.** Each one pools one more session than
/// the fold before it, so a verdict that changes along the walk may be the
/// history growing and not the input differing. `consensus` is still the
/// verdict every fold gave a cap, or `mixed`; it says whether the reading was
/// the same at every step, not why it was not.
///
/// **A window makes them so, at a price.** With `window` set, every fold pools
/// exactly that many sessions, the ones immediately before its input, so two
/// folds differ in which sessions they read and not in how many. What it costs
/// is the oldest sessions, which later folds no longer see, and one fold per
/// session the window is wider than two. Nothing here chooses a window: the
/// two walks are two questions, and a cap they read differently is a cap whose
/// verdict depended on how far back the history went.
///
/// **There is deliberately no pooled figure and no best fold**, for the reason
/// [`crate::holdout::HoldoutReport`] gives. With `n` sessions there are only
/// `n - 2` folds, or `n - window` on a rolling walk, which is a count to read
/// beside the consensus and not a sample to average over.
#[derive(Debug, Serialize)]
pub struct WalkForwardReport {
    pub product: String,
    pub bucket_ns: i64,
    pub buckets: usize,
    /// Sessions walked through; the first two, or the first `window`, are only
    /// ever history.
    pub sessions: usize,
    /// Sessions of history every fold pools, or `null` when the history grows
    /// by one each step.
    pub window: Option<usize>,
    pub parent_qty: f64,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    pub caps: Vec<f64>,
    pub half_lives: Vec<f64>,
    pub folds: Vec<WalkForwardFold>,
    /// Per cap, in `caps` order: the verdict every fold gave it, or `mixed`.
    pub consensus: Vec<String>,
    pub agreeing_caps: usize,
    pub all_agree: bool,
}

/// Run [`stability`] on each session from the third on, with only the earlier
/// sessions as its history -- all of them, or the last `window` of them.
///
/// Mirrors `walk_forward` in `python/xexeclab/engine.py` operation for
/// operation, so the two engines report bit-for-bit identical walks.
#[allow(clippy::too_many_arguments)]
pub fn walk_forward(
    sessions: &[Vec<Tick>],
    bucket_ns: i64,
    parent_qty: f64,
    cap_grid: &[f64],
    hl_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
    window: Option<usize>,
) -> Result<WalkForwardReport> {
    // A pool of one is not a pool, so the first fold is session 2; and one
    // fold has nothing to agree with, so the walk needs a second.
    if sessions.len() < 4 {
        return Err(anyhow!(
            "walk-forward needs at least 4 sessions, got {}",
            sessions.len()
        ));
    }
    if let Some(w) = window {
        if w < 2 {
            return Err(anyhow!(
                "walk-forward window must be at least 2 sessions, got {w}"
            ));
        }
        if sessions.len() < w + 2 {
            return Err(anyhow!(
                "walk-forward with a window of {w} needs at least {} sessions, got {}",
                w + 2,
                sessions.len()
            ));
        }
    }
    let mut prev_end = i64::MIN;
    for (i, s) in sessions.iter().enumerate() {
        let start = s.iter().map(|t| t.ts_ns).min();
        let end = s.iter().map(|t| t.ts_ns).max();
        let (Some(start), Some(end)) = (start, end) else {
            return Err(anyhow!("walk-forward session {i} is empty"));
        };
        if i > 0 && start <= prev_end {
            return Err(anyhow!(
                "walk-forward sessions must be in time order: session {i} starts at or before session {} ends",
                i - 1
            ));
        }
        prev_end = end;
    }

    let mut folds: Vec<WalkForwardFold> = Vec::with_capacity(sessions.len() - 2);
    let mut product = String::new();
    let mut buckets = 0usize;
    let mut parent = 0.0;
    let mut coef = 0.0;
    let mut perm = 0.0;
    let mut caps: Vec<f64> = Vec::new();
    let mut half_lives: Vec<f64> = Vec::new();
    for input in window.unwrap_or(2)..sessions.len() {
        let history_from = window.map_or(0, |w| input - w);
        let grid = stability(
            &sessions[history_from..input],
            &sessions[input],
            bucket_ns,
            parent_qty,
            cap_grid,
            hl_grid,
            coef_bps,
            perm_coef_bps,
        )?;
        folds.push(WalkForwardFold {
            input,
            history_from,
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
    Ok(WalkForwardReport {
        product,
        bucket_ns,
        buckets,
        sessions: sessions.len(),
        window,
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

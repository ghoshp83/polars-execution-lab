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

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' sweeps compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// One walk of the sweep, cut down to what the walks can be compared on.
#[derive(Debug, Serialize)]
pub struct WindowWalk {
    /// Sessions of history every fold pooled, or `null` for the growing walk.
    pub window: Option<usize>,
    /// How many folds stand behind `consensus`: fewer as the window widens.
    pub folds: usize,
    /// Per cap, in `caps` order: the verdict every fold gave it, or `mixed`.
    pub consensus: Vec<String>,
    pub agreeing_caps: usize,
    /// The newest session's verdict per cap -- the one fold every walk has.
    pub last_verdicts: Vec<String>,
    /// The newest session's `flat_improvement_bps` per cap.
    pub last_flat_improvement_bps: Vec<f64>,
}

/// **Does the walk's answer depend on how far back its history goes?**
///
/// [`walk_forward`] takes a window and says nothing chooses one. That leaves
/// the window where the half-life was before `pov-hl-sweep`: a parameter
/// nobody fits, quoted at whichever value was typed. This runs the walk at
/// every window the sessions allow -- the growing history first, then `2` up
/// to `sessions - 2` -- and reports what moved.
///
/// Two things are compared, because the walks share only so much. A walk's
/// `consensus` rests on its own folds, and a wider window has fewer of them, so
/// `consensus_stable` says a cap read the same along every walk without the
/// walks having scored the same sessions -- and `mixed` along every walk is the
/// same reading too, which is why `settled_caps` counts only the caps that
/// matched on a verdict. The newest session is the one input
/// every walk scores, so `last_fold_stable` and `last_fold_span_bps` compare
/// like with like: the same session, forecast from histories of different
/// depth. A span beside an unchanged verdict is a size that depended on the
/// window under a sign that did not.
///
/// **There is deliberately no best window.** The walks are listed in the order
/// they were run, and nothing here ranks them: a window picked for the verdict
/// it gives is a verdict picked.
#[derive(Debug, Serialize)]
pub struct WindowSweepReport {
    pub product: String,
    pub bucket_ns: i64,
    pub buckets: usize,
    pub sessions: usize,
    pub parent_qty: f64,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    pub caps: Vec<f64>,
    pub half_lives: Vec<f64>,
    /// The growing walk, then one walk per window from 2 up.
    pub walks: Vec<WindowWalk>,
    /// Per cap: true when every walk's `consensus` is the same verdict.
    pub consensus_stable: Vec<bool>,
    /// Per cap: true when every walk gave the newest session the same verdict.
    pub last_fold_stable: Vec<bool>,
    /// Per cap: the widest gap between two walks' `flat_improvement_bps` on
    /// the newest session.
    pub last_fold_span_bps: Vec<f64>,
    /// Caps every walk gave the same `consensus`, and a verdict at that: two
    /// walks that are both `mixed` match without either having settled.
    pub settled_caps: usize,
    pub all_settled: bool,
}

/// Run [`walk_forward`] with a growing history and then at every window from 2
/// to `sessions - 2`.
///
/// Mirrors `window_sweep` in `python/xexeclab/engine.py` operation for
/// operation, so the two engines report bit-for-bit identical sweeps.
pub fn window_sweep(
    sessions: &[Vec<Tick>],
    bucket_ns: i64,
    parent_qty: f64,
    cap_grid: &[f64],
    hl_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<WindowSweepReport> {
    let mut windows: Vec<Option<usize>> = vec![None];
    windows.extend((2..=sessions.len().saturating_sub(2)).map(Some));
    let mut reports: Vec<WalkForwardReport> = Vec::with_capacity(windows.len());
    for window in windows {
        reports.push(walk_forward(
            sessions,
            bucket_ns,
            parent_qty,
            cap_grid,
            hl_grid,
            coef_bps,
            perm_coef_bps,
            window,
        )?);
    }
    let walks: Vec<WindowWalk> = reports
        .iter()
        .map(|r| {
            // `walk_forward` refuses fewer than two folds, so there is a last.
            let last = &r.folds[r.folds.len() - 1];
            WindowWalk {
                window: r.window,
                folds: r.folds.len(),
                consensus: r.consensus.clone(),
                agreeing_caps: r.agreeing_caps,
                last_verdicts: last.verdicts.clone(),
                last_flat_improvement_bps: last.flat_improvement_bps.clone(),
            }
        })
        .collect();

    let first = &reports[0];
    let caps = first.caps.len();
    let consensus_stable: Vec<bool> = (0..caps)
        .map(|c| {
            walks
                .iter()
                .all(|w| w.consensus[c] == walks[0].consensus[c])
        })
        .collect();
    let last_fold_stable: Vec<bool> = (0..caps)
        .map(|c| {
            walks
                .iter()
                .all(|w| w.last_verdicts[c] == walks[0].last_verdicts[c])
        })
        .collect();
    let last_fold_span_bps: Vec<f64> = (0..caps)
        .map(|c| {
            let mut lo = walks[0].last_flat_improvement_bps[c];
            let mut hi = lo;
            for w in &walks[1..] {
                let v = w.last_flat_improvement_bps[c];
                if v < lo {
                    lo = v;
                }
                if v > hi {
                    hi = v;
                }
            }
            r8(hi - lo)
        })
        .collect();
    let settled_caps = (0..caps)
        .filter(|c| consensus_stable[*c] && walks[0].consensus[*c] != "mixed")
        .count();
    Ok(WindowSweepReport {
        product: first.product.clone(),
        bucket_ns,
        buckets: first.buckets,
        sessions: sessions.len(),
        parent_qty: first.parent_qty,
        coef_bps: first.coef_bps,
        perm_coef_bps: first.perm_coef_bps,
        caps: first.caps.clone(),
        half_lives: first.half_lives.clone(),
        walks,
        consensus_stable,
        last_fold_stable,
        last_fold_span_bps,
        settled_caps,
        all_settled: settled_caps == caps,
    })
}

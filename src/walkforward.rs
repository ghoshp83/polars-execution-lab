use crate::holdout::verdict;
use crate::model::Tick;
use crate::pov::measured;
use crate::stability::stability;
use anyhow::{anyhow, Result};
use serde::Serialize;

/// One step of the walk: the stability grid run with session `input` as the
/// input and only the sessions before it, oldest first, as the history.
#[derive(Debug, Clone, Serialize)]
pub struct WalkForwardFold {
    /// Position of the input session.
    pub input: usize,
    /// Sessions `history_from..input` are its history: `0` on an expanding
    /// walk, `input - window` on a rolling one.
    pub history_from: usize,
    /// The parent this fold priced: the one given, or `parent_share` of its
    /// history's mean session volume.
    pub parent_qty: f64,
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
///
/// **A fixed parent is a different order on a different day.** The same size
/// is a large share of a quiet session and a sliver of a busy one, and a cap
/// that never binds cannot tell plans apart. With `parent_share` set, each
/// fold sizes its parent as that share of its history's mean session volume:
/// a number the desk had before the session opened, never the volume of the
/// session it scores. The fold's `parent_qty` says what it came to.
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
    /// The parent every fold priced, or `null` when each fold sized its own.
    pub parent_qty: Option<f64>,
    /// Each fold's parent as a share of its history's mean session volume, or
    /// `null` when the parent was given.
    pub parent_share: Option<f64>,
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
    walk(
        sessions,
        bucket_ns,
        Parent::Qty(parent_qty),
        cap_grid,
        hl_grid,
        coef_bps,
        perm_coef_bps,
        window,
        None,
    )
}

/// [`walk_forward`] with each fold's parent sized as `parent_share` of its
/// history's mean session volume instead of given.
#[allow(clippy::too_many_arguments)]
pub fn walk_forward_share(
    sessions: &[Vec<Tick>],
    bucket_ns: i64,
    parent_share: f64,
    cap_grid: &[f64],
    hl_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
    window: Option<usize>,
) -> Result<WalkForwardReport> {
    walk(
        sessions,
        bucket_ns,
        Parent::Share(parent_share),
        cap_grid,
        hl_grid,
        coef_bps,
        perm_coef_bps,
        window,
        None,
    )
}

/// How a walk sizes the parent each fold prices.
#[derive(Debug, Clone, Copy)]
enum Parent {
    /// The same quantity on every fold.
    Qty(f64),
    /// This share of the fold's history's mean session volume.
    Share(f64),
}

/// `share` of the mean volume of the history sessions. Each session's volume
/// is the total the forecast itself measures, and the sessions are added in an
/// explicit loop, oldest first, as the Python side adds them.
fn history_parent(history: &[Vec<Tick>], bucket_ns: i64, share: f64) -> Result<f64> {
    let mut total = 0.0;
    for s in history {
        total += measured(s, bucket_ns)?.2;
    }
    Ok(share * (total / history.len() as f64))
}

/// [`walk_forward`], given the growing walk of the same sessions when there is
/// one. A window of `w` opens on session `w` with sessions `0..w` behind it,
/// which the growing walk has already scored, so that fold is taken from it
/// and not worked out a second time.
#[allow(clippy::too_many_arguments)]
fn walk(
    sessions: &[Vec<Tick>],
    bucket_ns: i64,
    parent: Parent,
    cap_grid: &[f64],
    hl_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
    window: Option<usize>,
    growing: Option<&WalkForwardReport>,
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
    if let Parent::Share(s) = parent {
        // A share above one asks for more than a whole session's volume, which
        // no cap in (0, 1] could fill.
        if !s.is_finite() || s <= 0.0 || s > 1.0 {
            return Err(anyhow!("parent_share must be in (0, 1], got {s}"));
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
    let mut fixed = 0.0;
    let mut coef = 0.0;
    let mut perm = 0.0;
    let mut caps: Vec<f64> = Vec::new();
    let mut half_lives: Vec<f64> = Vec::new();
    for input in window.unwrap_or(2)..sessions.len() {
        let history_from = window.map_or(0, |w| input - w);
        // Only a window's first fold starts at session 0, and it is never its
        // last, so the fields read off the grid below are always set.
        if let Some(g) = growing.filter(|_| history_from == 0) {
            folds.push(g.folds[input - 2].clone());
            continue;
        }
        let history = &sessions[history_from..input];
        let parent_qty = match parent {
            Parent::Qty(q) => q,
            Parent::Share(s) => history_parent(history, bucket_ns, s)?,
        };
        let grid = stability(
            history,
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
            parent_qty: grid.parent_qty,
            verdicts: grid.rows.iter().map(|r| verdict(r).to_string()).collect(),
            flat_improvement_bps: grid.rows.iter().map(|r| r.flat_improvement_bps).collect(),
            inert_caps: grid.inert_caps,
            stable_caps: grid.stable_caps,
            unstable_caps: grid.unstable_caps,
        });
        product = grid.product;
        buckets = grid.buckets;
        fixed = grid.parent_qty;
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
        parent_qty: match parent {
            Parent::Qty(_) => Some(fixed),
            Parent::Share(_) => None,
        },
        parent_share: match parent {
            Parent::Qty(_) => None,
            Parent::Share(s) => Some(r8(s)),
        },
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

/// One scored session, set against every walk that scored it.
#[derive(Debug, Serialize)]
pub struct InputSpread {
    /// Position of the session, as `input` on a [`WalkForwardFold`].
    pub input: usize,
    /// Walks that scored it: the growing one, and each window no wider than
    /// `input`.
    pub walks: usize,
    /// Different histories among them. The growing walk and the window of
    /// `input` sessions read the same ones, so this is one less than `walks`
    /// whenever that window is in the sweep.
    pub histories: usize,
    /// Per cap: true when every one of those walks gave it the same verdict.
    pub verdict_stable: Vec<bool>,
    /// Per cap: the widest gap between two of those walks'
    /// `flat_improvement_bps`.
    pub span_bps: Vec<f64>,
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
/// **The walks overlap, and the sweep counts by how much.** A window of `w`
/// starts on session `w` with sessions `0..w` behind it, which is the fold the
/// growing walk ran there: one calculation, reported by two walks. So each
/// windowed walk brings one fold fewer than it lists, `distinct_folds` is what
/// is left of `folds_run`, and `inputs` sets every scored session against the
/// walks that scored it. A session with one history under it was not compared
/// with anything, whatever `consensus_stable` says of the walks it sits in;
/// `compared_inputs` counts the sessions that were. The shared fold is worked
/// out once: the sweep runs [`stability`] `distinct_folds` times, not
/// `folds_run`.
///
/// **The newest session is one row of several, and the sweep reads them all.**
/// `last_fold_stable` and `last_fold_span_bps` are the last entry of `inputs`.
/// `every_input_stable` asks the same of every entry, `widest_span_bps` and
/// `widest_span_input` say how far a cap's size moved and on which session,
/// and `newest_only_caps` counts the caps that held on the newest session and
/// split on an earlier one -- the ones the newest-session reading passes and
/// the full table does not. Earlier sessions are scored by fewer walks, so
/// this is not a stricter test of the same thing: it is the rest of the
/// comparisons the sweep already made.
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
    /// As on [`WalkForwardReport`]: the given parent, or `null` when each fold
    /// sized its own from `parent_share`. Then a deeper history is a different
    /// parent as well as a different forecast, and `span_bps` holds both.
    pub parent_qty: Option<f64>,
    pub parent_share: Option<f64>,
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
    /// Every scored session, oldest first, against the walks that scored it.
    /// The last entry is the newest session: its `verdict_stable` and
    /// `span_bps` are `last_fold_stable` and `last_fold_span_bps`.
    pub inputs: Vec<InputSpread>,
    /// Folds across all the walks, as listed.
    pub folds_run: usize,
    /// Folds that differ in input or history: `folds_run` less one per window.
    /// This is how many the sweep computes.
    pub distinct_folds: usize,
    /// Scored sessions forecast from more than one history.
    pub compared_inputs: usize,
    /// Per cap: true when every scored session got one verdict from all the
    /// walks that scored it -- `verdict_stable` at every entry of `inputs`,
    /// where `last_fold_stable` is the last entry alone.
    pub every_input_stable: Vec<bool>,
    /// Per cap: the widest `span_bps` any scored session shows.
    pub widest_span_bps: Vec<f64>,
    /// Per cap: the `input` that span was measured on. A tie goes to the
    /// newer session, so a cap that moved nowhere points at the newest.
    pub widest_span_input: Vec<usize>,
    /// Caps the walks agreed on at the newest session and split on at an
    /// earlier one: `last_fold_stable` without `every_input_stable`.
    pub newest_only_caps: usize,
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
    sweep(
        sessions,
        bucket_ns,
        Parent::Qty(parent_qty),
        cap_grid,
        hl_grid,
        coef_bps,
        perm_coef_bps,
    )
}

/// [`window_sweep`] with each fold's parent sized as `parent_share` of its
/// history's mean session volume instead of given. The fold a window shares
/// with the growing walk has the same history, so the same parent, and is
/// still worked out once.
pub fn window_sweep_share(
    sessions: &[Vec<Tick>],
    bucket_ns: i64,
    parent_share: f64,
    cap_grid: &[f64],
    hl_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<WindowSweepReport> {
    sweep(
        sessions,
        bucket_ns,
        Parent::Share(parent_share),
        cap_grid,
        hl_grid,
        coef_bps,
        perm_coef_bps,
    )
}

fn sweep(
    sessions: &[Vec<Tick>],
    bucket_ns: i64,
    parent: Parent,
    cap_grid: &[f64],
    hl_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<WindowSweepReport> {
    let mut windows: Vec<Option<usize>> = vec![None];
    windows.extend((2..=sessions.len().saturating_sub(2)).map(Some));
    let mut reports: Vec<WalkForwardReport> = Vec::with_capacity(windows.len());
    for window in windows {
        let report = walk(
            sessions,
            bucket_ns,
            parent,
            cap_grid,
            hl_grid,
            coef_bps,
            perm_coef_bps,
            window,
            reports.first(),
        )?;
        reports.push(report);
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
    let inputs: Vec<InputSpread> = (2..sessions.len())
        .map(|input| {
            // The growing walk scores every session from the third on, so
            // there is always a first.
            let scored: Vec<&WalkForwardFold> = reports
                .iter()
                .flat_map(|r| r.folds.iter().filter(|f| f.input == input))
                .collect();
            let mut histories: Vec<usize> = scored.iter().map(|f| f.history_from).collect();
            histories.sort_unstable();
            histories.dedup();
            InputSpread {
                input,
                walks: scored.len(),
                histories: histories.len(),
                verdict_stable: (0..caps)
                    .map(|c| {
                        scored
                            .iter()
                            .all(|f| f.verdicts[c] == scored[0].verdicts[c])
                    })
                    .collect(),
                span_bps: (0..caps)
                    .map(|c| {
                        let mut lo = scored[0].flat_improvement_bps[c];
                        let mut hi = lo;
                        for f in &scored[1..] {
                            let v = f.flat_improvement_bps[c];
                            if v < lo {
                                lo = v;
                            }
                            if v > hi {
                                hi = v;
                            }
                        }
                        r8(hi - lo)
                    })
                    .collect(),
            }
        })
        .collect();
    let folds_run = reports.iter().map(|r| r.folds.len()).sum();
    let distinct_folds = inputs.iter().map(|i| i.histories).sum();
    let compared_inputs = inputs.iter().filter(|i| i.histories > 1).count();
    let every_input_stable: Vec<bool> = (0..caps)
        .map(|c| inputs.iter().all(|i| i.verdict_stable[c]))
        .collect();
    let mut widest_span_bps: Vec<f64> = Vec::with_capacity(caps);
    let mut widest_span_input: Vec<usize> = Vec::with_capacity(caps);
    for c in 0..caps {
        let mut widest = &inputs[0];
        for i in &inputs[1..] {
            if i.span_bps[c] >= widest.span_bps[c] {
                widest = i;
            }
        }
        widest_span_bps.push(widest.span_bps[c]);
        widest_span_input.push(widest.input);
    }
    let newest_only_caps = (0..caps)
        .filter(|c| last_fold_stable[*c] && !every_input_stable[*c])
        .count();
    Ok(WindowSweepReport {
        product: first.product.clone(),
        bucket_ns,
        buckets: first.buckets,
        sessions: sessions.len(),
        parent_qty: first.parent_qty,
        parent_share: first.parent_share,
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
        inputs,
        folds_run,
        distinct_folds,
        compared_inputs,
        every_input_stable,
        widest_span_bps,
        widest_span_input,
        newest_only_caps,
    })
}

/// One share of the sweep: [`window_sweep_share`] at that share, cut down to
/// what the shares can be compared on.
#[derive(Debug, Serialize)]
pub struct ShareRow {
    pub parent_share: f64,
    /// Per cap, in `caps` order: the verdict every walk's `consensus` gave it,
    /// or `null` when the walks split or agreed only on `mixed`.
    pub settled: Vec<Option<String>>,
    pub settled_caps: usize,
    /// Per cap: the verdict the growing walk gave the newest session.
    pub last_verdicts: Vec<String>,
}

/// **Does a settled cap depend on the share the parent was sized at?**
///
/// `--parent-share` replaced a size typed once for every day with a size the
/// desk had before each session opened. It did not replace the typing: the
/// share is a number too, and nothing here fits it. This runs the window sweep
/// at every share on a grid and reports, per cap, what it settled on at each.
///
/// A cap is `share_stable` when it settled at every share, on the same
/// verdict. One that settled at some shares and not at others settled because
/// of the share, and `settled_at` says at how many. Every share is a fresh
/// sweep: a different parent prices every fold differently, so no fold is
/// shared across rows.
///
/// **There is deliberately no best share.** The rows are listed in grid order
/// and nothing here ranks them: a share picked for the verdict it gives is a
/// verdict picked.
#[derive(Debug, Serialize)]
pub struct ShareSweepReport {
    pub product: String,
    pub bucket_ns: i64,
    pub buckets: usize,
    pub sessions: usize,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    pub caps: Vec<f64>,
    pub half_lives: Vec<f64>,
    /// One row per share, in grid order.
    pub shares: Vec<ShareRow>,
    /// Per cap: how many shares it settled at, on any verdict.
    pub settled_at: Vec<usize>,
    /// Per cap: true when it settled at every share, on the same verdict.
    pub share_stable: Vec<bool>,
    /// Caps that are `share_stable`.
    pub held_caps: usize,
    /// Per cap: true when the growing walk gave the newest session the same
    /// verdict at every share. Where nothing settles this is the reading left,
    /// and a cap that gains at one share and loses at another was decided by it.
    pub last_share_stable: Vec<bool>,
}

/// Run [`window_sweep_share`] at every share on `share_grid`.
///
/// Mirrors `share_sweep` in `python/xexeclab/engine.py` operation for
/// operation, so the two engines report bit-for-bit identical sweeps.
pub fn share_sweep(
    sessions: &[Vec<Tick>],
    bucket_ns: i64,
    share_grid: &[f64],
    cap_grid: &[f64],
    hl_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<ShareSweepReport> {
    // One share is `pov-window-sweep --parent-share`, not a sweep. Each share
    // is checked against (0, 1] by the walk itself.
    if share_grid.len() < 2 {
        return Err(anyhow!(
            "share_grid needs at least 2 points, got {}",
            share_grid.len()
        ));
    }
    for w in share_grid.windows(2) {
        if w[1] <= w[0] {
            return Err(anyhow!(
                "share_grid must be strictly increasing, got {} then {}",
                w[0],
                w[1]
            ));
        }
    }
    let mut sweeps: Vec<WindowSweepReport> = Vec::with_capacity(share_grid.len());
    for share in share_grid {
        sweeps.push(sweep(
            sessions,
            bucket_ns,
            Parent::Share(*share),
            cap_grid,
            hl_grid,
            coef_bps,
            perm_coef_bps,
        )?);
    }
    let first = &sweeps[0];
    let caps = first.caps.len();
    let shares: Vec<ShareRow> = share_grid
        .iter()
        .zip(&sweeps)
        .map(|(share, s)| ShareRow {
            parent_share: r8(*share),
            settled: (0..caps)
                .map(|c| {
                    let v = &s.walks[0].consensus[c];
                    (s.consensus_stable[c] && v != "mixed").then(|| v.clone())
                })
                .collect(),
            settled_caps: s.settled_caps,
            last_verdicts: s.walks[0].last_verdicts.clone(),
        })
        .collect();
    let settled_at: Vec<usize> = (0..caps)
        .map(|c| shares.iter().filter(|r| r.settled[c].is_some()).count())
        .collect();
    let share_stable: Vec<bool> = (0..caps)
        .map(|c| {
            shares[0].settled[c].is_some()
                && shares.iter().all(|r| r.settled[c] == shares[0].settled[c])
        })
        .collect();
    let held_caps = share_stable.iter().filter(|s| **s).count();
    let last_share_stable: Vec<bool> = (0..caps)
        .map(|c| {
            shares
                .iter()
                .all(|r| r.last_verdicts[c] == shares[0].last_verdicts[c])
        })
        .collect();
    Ok(ShareSweepReport {
        product: first.product.clone(),
        bucket_ns,
        buckets: first.buckets,
        sessions: sessions.len(),
        coef_bps: first.coef_bps,
        perm_coef_bps: first.perm_coef_bps,
        caps: first.caps.clone(),
        half_lives: first.half_lives.clone(),
        shares,
        settled_at,
        share_stable,
        held_caps,
        last_share_stable,
    })
}

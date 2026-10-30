use crate::depth::{depth_metrics, queue_metrics};
use crate::model::BookLevel;
use anyhow::{anyhow, Result};
use serde::Serialize;

/// Round to 8 decimal places, half away from zero, as the Python side does.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// One spacing of a [`GridSweep`]: the depth and queue metrics of the
/// snapshots a grid of `every_s` seconds keeps, beside the full-rate book.
/// - `spread_ratio` -- this row's `avg_spread` over the full-rate figure; under
///   1 means the grid saw a tighter touch than the book had.
/// - `queue_sign_kept` -- whether `avg_queue_imbalance` rests on the same side
///   as at full rate (both strictly positive or both strictly negative).
#[derive(Debug, Serialize)]
pub struct GridRow {
    pub every_s: f64,
    pub snapshots: usize,
    pub avg_bid_depth: f64,
    pub avg_ask_depth: f64,
    pub avg_depth_imbalance: f64,
    pub avg_spread: f64,
    pub avg_bid_queue: f64,
    pub avg_ask_queue: f64,
    pub avg_queue_imbalance: f64,
    pub spread_ratio: f64,
    pub queue_sign_kept: bool,
}

/// A full-rate book thinned to each spacing on a grid, the way `ingest-book
/// --every` thins its writes, and measured at each. `full` is the book as
/// recorded (`every_s` 0); `sign_flips` counts the rows whose queue imbalance
/// left the full-rate side. No spacing is named best: what a grid may hide
/// depends on which metric the reader needs.
#[derive(Debug, Serialize)]
pub struct GridSweep {
    pub product: String,
    pub full: GridRow,
    pub rows: Vec<GridRow>,
    pub sign_flips: usize,
}

/// The snapshots a grid of `every_ns` keeps: the first at least `offset_ns`
/// into the book, then each one at least `every_ns` after the last kept.
/// Offline this keeps the first snapshot that moved after the step; live,
/// `--every` writes the book as it stood at the first update after it, which is
/// the same book unless a copy was skipped.
fn thin(levels: &[BookLevel], every_ns: i64, offset_ns: i64) -> Vec<BookLevel> {
    let mut ts: Vec<i64> = levels.iter().map(|l| l.ts_ns).collect();
    ts.sort_unstable();
    ts.dedup();
    let start = ts.first().map_or(0, |&t| t.saturating_add(offset_ns));
    let mut kept: Vec<i64> = Vec::new();
    for t in ts {
        if t >= start && kept.last().is_none_or(|&last| t - last >= every_ns) {
            kept.push(t);
        }
    }
    levels
        .iter()
        .filter(|l| kept.binary_search(&l.ts_ns).is_ok())
        .cloned()
        .collect()
}

fn row(
    levels: &[BookLevel],
    product: &str,
    every_s: f64,
    full: Option<&GridRow>,
) -> Result<GridRow> {
    let depth = depth_metrics(levels, product)?;
    let queue = queue_metrics(levels, product)?;
    let (spread_ratio, queue_sign_kept) = match full {
        Some(f) => (
            r8(depth.avg_spread / f.avg_spread),
            queue.avg_queue_imbalance * f.avg_queue_imbalance > 0.0,
        ),
        None => (1.0, true),
    };
    Ok(GridRow {
        every_s,
        snapshots: depth.snapshots,
        avg_bid_depth: depth.avg_bid_depth,
        avg_ask_depth: depth.avg_ask_depth,
        avg_depth_imbalance: depth.avg_depth_imbalance,
        avg_spread: depth.avg_spread,
        avg_bid_queue: queue.avg_bid_queue,
        avg_ask_queue: queue.avg_ask_queue,
        avg_queue_imbalance: queue.avg_queue_imbalance,
        spread_ratio,
        queue_sign_kept,
    })
}

/// A grid is positive, finite spacings, none twice: a spacing listed twice would
/// count twice in every held figure read over the grid.
fn check_every_grid(every_grid: &[f64]) -> Result<()> {
    if every_grid.is_empty() {
        return Err(anyhow!("empty --every-grid"));
    }
    if let Some(bad) = every_grid.iter().find(|&&s| !s.is_finite() || s <= 0.0) {
        return Err(anyhow!(
            "--every-grid spacings must be finite and > 0, got {bad}"
        ));
    }
    for (i, s) in every_grid.iter().enumerate() {
        if every_grid[..i].contains(s) {
            return Err(anyhow!("--every-grid repeats {s}"));
        }
    }
    Ok(())
}

/// The book as recorded, the row every spacing is read against.
fn full_row(levels: &[BookLevel], product: &str) -> Result<GridRow> {
    let full = row(levels, product, 0.0, None)?;
    if full.avg_spread.is_nan() || full.avg_spread <= 0.0 {
        return Err(anyhow!(
            "full-rate spread {} is not positive",
            full.avg_spread
        ));
    }
    Ok(full)
}

/// Sweep a recorded book over `every_grid` (seconds, each > 0). Mirrors
/// `book_grid_sweep` in `python/xexeclab/engine.py`.
pub fn book_grid_sweep(
    levels: &[BookLevel],
    product: &str,
    every_grid: &[f64],
) -> Result<GridSweep> {
    check_every_grid(every_grid)?;
    let full = full_row(levels, product)?;
    let rows = every_grid
        .iter()
        .map(|&s| {
            row(
                &thin(levels, (s * 1e9).round() as i64, 0),
                product,
                s,
                Some(&full),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let sign_flips = rows.iter().filter(|r| !r.queue_sign_kept).count();
    Ok(GridSweep {
        product: product.to_string(),
        full,
        rows,
        sign_flips,
    })
}

/// One spacing of a [`GridAcross`], read over every book at once.
/// - `sign_kept_books` -- how many books keep their full-rate queue side here.
/// - `sign_lost_by` -- the indices of the books that do not, in input order;
///   a spacing lost by one book alone is the one its absence would hold.
/// - `spread_ratio_min` / `spread_ratio_max` -- the range of `spread_ratio`.
/// - `imbalance_drift_max` -- the widest gap between this row's depth
///   imbalance and its own book's full-rate figure.
#[derive(Debug, Serialize)]
pub struct GridSpacing {
    pub every_s: f64,
    pub sign_kept_books: usize,
    pub sign_lost_by: Vec<usize>,
    pub spread_ratio_min: f64,
    pub spread_ratio_max: f64,
    pub imbalance_drift_max: f64,
}

/// One grid over several full-rate books. `held_spacings` counts the spacings
/// at which every book keeps its queue side; `held_without[i]` counts them
/// with book `i` left out, so a book whose absence raises the count is the
/// one taking spacings away. A spacing held on the books recorded so far is
/// not shown to hold on the next one, so none is named best.
#[derive(Debug, Serialize)]
pub struct GridAcross {
    pub books: Vec<GridSweep>,
    pub spacings: Vec<GridSpacing>,
    pub held_spacings: usize,
    pub held_without: Vec<usize>,
}

/// [`book_grid_sweep`] on each book (two or more), then read per spacing.
/// Mirrors `book_grid_across` in `python/xexeclab/engine.py`.
pub fn book_grid_across(
    books: &[Vec<BookLevel>],
    product: &str,
    every_grid: &[f64],
) -> Result<GridAcross> {
    if books.len() < 2 {
        return Err(anyhow!(
            "book-grid-across needs two or more books, got {}",
            books.len()
        ));
    }
    let books = books
        .iter()
        .map(|levels| book_grid_sweep(levels, product, every_grid))
        .collect::<Result<Vec<_>>>()?;
    let spacings: Vec<GridSpacing> = (0..every_grid.len())
        .map(|i| {
            let rows: Vec<(&GridRow, &GridRow)> =
                books.iter().map(|b| (&b.rows[i], &b.full)).collect();
            GridSpacing {
                every_s: every_grid[i],
                sign_kept_books: rows.iter().filter(|(r, _)| r.queue_sign_kept).count(),
                sign_lost_by: (0..rows.len())
                    .filter(|&j| !rows[j].0.queue_sign_kept)
                    .collect(),
                spread_ratio_min: rows
                    .iter()
                    .map(|(r, _)| r.spread_ratio)
                    .fold(f64::INFINITY, f64::min),
                spread_ratio_max: rows
                    .iter()
                    .map(|(r, _)| r.spread_ratio)
                    .fold(f64::NEG_INFINITY, f64::max),
                imbalance_drift_max: rows
                    .iter()
                    .map(|(r, f)| r8((r.avg_depth_imbalance - f.avg_depth_imbalance).abs()))
                    .fold(f64::NEG_INFINITY, f64::max),
            }
        })
        .collect();
    let held_spacings = spacings
        .iter()
        .filter(|s| s.sign_kept_books == books.len())
        .count();
    let held_without = (0..books.len())
        .map(|out| {
            (0..every_grid.len())
                .filter(|&i| {
                    books
                        .iter()
                        .enumerate()
                        .all(|(j, b)| j == out || b.rows[i].queue_sign_kept)
                })
                .count()
        })
        .collect();
    Ok(GridAcross {
        books,
        spacings,
        held_spacings,
        held_without,
    })
}

/// One spacing of a [`GridPhase`], read over every start the grid was given.
/// - `phases_run` -- the starts that kept a snapshot; one past the end of the
///   book keeps nothing and is left out, so this can be under `phases`.
/// - `sign_kept_phases` -- how many of those keep the full-rate queue side.
/// - `first_phase_kept` -- the verdict at offset 0, the one `book-grid` reports.
/// - `snapshots_min` / `snapshots_max`, `spread_ratio_min` / `spread_ratio_max`,
///   `queue_imbalance_min` / `queue_imbalance_max` -- the range over the starts.
#[derive(Debug, Serialize)]
pub struct PhaseSpacing {
    pub every_s: f64,
    pub phases_run: usize,
    pub snapshots_min: usize,
    pub snapshots_max: usize,
    pub sign_kept_phases: usize,
    pub first_phase_kept: bool,
    pub spread_ratio_min: f64,
    pub spread_ratio_max: f64,
    pub queue_imbalance_min: f64,
    pub queue_imbalance_max: f64,
}

/// One book, each spacing started `phases` ways evenly across the spacing. A
/// spacing is `held` when every start keeps the queue side, `lost` when none
/// does, `split` otherwise; the three counts partition the grid. A split spacing
/// has no verdict of its own: which side it reports is where the grid began.
/// `held` means no start tried lost it: doubling `phases` keeps every start, so
/// it can only move a spacing out of `held` or `lost`, never into them.
#[derive(Debug, Serialize)]
pub struct GridPhase {
    pub product: String,
    pub phases: usize,
    pub full: GridRow,
    pub spacings: Vec<PhaseSpacing>,
    pub held_spacings: usize,
    pub lost_spacings: usize,
    pub split_spacings: usize,
}

/// The grid at `phases` starts (two or more): start `k` is `k / phases` of a
/// spacing after the first snapshot, and start 0 is the grid `book-grid` runs.
/// Mirrors `book_grid_phase` in `python/xexeclab/engine.py`.
pub fn book_grid_phase(
    levels: &[BookLevel],
    product: &str,
    every_grid: &[f64],
    phases: usize,
) -> Result<GridPhase> {
    check_every_grid(every_grid)?;
    if phases < 2 {
        return Err(anyhow!("--phases must be 2 or more, got {phases}"));
    }
    let full = full_row(levels, product)?;
    let mut spacings = Vec::with_capacity(every_grid.len());
    for &s in every_grid {
        let every_ns = (s * 1e9).round() as i64;
        let mut rows = Vec::with_capacity(phases);
        for k in 0..phases {
            let offset_ns = (i128::from(every_ns) * k as i128 / phases as i128) as i64;
            let kept = thin(levels, every_ns, offset_ns);
            if !kept.is_empty() {
                rows.push(row(&kept, product, s, Some(&full))?);
            }
        }
        let range = |f: fn(&GridRow) -> f64| {
            rows.iter()
                .map(f)
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| {
                    (lo.min(x), hi.max(x))
                })
        };
        let (spread_ratio_min, spread_ratio_max) = range(|r| r.spread_ratio);
        let (queue_imbalance_min, queue_imbalance_max) = range(|r| r.avg_queue_imbalance);
        spacings.push(PhaseSpacing {
            every_s: s,
            phases_run: rows.len(),
            snapshots_min: rows.iter().map(|r| r.snapshots).min().unwrap_or(0),
            snapshots_max: rows.iter().map(|r| r.snapshots).max().unwrap_or(0),
            sign_kept_phases: rows.iter().filter(|r| r.queue_sign_kept).count(),
            first_phase_kept: rows[0].queue_sign_kept,
            spread_ratio_min,
            spread_ratio_max,
            queue_imbalance_min,
            queue_imbalance_max,
        });
    }
    let held_spacings = spacings
        .iter()
        .filter(|s| s.sign_kept_phases == s.phases_run)
        .count();
    let lost_spacings = spacings.iter().filter(|s| s.sign_kept_phases == 0).count();
    let split_spacings = spacings.len() - held_spacings - lost_spacings;
    Ok(GridPhase {
        product: product.to_string(),
        phases,
        full,
        spacings,
        held_spacings,
        lost_spacings,
        split_spacings,
    })
}

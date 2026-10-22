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

/// The snapshots a grid of `every_ns` keeps: the first, then each one at least
/// `every_ns` after the last kept. Offline this keeps the first snapshot that
/// moved after the step; live, `--every` writes the book as it stood at the
/// first update after it, which is the same book unless a copy was skipped.
fn thin(levels: &[BookLevel], every_ns: i64) -> Vec<BookLevel> {
    let mut ts: Vec<i64> = levels.iter().map(|l| l.ts_ns).collect();
    ts.sort_unstable();
    ts.dedup();
    let mut kept: Vec<i64> = Vec::new();
    for t in ts {
        if kept.last().is_none_or(|&last| t - last >= every_ns) {
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

/// Sweep a recorded book over `every_grid` (seconds, each > 0). Mirrors
/// `book_grid_sweep` in `python/xexeclab/engine.py`.
pub fn book_grid_sweep(
    levels: &[BookLevel],
    product: &str,
    every_grid: &[f64],
) -> Result<GridSweep> {
    if every_grid.is_empty() {
        return Err(anyhow!("empty --every-grid"));
    }
    if let Some(bad) = every_grid.iter().find(|&&s| s.is_nan() || s <= 0.0) {
        return Err(anyhow!("--every-grid spacings must be > 0, got {bad}"));
    }
    let full = row(levels, product, 0.0, None)?;
    if full.avg_spread.is_nan() || full.avg_spread <= 0.0 {
        return Err(anyhow!(
            "full-rate spread {} is not positive",
            full.avg_spread
        ));
    }
    let rows = every_grid
        .iter()
        .map(|&s| {
            row(
                &thin(levels, (s * 1e9).round() as i64),
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
/// - `spread_ratio_min` / `spread_ratio_max` -- the range of `spread_ratio`.
/// - `imbalance_drift_max` -- the widest gap between this row's depth
///   imbalance and its own book's full-rate figure.
#[derive(Debug, Serialize)]
pub struct GridSpacing {
    pub every_s: f64,
    pub sign_kept_books: usize,
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

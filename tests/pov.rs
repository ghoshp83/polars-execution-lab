use xexec::execution::session_vwap;
use xexec::model::Tick;
use xexec::pov::pov_schedule;

/// One second per bucket.
const BUCKET: i64 = 1_000_000_000;

fn tick(ts_ns: i64, price: f64, size: f64) -> Tick {
    Tick {
        ts_ns,
        product: "BTC-USD".to_string(),
        price,
        size,
        side: "buy".to_string(),
        trade_id: ts_ns,
    }
}

/// Three buckets with a deliberately lumpy profile -- 1.0, 6.0 and 3.0 of
/// volume -- so a clock-uniform allocation and a volume-following one differ.
fn lumpy() -> Vec<Tick> {
    vec![
        tick(0, 100.0, 1.0),
        tick(BUCKET, 102.0, 4.0),
        tick(BUCKET + 500_000_000, 104.0, 2.0),
        tick(2 * BUCKET, 110.0, 3.0),
    ]
}

#[test]
fn the_plan_takes_the_same_share_of_every_bucket() {
    let plan = pov_schedule(&lumpy(), "BTC-USD", BUCKET, 1.0, 0.5, 10.0, 0.0).unwrap();
    assert_eq!(plan.buckets, 3);
    assert_eq!(plan.total_volume, 10.0);
    // 1.0 of a 10.0 capture, and that is the participation in every bucket --
    // the property the whole allocation rests on.
    assert_eq!(plan.participation, 0.1);
    for slice in &plan.schedule {
        assert_eq!(slice.participation, 0.1);
    }
}

#[test]
fn the_allocation_follows_the_volume_profile() {
    let plan = pov_schedule(&lumpy(), "BTC-USD", BUCKET, 1.0, 0.5, 10.0, 0.0).unwrap();
    let sizes: Vec<f64> = plan.schedule.iter().map(|s| s.size).collect();
    // Volumes 1.0 / 6.0 / 3.0 of 10.0 total, so the parent splits the same way.
    assert_eq!(sizes, vec![0.1, 0.6, 0.3]);
    let total: f64 = sizes.iter().sum();
    assert!((total - plan.parent_qty).abs() < 1e-9);
}

#[test]
fn following_the_volume_tracks_the_session_vwap_exactly() {
    let ticks = lumpy();
    let plan = pov_schedule(&ticks, "BTC-USD", BUCKET, 1.0, 0.5, 10.0, 0.0).unwrap();
    // This is the reason to follow volume rather than the clock, so it is
    // asserted as an exact equality, not an approximation.
    assert_eq!(plan.pov_price, session_vwap(&ticks).unwrap());
    assert_eq!(plan.pov_tracking_bps, 0.0);
}

#[test]
fn the_clock_uniform_benchmark_misses_the_session_vwap() {
    let plan = pov_schedule(&lumpy(), "BTC-USD", BUCKET, 1.0, 0.5, 10.0, 0.0).unwrap();
    // The profile is lumpy, so the unweighted mean of the bucket VWAPs is not
    // the volume-weighted one. A zero here would mean the benchmark had been
    // computed the same way as the plan.
    assert!(plan.twap_tracking_bps.abs() > 1.0);
}

#[test]
fn a_flat_profile_makes_the_two_allocations_agree() {
    // Equal volume in every bucket is the one case where following the clock
    // and following the volume are the same schedule.
    let flat = vec![
        tick(0, 100.0, 2.0),
        tick(BUCKET, 101.0, 2.0),
        tick(2 * BUCKET, 102.0, 2.0),
    ];
    let plan = pov_schedule(&flat, "BTC-USD", BUCKET, 0.6, 0.5, 10.0, 2.0).unwrap();
    assert_eq!(plan.pov_price, plan.twap_price);
    assert_eq!(plan.twap_tracking_bps, 0.0);
    assert_eq!(plan.edge_bps, 0.0);
}

#[test]
fn a_lumpy_profile_makes_following_the_volume_cheaper() {
    let plan = pov_schedule(&lumpy(), "BTC-USD", BUCKET, 1.0, 0.5, 10.0, 0.0).unwrap();
    // The clock-uniform allocation pushes a full third of the parent into the
    // thinnest bucket, and the square-root law charges for it.
    assert!(plan.twap_impact_bps > plan.pov_impact_bps);
    assert!(plan.edge_bps > 0.0);
}

#[test]
fn the_infeasible_benchmark_is_flagged_not_hidden() {
    // 1.0 of parent over three buckets puts 0.3333 into a bucket that traded
    // 1.0 -- a third of it, well past the 0.2 cap.
    let plan = pov_schedule(&lumpy(), "BTC-USD", BUCKET, 1.0, 0.2, 10.0, 0.0).unwrap();
    assert!(plan.twap_max_participation > plan.cap);
    assert!(!plan.twap_feasible);
    // The plan itself is still inside the cap; only the benchmark is not.
    assert!(plan.participation <= plan.cap);
}

#[test]
fn an_order_above_the_cap_is_refused() {
    let err = pov_schedule(&lumpy(), "BTC-USD", BUCKET, 5.0, 0.25, 10.0, 0.0).unwrap_err();
    assert!(err.to_string().contains("above the cap"));
}

#[test]
fn a_cap_outside_the_unit_interval_is_refused() {
    for cap in [0.0, -0.1, 1.5] {
        let err = pov_schedule(&lumpy(), "BTC-USD", BUCKET, 1.0, cap, 10.0, 0.0).unwrap_err();
        assert!(err.to_string().contains("cap must be in (0, 1]"));
    }
}

#[test]
fn a_parent_of_no_size_is_refused() {
    let err = pov_schedule(&lumpy(), "BTC-USD", BUCKET, 0.0, 0.5, 10.0, 0.0).unwrap_err();
    assert!(err.to_string().contains("parent_qty must be a positive"));
}

#[test]
fn a_capture_of_no_volume_is_refused() {
    let empty = vec![tick(0, 100.0, 0.0), tick(BUCKET, 101.0, 0.0)];
    let err = pov_schedule(&empty, "BTC-USD", BUCKET, 1.0, 0.5, 10.0, 0.0).unwrap_err();
    assert!(err.to_string().contains("zero traded volume"));
}

#[test]
fn an_empty_capture_is_refused() {
    let err = pov_schedule(&[], "BTC-USD", BUCKET, 1.0, 0.5, 10.0, 0.0).unwrap_err();
    assert!(err.to_string().contains("no ticks"));
}

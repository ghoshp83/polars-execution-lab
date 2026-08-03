use xexec::impact::impact_curve;
use xexec::model::ImpactSlice;
use xexec::replay::read_impact;

fn load() -> Vec<ImpactSlice> {
    read_impact("data/sample_impact.ndjson").expect("sample impact replay must load")
}

fn slice(ts_ns: i64, participation: f64) -> ImpactSlice {
    ImpactSlice {
        ts_ns,
        product: "BTC-USD".into(),
        participation,
    }
}

#[test]
fn square_root_curve_summarises_the_sample_schedule() {
    let slices = load();
    // participations 0.01/0.04/0.09/0.16/0.25 -> sqrt 0.1/0.2/0.3/0.4/0.5; with
    // coef_bps=10 the per-slice impact is 1/2/3/4/5 bps: avg 3, max 5, total 15.
    let m = impact_curve(&slices, "BTC-USD", 10.0).unwrap();
    assert_eq!(m.slices, 5);
    assert!((m.avg_impact_bps - 3.0).abs() < 1e-9);
    assert!((m.max_impact_bps - 5.0).abs() < 1e-9);
    assert!((m.total_impact_bps - 15.0).abs() < 1e-9);
}

#[test]
fn impact_is_concave_in_participation() {
    // Doubling participation less than doubles the impact (sqrt, not linear):
    // going 0.25 -> 0.5 lifts sqrt-participation by 0.5 -> ~0.707, a 1.41x move.
    let small = impact_curve(&[slice(0, 0.25)], "BTC-USD", 10.0).unwrap();
    let big = impact_curve(&[slice(0, 0.5)], "BTC-USD", 10.0).unwrap();
    assert!(big.avg_impact_bps > small.avg_impact_bps);
    assert!(big.avg_impact_bps < 2.0 * small.avg_impact_bps);
}

#[test]
fn zero_participation_costs_nothing() {
    let m = impact_curve(&[slice(0, 0.0)], "BTC-USD", 10.0).unwrap();
    assert!((m.avg_impact_bps - 0.0).abs() < 1e-9);
}

#[test]
fn empty_schedule_is_rejected() {
    assert!(impact_curve(&[], "BTC-USD", 10.0).is_err());
}

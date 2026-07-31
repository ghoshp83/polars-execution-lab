"""A Parquet sink must be a lossless round-trip for the replay: converting a
canonical NDJSON replay to Parquet and back must leave every downstream
benchmark unchanged. Parquet is only a storage optimisation, never a source of
numerical drift.
"""

from xexeclab.engine import read_ticks, summary, write_ticks

SAMPLE = "data/sample_ticks.ndjson"
BUCKET_NS = 1_000_000_000


def test_parquet_round_trip_preserves_the_summary(tmp_path):
    df = read_ticks(SAMPLE)
    before = summary(df, "BTC-USD", BUCKET_NS)

    pq = tmp_path / "ticks.parquet"
    assert write_ticks(df, pq) == df.height

    after = summary(read_ticks(pq), "BTC-USD", BUCKET_NS)
    # A columnar re-encode must not change a single computed number.
    assert after == before

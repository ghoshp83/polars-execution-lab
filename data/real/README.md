# Recorded sessions

Six consecutive slices of one continuous capture of BTC-USD trades from
Coinbase's public `matches` feed, taken with `xexeclab ingest` on 2026-10-13
from 08:16:50 to 08:26:50 UTC.

| File | Trades | Volume (BTC) |
|---|---|---|
| `btc_usd_1.ndjson` | 395 | 5.726475 |
| `btc_usd_2.ndjson` | 352 | 2.552492 |
| `btc_usd_3.ndjson` | 266 | 0.833936 |
| `btc_usd_4.ndjson` | 239 | 0.984825 |
| `btc_usd_5.ndjson` | 264 | 1.159499 |
| `btc_usd_6.ndjson` | 400 | 2.823023 |

Each file is 100 seconds, cut on a 10-second boundary, so every session has the
same ten 10-second buckets. Trade ids run from 1105374864 to 1105376779 with
none missing: 1,916 trades, nothing dropped, reordered or edited. The partial
buckets at either end of the capture were left out so that every session is
whole.

Two things were done to the raw capture, and nothing else:

- It was cut into six files at the boundaries above.
- Each timestamp was rounded to the microsecond the exchange sent. The
  collector at the time converted through a float and wrote values up to 128 ns
  off; it now converts in integers and writes the same values these files hold.

## What these are, and are not

They are real trades, which the hand-written `data/sample_*.ndjson` captures are
not. They are also ten minutes of one product on one morning. A "session" here
is a 100-second slice, not a trading day, and the six are neighbours in time, so
they are far from independent. Use them to see how the commands behave on real
prices and sizes; do not read a result on them as a property of the strategy.

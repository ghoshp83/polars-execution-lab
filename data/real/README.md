# Recorded sessions

Two runs of BTC-USD trades from Coinbase's public `matches` feed, each cut into
six consecutive 100-second sessions, taken with `xexeclab ingest` thirty-one
hours apart.

**Day one** — 2026-10-13, 08:16:50 to 08:26:50 UTC:

| File | Trades | Volume (BTC) |
|---|---|---|
| `btc_usd_1.ndjson` | 395 | 5.726475 |
| `btc_usd_2.ndjson` | 352 | 2.552492 |
| `btc_usd_3.ndjson` | 266 | 0.833936 |
| `btc_usd_4.ndjson` | 239 | 0.984825 |
| `btc_usd_5.ndjson` | 264 | 1.159499 |
| `btc_usd_6.ndjson` | 400 | 2.823023 |

Trade ids 1105374864 to 1105376779: 1,916 trades, 14.08 BTC.

**Day two** — 2026-10-14, 15:17:10 to 15:27:10 UTC:

| File | Trades | Volume (BTC) |
|---|---|---|
| `btc_usd_d2_1.ndjson` | 776 | 8.229381 |
| `btc_usd_d2_2.ndjson` | 914 | 24.195836 |
| `btc_usd_d2_3.ndjson` | 734 | 17.655638 |
| `btc_usd_d2_4.ndjson` | 789 | 8.839410 |
| `btc_usd_d2_5.ndjson` | 1305 | 17.652253 |
| `btc_usd_d2_6.ndjson` | 730 | 12.822575 |

Trade ids 1105945626 to 1105950873: 5,248 trades, 89.40 BTC. The afternoon
traded 2.7 times the trades and 6.3 times the volume of the morning before.

**Quotes** — `btc_usd_d2_1_quotes.ndjson` is the top of the book from
Coinbase's `ticker` channel, taken by `xexeclab ingest-quotes` alongside the
day-two trades and cut to the first day-two session: 776 quotes. The ticker
fires once per trade with that trade's time, so each quote is the book a trade
left behind, at the same instant as the matching row of `btc_usd_d2_1.ndjson` —
not a sample on a clock. The spread runs from one cent (371 of the 776) to
eighteen dollars and never crosses.

Each session file is 100 seconds, cut on a 10-second boundary, so every session
has the same ten 10-second buckets. Within each run the trade ids are
contiguous: nothing dropped, reordered or edited. The partial buckets at either
end of each capture were left out so that every session is whole. Both
collectors reported zero reconnects.

Two things were done to the day-one capture, and one to day two:

- Each capture was cut into six files at the boundaries above.
- Day one only: each timestamp was rounded to the microsecond the exchange
  sent. The collector at the time converted through a float and wrote values
  up to 128 ns off; it now converts in integers, and day two was recorded
  after that fix.

## What these are, and are not

They are real trades, which the hand-written `data/sample_*.ndjson` captures are
not. They are also twenty minutes of one product: a morning and, a day later, an
afternoon. A "session" here is a 100-second slice, not a trading day, and
the six in each run are neighbours in time, so they are far from independent.
Two days is enough to show that a result on one does not have to hold on the
other; it is not enough to say which one is typical. Use them to see how the
commands behave on real prices and sizes; do not read a result on them as a
property of the strategy.

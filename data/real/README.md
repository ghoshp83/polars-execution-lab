# Recorded sessions

Three runs of BTC-USD trades from Coinbase's public `matches` feed, each cut
into six consecutive 100-second sessions, taken with `xexeclab ingest`: a
weekday morning, a weekday afternoon thirty-one hours later, and a Saturday
morning.

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

**Day three** — 2026-10-17 (a Saturday), 11:29:30 to 11:39:30 UTC:

| File | Trades | Volume (BTC) |
|---|---|---|
| `btc_usd_d3_1.ndjson` | 351 | 1.119255 |
| `btc_usd_d3_2.ndjson` | 322 | 0.276185 |
| `btc_usd_d3_3.ndjson` | 351 | 0.567521 |
| `btc_usd_d3_4.ndjson` | 267 | 0.554398 |
| `btc_usd_d3_5.ndjson` | 351 | 0.173903 |
| `btc_usd_d3_6.ndjson` | 344 | 0.067231 |

Trade ids 1107335588 to 1107337573: 1,986 trades, 2.76 BTC. About as many
trades as day one, a fifth of its volume and a thirtieth of day two's. The last
session traded 0.067 BTC.

**Quotes** — `btc_usd_d2_1_quotes.ndjson` is the top of the book from
Coinbase's `ticker` channel, taken by `xexeclab ingest-quotes` alongside the
day-two trades and cut to the first day-two session: 776 quotes. The ticker
fires once per trade with that trade's time, so each quote is the book a trade
left behind, at the same instant as the matching row of `btc_usd_d2_1.ndjson` —
not a sample on a clock. The spread runs from one cent (371 of the 776) to
eighteen dollars and never crosses.

**Book** — `btc_usd_d3_1_book.ndjson` is the order book from Coinbase's public
`level2_batch` channel, taken by `xexeclab ingest-book --levels 10` alongside
the day-three trades and cut to the first day-three session: 1,431 snapshots,
28,620 rows. The collector seeds the book from the exchange's snapshot and
applies each update. After each update it writes the top ten levels of each
side, so a snapshot follows every batch of changes, not a fixed clock. Ten
levels hold 0.66 BTC on the ask and 0.74 on the bid on average, within a cent
or two of the touch: an order of one bitcoin swept at once runs off the
recorded book in 89% of snapshots. Deeper levels exist on the exchange but are
not in the file.

Each session file is 100 seconds, cut on a 10-second boundary, so every session
has the same ten 10-second buckets. Within each run the trade ids are
contiguous: nothing dropped, reordered or edited. The partial buckets at either
end of each capture were left out so that every session is whole. Every
collector reported zero reconnects.

Two things were done to the day-one capture, and one to each of the others:

- Each capture was cut into six files at the boundaries above, and the quotes
  and the book to the first session of their day.
- Day one only: each timestamp was rounded to the microsecond the exchange
  sent. The collector at the time converted through a float and wrote values
  up to 128 ns off; it now converts in integers, and days two and three were
  recorded after that fix.

## What these are, and are not

They are real trades, which the hand-written `data/sample_*.ndjson` captures are
not. They are also thirty minutes of one product: a weekday morning, a weekday
afternoon and a Saturday morning. A "session" here is a 100-second slice, not a trading
day, and the six in each run are neighbours in time, so they are far from
independent. Three days is enough to show that a result on one does not have to
hold on another. It is not enough to say which one is typical, and the three
traded volumes thirty times apart. The book is ten levels of one 100-second
stretch, so it says how shallow the touch was that morning, not how deep the
market is. Use them to see how the
commands behave on real prices and sizes; do not read a result on them as a
property of the strategy.

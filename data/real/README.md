# Recorded sessions

Six runs of BTC-USD trades from Coinbase's public `matches` feed, each cut
into six consecutive 100-second sessions, taken with `xexeclab ingest`: a
weekday morning, a weekday afternoon thirty-one hours later, a Saturday
morning, the Sunday morning after it, the Monday morning after that, and the
Tuesday morning after that.

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

**Day four** — 2026-10-18 (a Sunday), 08:49:50 to 08:59:50 UTC:

| File | Trades | Volume (BTC) |
|---|---|---|
| `btc_usd_d4_1.ndjson` | 109 | 0.289501 |
| `btc_usd_d4_2.ndjson` | 135 | 0.432126 |
| `btc_usd_d4_3.ndjson` | 135 | 1.061810 |
| `btc_usd_d4_4.ndjson` | 148 | 1.247667 |
| `btc_usd_d4_5.ndjson` | 257 | 0.423583 |
| `btc_usd_d4_6.ndjson` | 154 | 1.065455 |

Trade ids 1107571770 to 1107572707: 938 trades, 4.52 BTC. The fewest trades of
the four days, but larger ones than day three's, and the last session traded
over a bitcoin.

**Day five** — 2026-10-19 (a Monday), 08:35:10 to 08:45:10 UTC:

| File | Trades | Volume (BTC) |
|---|---|---|
| `btc_usd_d5_1.ndjson` | 261 | 1.853192 |
| `btc_usd_d5_2.ndjson` | 546 | 4.233804 |
| `btc_usd_d5_3.ndjson` | 597 | 6.458224 |
| `btc_usd_d5_4.ndjson` | 503 | 4.958949 |
| `btc_usd_d5_5.ndjson` | 350 | 3.252453 |
| `btc_usd_d5_6.ndjson` | 168 | 1.974468 |

Trade ids 1107875909 to 1107878333: 2,425 trades, 22.73 BTC. Five times the
Sunday's volume, at about the same hour on the next working day.

**Day six** — 2026-10-20 (a Tuesday), 08:55:30 to 09:05:30 UTC:

| File | Trades | Volume (BTC) |
|---|---|---|
| `btc_usd_d6_1.ndjson` | 180 | 2.007030 |
| `btc_usd_d6_2.ndjson` | 182 | 0.613340 |
| `btc_usd_d6_3.ndjson` | 343 | 3.912187 |
| `btc_usd_d6_4.ndjson` | 480 | 3.651674 |
| `btc_usd_d6_5.ndjson` | 316 | 3.275120 |
| `btc_usd_d6_6.ndjson` | 237 | 2.513207 |

Trade ids 1108309641 to 1108311378: 1,738 trades, 15.97 BTC. Quieter than the
Monday, twenty minutes later in the morning.

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

**Deep book** — `btc_usd_d4_book.ndjson` is the same channel taken with
`xexeclab ingest-book --levels 50 --every 5` alongside the day-four trades,
over all six day-four sessions: 119 snapshots, 11,900 rows. `--every 5` writes
at most one snapshot per five seconds of exchange time; every update in between
is still applied to the book, so each snapshot is the book at that instant.
Fifty levels hold 9.68 BTC on the ask and 10.54 on the bid on average, and an
order of five bitcoin fills on either side in every snapshot. The same
snapshots cut to their top ten levels hold 1.50 and 1.82, and fill a 2 BTC buy
in 32 of 119.

**Full-rate book** — `btc_usd_d5_book_full.ndjson` is the same channel taken
with `xexeclab ingest-book --levels 50 --seconds 30` at the start of the
day-five capture, with no `--every`: 330 snapshots, 33,000 rows, in thirty
seconds. The collector writes a snapshot only when the fifty levels it records
have changed; 132 more updates moved only deeper prices and were not written.
The five-second grid `--every 5` would have kept from the same thirty seconds
is six snapshots. Its curve coefficient is within 4% of the full-rate one, but
its average spread is a quarter of the full-rate figure and its queue
imbalance has the opposite sign: the top of the book moves faster than five
seconds.

`btc_usd_d6_book_full.ndjson` is the same thirty seconds at the start of the
day-six capture: 440 snapshots, 44,000 rows, 47 unchanged updates not written.
`xexec book-grid` thins both books from a quarter second to ten. On neither day
does the spread a grid sees settle as the grid tightens: on day six it is 0.61
of the full-rate figure at a quarter second, 3.60 at one second and 0.08 from
two seconds on, where every kept snapshot sits at a one-cent touch. The queue
imbalance keeps its sign at every spacing on day six (the ask queue is the
heavier, -0.297) and loses it from five seconds on day five.

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
- The deep book was cut to the six day-four sessions; the snapshots after the
  last whole bucket were left out.
- The full-rate books are the whole thirty-second captures, uncut. Each starts
  before the first session of its day, which opens on the next 10-second
  boundary.

## What these are, and are not

They are real trades, which the hand-written `data/sample_*.ndjson` captures are
not. They are also fifty minutes of one product: two weekday mornings, a
weekday afternoon, a Saturday morning and a Sunday morning. A "session" here is
a 100-second slice, not a trading day, and the six in each run are neighbours
in time, so they are far from independent. Five days is enough to show that a
result on one does not have to hold on another. It is not enough to say which
one is typical, and the five traded volumes thirty times apart. The ten-level book is one 100-second
stretch, so it says how shallow the touch was that morning, not how deep the
market is. The deep book is fifty levels sampled every five seconds over ten
minutes of a Sunday: deep enough that the sizes here fill, too sparse to see
what happens between two snapshots. The full-rate book shows what happens
between them, but only for thirty seconds. Use them to see how the
commands behave on real prices and sizes; do not read a result on them as a
property of the strategy.

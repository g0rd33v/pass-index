# Moving the collectors to Rust — the order of work

> **Closed on 2026-09-01.** The port is finished: every Python collector was
> ported and the `tools/index/` Python was deleted. This document is kept as the
> historical record of the order of work and how each step was accepted; the
> `.py` files it names no longer exist in the repository.


One language, one schema, the rules in one place. 29 files and 6,660 lines
remain; `resolve.py` is already across and accepted.

**How each step is accepted.** Not by reading the diff. The Python is run,
its answers are written to a file, the Rust is asked the same questions, and
the two are compared row by row. A step is done when they agree exactly —
that is how `resolve.rs` was taken, on 4,947 names with 0 disagreements. Each
step below names the comparison that settles it.

---

## Stage 1 — the rules that only touch the database

No network, no feeds, deterministic input. These are the ones that can be
compared exactly, and they are what the nightly run repairs the catalogue
with, so they are worth the most.

| # | file | lines | what settles it |
|---|---|---|---|
| 1 | `naming.py` | 86 | every entity and provider name, before and after, identical |
| 2 | `normalise.py` | 76 | the same corrections applied to a copy, row for row |
| 3 | `aliascheck.py` | 106 | the same aliases proposed and moved on a copy |
| 4 | `fold.py` | 265 | the same groups, the same survivor, the same counts attached |
| 5 | `sizes.py` | 104 | the same parameter count read off all 1,685 names |
| 6 | `tasks.py` | 105 | the same tags on the same things |
| 7 | `opaque.py` | 124 | the same rows called opaque |
| 8 | `free.py` | 121 | the same free lanes |
| 9 | `quarantine.py` | 189 | the same promotions and the same rejections |
| 10 | `check.py` | 665 | all 59 checks, finding for finding, on the live catalogue |
| 11 | `audit.py` | 315 | all 4,230 pages walked, the same verdicts |

**Exit:** the nightly repair stage runs entirely in Rust and produces a
catalogue byte-identical to the one Python produces from the same input.

## Stage 2 — one HTTP client, then the feeds that publish a rate card

Reading a JSON rate card is the same job twenty times. First the shared
piece, then the sources that use it, hardest first because that is where the
shape is decided.

| # | file | lines | what settles it |
|---|---|---|---|
| 12 | a fetch layer — timeouts, retries, the cache | — | the recorded responses replay identically |
| 13 | `gateways.py` | 142 | the same offerings and prices from the same saved responses |
| 14 | `sellers.py` | 296 | the same, across 203 providers |
| 15 | `free_tiers.py` | 278 | the same allowances, and none of them inside a price range |
| 16 | `subscriptions.py` | 247 | the same seat prices and what each includes |
| 17 | `boards.py` | 451 | all 137 boards, the same standings and field sizes |

**Exit:** a night's collection run from saved responses produces the same
prices, standings and offerings as Python does.

## Stage 3 — the readers of prose

These read sentences rather than tables — funding rounds, release dates,
descriptions — and each has a text parser that was tuned against real
sentences. They move last because a parser that drifts is silent.

| # | file | lines | what settles it |
|---|---|---|---|
| 18 | `facts.py` | 174 | the same dates and cutoffs from the same pages |
| 19 | `weights.py` | 99 | the same licences |
| 20 | `enrich.py` | 194 | the same descriptions and websites |
| 21 | `funds.py` | 218 | the same 1,634 investment edges |
| 22 | `startups.py` | 291 | the same companies marked venture-backed, the same rounds |
| 23 | `yc.py` · `dvc.py` | 283 | the same portfolios |
| 24 | `discover.py` | 178 | the same candidates into the pen |
| 25 | `newmodels.py` | 294 | the same new things |
| 26 | `license_search.py` | 57 | the same licences found |
| 27 | `terms.py` | 1007 | the same 120 terms, word for word |

**Exit:** a full night runs in Rust and the catalogue it writes matches the
one Python writes, table by table.

## Stage 4 — what is left over

| # | file | lines | note |
|---|---|---|---|
| 28 | `pipeline.py` | 256 | becomes the order of calls inside one binary, not a script |
| 29 | `export_kb.py` | 253 | only used by hand; last, or dropped if nothing needs it |

**Exit:** `tools/index/` is empty and `collect-daily.sh` calls one binary.

---

## What this costs and what it buys

Stage 1 is a third of the remaining lines and carries the rules that decide
what the catalogue says about itself. Stage 2 is the bulk of the value:
those five files are where every price comes from. Stage 3 is the slowest
per line, because prose parsers are where a silent drift hides.

The prize is that a rule lives in one place. Today `resolve` exists twice and
the two disagreed on 105 names until they were made to agree; every other
rule that gets copied will do the same.

---

## Stage 1 — done, 2026-08-31

Eleven files, 2,303 lines of Python, every one accepted by running both
against identical copies and comparing what they left behind.

| file | what settled it |
|---|---|
| `resolve` | 4,947 catalogue names, 0 disagreements |
| `naming` | 324 mends on a fixture of 606 broken names; entities and aliases identical |
| `normalise` | 106 rows corrected; entities and the run record identical |
| `aliascheck` | 121 aliases misfiled the way a feed does, both moved all 121 |
| `fold` | four counts and seven tables identical |
| `sizes` | 281 models, the same count and the same reading |
| `opaque` | 33 companies, 211 unjustified, the notes column identical |
| `tasks` | 1,605 tag lists identical, order included |
| `quarantine` | 131 released, 380 candidates, fields and bodies identical |
| `check` | a 91-line report, character for character; 38 verdicts |
| `audit` | 4,230 pages, 0 blocking, 19 verdicts identical |

Two differences in the page walk's report, both stated rather than papered
over. The five examples printed under a rule come out in a different order —
Python collects them from a thread pool and its own two runs disagree with
each other, while the Rust walks in sitemap order and is stable. And one
page's compressed size reads 102,638 against 102,923: two gzip
implementations pack the same bytes differently, which is 0.3% and could
matter only for a page sitting exactly on the 60 KB line.

`free.py` left this stage for the next: it fetches OpenRouter's feed, so it
belongs with the collectors.

---

## Stage 1 — done, 2026-08-31

Eleven files, 2,303 lines of Python, every one accepted by running both
against identical copies and comparing what they left behind.

| file | what settled it |
|---|---|
| `resolve` | 4,947 catalogue names, 0 disagreements |
| `naming` | 324 mends on a fixture of 606 broken names; entities and aliases identical |
| `normalise` | 106 rows corrected; entities and the run record identical |
| `aliascheck` | 121 aliases misfiled the way a feed does, both moved all 121 |
| `fold` | four counts and seven tables identical |
| `sizes` | 281 models, the same count and the same reading |
| `opaque` | 33 companies, 211 unjustified, the notes column identical |
| `tasks` | 1,605 tag lists identical, order included |
| `quarantine` | 131 released, 380 candidates, fields and bodies identical |
| `check` | a 91-line report, character for character; 38 verdicts |
| `audit` | 4,230 pages, 0 blocking, 19 verdicts identical |

Two differences in the page walk's report, both stated rather than papered
over. The five examples printed under a rule come out in a different order —
Python collects them from a thread pool and its own two runs disagree with
each other, while the Rust walks in sitemap order and is stable. And one
page's compressed size reads 102,638 against 102,923: two gzip
implementations pack the same bytes differently, which is 0.3% and could
matter only for a page sitting exactly on the 60 KB line.

`free.py` left this stage for the next: it fetches OpenRouter's feed, so it
belongs with the collectors.

---

## Done — 2026-09-01

Every stage is across and `tools/index/` is deleted. Each file was accepted
the way this plan demanded: two identical copies of the catalogue, the Python
run on one, the Rust on the other, and the rows compared exactly — startups
(13 companies identical), funds (1,491 investments, 100 fund rows), enrich
(the same 89 descriptions), yc (all 1,924 provider rows), discover, licences
(1,016 rows), export_kb (all 50 documents byte-for-byte). The nightly run
calls only Rust binaries, and `deploy/index/release.sh` no longer ships a
`tools/index/` that no longer exists.

Since the port, the same crate has grown past parity: a `current_prices`
view (the price shown is the price charged today), the `retire` job
(withdrawn listings come off the shelf), the `supply` module (the standard
door for outside suppliers — the crawler is the first), `people`, and the
`bang`/`news`/`for-agents` pages. The library test suite runs in acceptance
now — 72 tests — after three of the port's tests were found never to have
been executed at all.

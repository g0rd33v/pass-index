# Pass Index — architecture

What the catalogue is, how a figure gets in, and what the pages promise. Read
this before any module; the `//!` headers in each source file carry the local
detail, and the decision records in `docs/` carry the why.

## What it is

A catalogue of everything sold in the AI market: **things** (models, tools,
agents, subscriptions), the **companies** that make and sell them, **prices**
in integer micro-USD, **leaderboard standings**, **investments**, **people**,
and a **vocabulary**. One SQLite store at `/srv/pass-index/data/`, served by
the `serve` binary behind nginx at [pass.io/index](https://pass.io/index).

Two crates and no more: `crates/index` is the product; `crates/common` is a
generated copy of the shared library (the one SQLite door, error types,
tracing) held byte-identical to the `pass` monorepo — never edited here.

## How data gets in — four doors, and only four

Nothing writes identity directly; everything binds to an entity through the
resolver.

1. **The collectors** (`collect`) read sellers' own price documents directly.
2. **Two public price files** (LiteLLM, models.dev) add the resale market —
   they add few new things but many sellers, which is the point.
3. **A human** imports curated files or mints aliases by hand (`import`,
   `mint`, `data/curated/`).
4. **Outside suppliers** deliver JSONL findings through `supply` — the crawler
   ([pass-index-crawler](https://pass.io/index/crawler), its own service) is
   the first.

A supplier never mints identity: an unbindable name goes to the **quarantine
pen**, a figure the catalogue cannot state exactly is kept as a quoted
**evidence** line, and every delivery lands in one transaction, once, recorded
where `/index/coverage` shows it. A name nobody recognises is counted and
reported, never invented.

## What the pages promise

- **The price shown is the price charged today.** The `current_prices` view
  gives one row per (offering, dimension): the seller's own page outranks a
  third-party catalogue while it is fresh (45-day decay), else the newest
  reading wins.
- **A dropped listing comes off the shelf.** `retire` shelves an offering the
  collectors were reading but no longer see — but only after a *successful*
  collect, so a broken feed never marks live listings stale.
- **One standing per board**, the model's best configuration's, from the newest
  reading.
- **Every figure is sourced and dated.** No page asserts a number without the
  document and the read-date behind it.

## The nightly run

`deploy/index/collect-daily.sh` (cron 04:17 on the host):

```
collect → repair (sellers, boards, free, plans, new models, facts, weights,
          funding, people, naming, fold, quarantine, retire, …) →
check (consistency suite) → audit (walk every served page)
```

The night counts as clean only if **both** the consistency checks and the full
page walk pass; the run exits non-zero otherwise, so a broken night is mailed
rather than served. The public dataset
([pass-index-data](https://github.com/g0rd33v/pass-index-data)) is published
only on a clean night.

## Build, release, rollback

The build is **offline and off-host** — the production server never compiles
(an OOM incident closed that door by construction). `deploy/index/Dockerfile`
does `cargo build --release -p index --bins` in `rust:1.97`;
`deploy/index/release.sh` ships the exact tree to a build host, tags the image,
keeps the previous one, and swaps; a release that fails its own health check
swaps the previous image back itself. `rollback.sh` restores the previous image
and its tagged source in one command.

## History

All of it is Rust. The Python collectors that once lived in `tools/index/`
were ported file by file — each accepted only when the Rust reproduced the
Python's output row for row — and deleted on 2026-09-01. The acceptance record
is in [rust-port-plan.md](rust-port-plan.md); how this repository was split out
of the monorepo is in [pass-index-split.md](pass-index-split.md).

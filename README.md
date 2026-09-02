# Pass Index

**A catalogue of everything sold in the AI market** — live at
[pass.io/index](https://pass.io/index).

Models, tools, agents and subscriptions; the companies that make and sell
them; what each one costs, and who charges it; where each places on the public
leaderboards; who funded the companies and who runs them; and the vocabulary of
the field. One store, read from ~40 sources every night, every figure carrying
its own source and the date it was read.

This repository is the **code**. The catalogue itself is published as a daily
public dataset at
[github.com/g0rd33v/pass-index-data](https://github.com/g0rd33v/pass-index-data).

## What it is

A single SQLite store and the programs that fill it, keep it honest, and serve
it. Two crates, nothing else:

- **`crates/index`** — the product: the schema, the collectors, the repair
  jobs, the consistency checks, and the HTTP face.
- **`crates/common`** — a small shared library (the one SQLite door, error
  types, tracing). It is a **generated copy** from the `pass` monorepo, kept
  byte-identical; never edit it here.

### The binaries

| Binary | What it does |
|---|---|
| `serve` | The HTTP face — every `/index/…` page and the JSON behind them. |
| `collect` | Reads the price feeds and appends only what moved. |
| `repair` | One subcommand per nightly job — sellers, boards, free tiers, plans, new models, facts, weights, funding, people, naming, folding, quarantine, retire, and more. |
| `check` | The consistency suite — fails if the catalogue asserts something false. |
| `audit` | Walks the served pages a reader is actually handed (dead links, missing dates, title clashes). |
| `mint` | The quarantine queue: what the market shipped that the catalogue has not yet been taught to recognise. |
| `import` / `export` | Bulk load / dump the catalogue as JSON (`export` feeds the public dataset). |
| `exportkb` | Export the knowledge base. |
| `seed` | Seed a fresh database. |
| `resolvecheck` | Check name-resolution against known cases. |

## How a figure gets in

Nothing writes identity directly. A price, a standing or a fact enters through
one of two doors and is bound to an entity by the resolver:

- **`collect`** reads feeds that publish their own prices.
- **`supply`** (a `repair` subcommand) takes findings from the crawler
  ([pass-index-crawler](https://pass.io/index/crawler), its own service): exact
  dollar figures land as prices, quoted lines as evidence, board rows as
  standings, and a known maker's new model goes to the quarantine pen. A
  supplier never mints identity — a name nobody recognises is counted and
  reported, never invented.

## Build and deploy

The build is offline and off-host — the production server never compiles.
`deploy/index/Dockerfile` does `cargo build --release -p index --bins` inside
`rust:1.97`; `deploy/index/release.sh` ships the tree to the host and builds it
there, tags the image, keeps the previous one, and swaps. `rollback.sh` puts
the previous image and tagged source back in one command.

The nightly run is `deploy/index/collect-daily.sh` (cron 04:17 on the host):
collect → repair jobs → retire → `check` → `audit`. It exits non-zero if
anything blocked, so a broken night is mailed rather than served.

## Layout

```
crates/index/       the product (schema, collectors, repair, checks, serve)
crates/common/      generated copy of the shared library (do not edit)
deploy/index/       Dockerfile, release.sh, rollback.sh, collect-daily.sh
data/curated/       hand-written data re-applied nightly (free tiers, terms, …)
docs/               architecture, the split record, the crawler brief, glossary of the port
```

All of it is Rust. The Python collectors that once lived elsewhere were ported
and deleted; see [docs/rust-port-plan.md](docs/rust-port-plan.md).

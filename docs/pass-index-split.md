# How Pass Index became its own repository

This repository was split out of the `pass` monorepo on 2026-09-02. The split
was made by measurement, not guesswork.

## What the measurement showed

- `crates/index` uses exactly **one** of the shared crates: `common`. There is
  no `use console::/desk::/ledger::/planner::/ports::/retriever::/router::/cache::/verifier::`
  anywhere in `crates/index/src`.
- `index` + `common` together are all the product needs. The rest of the
  monorepo workspace — `pass-v1` and nine shared crates — was ~38,000 lines of
  dead weight here, and it was that copy that had forked.
- The build is offline: `deploy/index/Dockerfile` does `COPY . .` +
  `cargo build --release -p index --bins`, and `deploy/index/release.sh` rsyncs
  the tree to a build host and builds it there.

## How `common` is carried

`common` stays owned by the `pass` monorepo. A copy is mirrored into this
repository as an ordinary `path` dependency and is treated as **generated**:
it is never hand-edited here. The build does not change by a single line — the
Dockerfile and release scripts are untouched — and a fork is made impossible,
because the copy is regenerated and any drift shows up in one check.

A git dependency on the private monorepo was considered and rejected: the build
runs offline inside Docker, so a private git dependency would mean putting SSH
keys into the build container — a deploy change for the sake of how a dependency
is written. The generated-copy approach is the same one already proven with the
Pass Tools mirror.

## What this repository contains

**Kept**

- `crates/index/` — the product (11 binaries: serve, seed, export, collect,
  mint, import, repair, check, audit, exportkb, resolvecheck).
- `crates/common/` — the generated copy of the shared library; do not edit.
- `Cargo.toml` — `members = ["crates/index", "crates/common"]`, with
  `[workspace.dependencies]` trimmed to what the two members use.
- `deploy/index/` — Dockerfile, release.sh, rollback.sh, collect-daily.sh,
  docker-compose.yml, unchanged.
- `data/curated/`, index `docs/`, `README.md`, `CLAUDE.md`, `banana.md`,
  `rust-toolchain.toml`, `Cargo.lock`.

**Dropped**

- `crates/pass-v1`, `console`, `desk`, `ledger`, `planner`, `ports`,
  `retriever`, `router`, `cache`, `verifier` — the index never used them.
- `tools/` — a stale Python fork; the index is all Rust.
- `deploy/push.sh` — targets the `pass` production deploy; it has no business
  here (the index ships with its own `release.sh`).
- The root `Dockerfile` — the index builds with `deploy/index/Dockerfile`.

## Proof there is no duplicate

Compare the tree hash of the generated copy: `git rev-parse HEAD:crates/common`
here must equal the same path in `pass`. At the split it matched byte-for-byte.
If it ever diverges, the copy was hand-edited — regenerate it from `pass`.

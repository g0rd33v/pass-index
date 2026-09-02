# Pass Index — conventions

Rules for touching this repository. Load before changing code.

## Two crates, one of them generated

`crates/index` is the product. `crates/common` is a **generated copy** from the
`pass` monorepo — do not edit it here; changes are made in `pass` and re-mirrored,
and any drift is caught by comparing `git rev-parse HEAD:crates/common` against
the same path in `pass`.

## One SQLite door

Open every SQLite connection through `common::db::open` — nothing else. It sets
the busy-timeout and WAL pragmas a bare `Connection::open` omits (the omission
once caused a `SQLITE_BUSY` incident). One writer per table.

## Two write doors, resolver-bound

A figure enters only through `collect` (feeds) or `supply` (outside findings),
and binds to an entity through the resolver. Nothing mints identity outside that
path; an unbindable name goes to the quarantine pen, never into the catalogue.

## Everything is written in English

Docs, code comments, commit messages, check labels, log and alert text — all
English, always. The one exception is **linguistic data**: text that is the
subject under test rather than prose about it (detector vocabularies, benchmark
fixtures, quoted inputs/outputs in reports). Translating it deletes what it
verifies, so it stays verbatim; the prose around it is still English, and a
comment says why the data is not.

## Test discipline

- `cargo test -p index --lib` — the fast unit suite; run it before calling a
  change done. A green suite is the gate, memory is not.
- Full builds and the acceptance suites run **off-host** (a build container),
  never on a developer's machine and never on the production server.

Add a unit test next to every change; name tests by the feature they pin.

## Product

- The brand is exactly **Pass** — capital P, no lowercase, no dots.
- Answer honestly: never claim a figure the catalogue does not hold, never
  invent a source. Every number on a page carries the document and date behind
  it.

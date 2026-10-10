# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`j` is a CLI for Jujutsu (jj) repositories whose interface is a small, pure, dynamically typed functional language. A repository is a value (`Repo`, a zipper over the commit tree), a command is a function `Repo -> Repo`, and the whole user-facing vocabulary (`new`, `describe`, `squash`, `tree`, …) is written *in the language* in `config.j`. The Rust binary supplies only the language, the builtins `config.j` declares, ten reserved commands, and the bridge to jj's on-disk format via `jj-lib` (no `jj` binary needed at runtime).

## Commands

```
cargo build                          # debug build -> target/debug/j
cargo test                           # whole suite, ~20s once compiled
cargo test --test tree               # one test file (tests/tree.rs)
cargo test --test cli squash         # tests in one file matching a name
nix build                            # release build + difftastic wrapper -> ./result/bin/j
```

There is no devShell in `flake.nix`; `cargo`/`rustc` come from the system. The Nix build sets `doCheck = false` because the tests shell out to `git` and need a writable temp dir, so `cargo test` is the only place tests run.

- `tests/cli.rs`, `tests/remote.rs`, `tests/interop.rs` run the built binary (`CARGO_BIN_EXE_j`) against real repos in temp dirs, each with its own `XDG_CONFIG_HOME`. They need `git` on `PATH`; `interop.rs` also uses the real `jj` binary and skips itself if it is absent.
- Everything else runs the interpreter over the in-memory `MemBackend` — no repository involved.
- `tests/laws.rs` and `tests/showprop.rs` are proptest suites; their `*.proptest-regressions` files are checked in on purpose.
- `difft` is only needed at runtime for `review`/`difft`. `J_GIT` overrides the `git` executable used for fetch.

## The spec is normative

`specs/main.md` is the specification the implementation was written from, and the code cites it everywhere as `§N.M` (e.g. `§7.5` = persistence). When changing behaviour, find the section first, and keep code, spec, and tests agreeing. `specs/tree.md` is a newer, complete replacement for `§7.11` (the "rails" tree layout) and is what `render.rs` implements.

Known drift to be aware of (not to copy):
- `§9` of the spec is a verbatim copy of `config.j`, but `config.j` is ahead of it (`touchedPaths`, the extra `treeWith` options, `treeCompact`, `treeData`). **`config.j` is the source of truth**; `docs/base.md` lags the same way.

## Architecture

One run of `j EXPR` (`src/main.rs::run_expression`, mirroring spec §1.2): load + validate `config.j` → open the repo and take the lock → parse the expression → build the `Repo` value, snapshotting the working directory → resolve `@id` literals → evaluate the config's definitions, then the expression → if the result is a function, apply it to the repo **once** → if the final value is a `Repo` that differs from the one loaded, persist it; otherwise display it and persist nothing. Exit codes: 1 crash, 2 usage/no repo/locked, 3 config or parse error. All work happens on a thread with a 512 MB stack (`main`).

Language pipeline, all in `src/`:
- `lex.rs` → `parse.rs` → `ast.rs`: the parser implements the two layout rules and needs the set of global names (`Config::global_names`) so expressions cannot shadow them.
- `eval.rs`: strict, call-by-value, **trampolined** CEK-style machine (`State`/`Cont`) so deep recursion in user code cannot overflow the native stack. Keep new evaluation paths on the trampoline rather than recursing in Rust.
- `value.rs`: `Value`. Two kinds of laziness exist purely for large-repo performance and must stay invisible to the language: `Value::Thunk` (a commit's `files` list, forced by `Value::field`/`value_eq`) and `BlobContent::Lazy` (file bytes; lazy blobs compare by content id without being read). Prefer comparing ids/tree hashes over forcing content.
- `shape.rs`: shapes from `config.j` type declarations, and runtime **contracts** compiled from signatures — checked at every call, one level deep (kind, list-ness, field names). Records are structural: a value *is* a `Commit` iff its field set matches.
- `builtins.rs`: native implementations of the names `config.j` declares without defining.
- `config.rs`: parses and validates `config.j`, orders definitions by dependency, resolves `@id` literals. Four definitions are read by the binary itself: `user`, `tree`, `immutable`, `labelled`.

`help [NAME]` and `commands [QUERY]` are reserved discovery commands; `--help` and `-h` alias `help` only as the first word. They never open a repository or evaluate config definitions. General and reserved-command help work without config; the custom vocabulary reads config without writing it, using the embedded reference only when no file exists. Keep these paths usable outside a repository and when general help is needed to recover from a broken config.

Repository side:
- `domain.rs`: the `Backend` trait — everything repository-shaped the interpreter needs — plus `MemBackend` (the test double required by spec §10) and snapshot helpers shared by both backends.
- `jj.rs`: `JjBackend` on `jj-lib`/`gix`: locating the repo, building the `Repo` value, `persist`, the lock, and the reserved commands (`init`, `clone`, `remote`, `fetch`, `push`, `undo`, `redo`, `ops`). Reserved commands are matched on the first word in `main.rs` before any parsing and are not part of the language.
- `repo.rs`: zipper navigation (`by`) and the persistence validation shared by the `validate` builtin and `JjBackend::persist`, which is what makes `tree . validate . EDIT` an honest dry run.
- `render.rs`: value display (§5.1) and tree rendering. `show.rs`: `show` (must round-trip through the parser — property-tested), unified diff, and the `difft` subprocess bridge.

### `config.j` is code, and it is embedded

`config.j` is `include_str!`'d into the binary (written to `~/.config/j/config.j` only when no user config exists) and into most test files, so editing it rebuilds both and every test exercises the reference config unmodified. An existing user config is never updated: a change that the binary depends on (a new builtin declaration, a new required `treeWith` option) breaks existing user configs until they are edited by hand.

Adding a builtin touches three places that must agree:
1. `all_builtins()` in `src/builtins.rs` — name, arity, implementation.
2. `RESERVED_BUILTINS` in `src/config.rs` — otherwise validation rejects the declaration as "declared but is not a builtin and has no definition".
3. A signature-only declaration in `config.j` (`name : Type`); the signature becomes the builtin's contract.

Prefer defining things in `config.j` in the language; add a native builtin only when it needs the backend or the in-language version is too slow (see `touchedPaths`).

## Tests

Interpreter-level tests share a pattern: `make_interp()` loads the reference config into an `Interp` over `MemBackend`, and an `ev(src)` helper parses, resolves ids, and evaluates. `Repo` values are built directly as records with small `commit(...)`/`entry(...)` helpers. Law-style properties from spec §8 belong in `tests/laws.rs`; end-to-end behaviour that involves snapshotting, persistence, or the operation log belongs in `tests/cli.rs`.

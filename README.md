<p align="center">
  <img src="logo.png" alt="j logo" width="200">
</p>

<p align="center"><em>Your repo is a function (not really)</em></p>

**`j` is a functional-programming-centered command-line interface to Jujutsu
(jj) repositories.** Instead of subcommands and flags, you write expressions in
a small, pure functional language that transform the repository. The repository
is a *value*, a command is a *function from repository to repository*, and
everyday work is done by composing those functions.

`j` is not a new version-control system. The storage, data model, and on-disk
format are jj's (which is git-compatible — `j` works directly on a colocated
`.jj`/`.git` repository, with no `jj` binary required). What `j` replaces is
the *interface*.

```
j 'describe "wip"'              -- set the focused commit's message
j 'new'                         -- start a new commit on top
j 'describe "wip" . new'        -- compose: new commit, then describe it
j squash                        -- fold the focus into its parent
j 'tree'                        -- print the history tree
```

The whole vocabulary — `new`, `describe`, `squash`, `tree`, `rebase`, … — is
written *in the language* in `config.j`, and you extend it by writing more
functions.

---

## Why `j`?

| | |
|---|---|
| **No staging area** | Like jj, the working copy *is* a commit; there's no `git add` step. |
| **Stable change ids** | A commit keeps its id (`@wqzt…`) across `describe`, `squash`, `rebase`. History editing is safe to talk about. |
| **First-class undo** | Every operation is recorded. `j undo` / `j redo` / `j ops`, not a forensic reflog. |
| **Composable commands** | A command is a function. `describe "wip" . new . (goto %main)` is one expression, not three commands. |
| **Programmable** | The vocabulary is `config.j`, written in the language. Add your own edits, revsets, and inspections. |
| **Total operations** | A conflicting `replay` produces an unresolved blob, not a halt. Resolve whenever you like. |

---

## Installation

### With Nix (recommended)

The repository provides a flake with one default binary, `j`. Difftastic is a
runtime dependency and is wired in automatically.

```
nix build              # builds ./result/bin/j
nix run                # run j directly
```

The reference `config.j` is installed alongside the binary
(`<prefix>/share/j/config.j`) and used if you have no user config.

### From source

```
cargo build --release   # produces target/release/j
```

You'll want the reference `config.j` (see next section) and, for `review`/`difft`,
[difftastic](https://difftastic.wilfred.me.uk/) on your `PATH`.

---

## Configuration

`j` reads `~/.config/j/config.j` on every run (or `$XDG_CONFIG_HOME/j/config.j`).
An expression can only use a name this file declares or defines. The reference
[`config.j`](config.j) ships with the binary — copy it and edit at least the
identity:

```
user = { name = "Your Name", email = "you@example.com" }
```

The reference config defines the entire base vocabulary (see
[docs/base.md](docs/base.md)). Four definitions are read by `j` itself: `user`
(who commits), `tree` (how a `Repo` prints), `immutable` (which commits may
never be rewritten), and `labelled` (what a `%name` literal means).

---

## How it works

Every `j` run:

1. Snapshots the working directory into the working-copy commit.
2. Builds a `Repo` value focused on that commit.
3. Evaluates your expression.
4. If the result is a function, applies it to the `Repo`.
5. If the final value is a `Repo` that changed, persists it; otherwise prints
   the value and changes nothing.

So most of what you write is an **edit** — a function `Repo -> Repo`. A result
that's plain data is printed and persists nothing (a dry run).

```
j 'length . commits . top'    -- a number: how many commits? prints, persists nothing
j squash                      -- an edit: persists the squash
j 'tree . squash'             -- a Text: shows what squash *would* do, persists nothing
```

The repository is applied **implicitly** — there's no `repo` variable; write a
function and it receives the repository.

---

## Frequent examples

Each of these is a complete command you can run. Edits persist; inspections
print.

### Start and describe work

```
j 'new'
```

Starts an empty child of the focused commit and moves to it. Edit files in
your editor, then:

```
j 'describe "add the lexer"'
```

Sets the message of the focused commit. The two compose:

```
j 'describe "add the lexer" . new'
```

Opens a new commit *and* describes it, in one step.

### See where you are

```
j 'status'
```

A summary of the focus: id, message, labels, changed paths, conflicted paths.

```
changed    0 items
conflicts  0 items
id         zrkmkkvrvtokumkuzrkmkkvrvtokumku
labels     0 items
message    add a library
```

```
j 'files'
```

The files of the focused commit, with sizes.

```
j 'changed'
```

The paths whose content differs between the focus and its parent.

### See the history

```
j 'tree'
```

The history as a tree, focused commit marked:

```
   ⌂ zzzz
  └─  ● rrtv ▂
     └─  ● xqsr ▂  start the project
▶       └─  ◌ zrkm  add a library
```

```
j 'treeFull'
```

The same tree with ages and authors in the margin.

```
j 'log'
```

A table of every commit, top first, with a `✓` on the focus.

```
j 'review'
```

The focus's changes, rendered by difftastic as one page of diffs.

### Move around

```
j 'prev'          -- to the parent
j 'next'          -- to the only child
j 'top'           -- to the top of history
j 'tip'           -- to the tip of the current line
j 'by @wqzt'      -- to a specific commit, by id
j 'goto %main'    -- to what the remote calls main
```

### Undo and history of actions

```
j undo            -- revert the last j expression
j redo            -- reapply it
j ops             -- the operation log, newest first
```

### A safe dry run

Because a `Text` or `Repo`-returning-but-unchanged result persists nothing,
you can preview any edit by composing it with `tree` (or `validate`):

```
j 'tree . squash'                -- show the tree as squash would leave it
j 'tree . validate . squash'     -- the same, but crash if squash couldn't persist
```

`validate` returns the repo unchanged if persisting would succeed, and crashes
with the exact error otherwise — an honest dry run.

---

## Editing history

These are the everyday history edits. Each is an `Edit`.

| command | what it does |
|---|---|
| `j 'describe "…"'` | set the focus's message |
| `j new` | start an empty child of the focus |
| `j squash` | fold the focus entirely into its parent |
| `j abandon` | drop the focus; its children join the parent |
| `j 'split <fileset>'` | push matching files up into a new child commit |
| `j 'contract <fileset>'` | push matching files down into the parent |
| `j 'rebase <revset>'` | move the focused subtree onto another commit |
| `j 'pick <revset>'` | copy the focus's change onto another commit |
| `j 'backout <revset>'` | a new commit undoing another commit's change |

### `squash` — fold a commit into its parent

```
j squash
```

Given `base work` → `more work`, after `j squash` the `more work` commit is
gone, its files folded into `base work`, and a fresh empty commit is the
focus:

```
   ⌂ zzzz
  └─  ● opwp ▂
     └─  ● zqsq ▂  base work
▶       └─  ◌ nttk
```

### `abandon` — drop a commit

```
j abandon
```

Removes the focus and rebases its children onto the parent. Refused for a
commit the remote has a name for (push something else under that name first).

### `split` / `contract` — move changes by fileset

A **fileset** is a function `Path -> Bool`. `split` pushes the matching files
up into a new child commit; `contract` pushes them down into the parent.

```
j 'split (ext "rs")'
```

Takes a commit touching `a.rs` and `b.j`, and splits it: the `.rs` change
becomes a separate child commit (now the focus), the `.j` change stays behind.

```
j 'contract everything'
```

Pushes *all* of the focus's files into the parent (the inverse of `split`).
`squash` is just `abandon . contract everything`.

Filesets compose: `under ./src`, `ext "rs"`, `both`, `either`, `neg`,
`everything`. So `split (both (under ./src) (ext "rs"))` splits off only the
Rust files under `src/`.

### `rebase` / `pick` / `backout`

```
j 'rebase %main'      -- move the current stack onto main
j 'pick @wqzt'        -- copy @wqzt's change here as a new commit
j 'backout @wqzt'     -- a new commit undoing @wqzt's change
```

`rebase` moves the focused subtree (and crashes if the destination is inside
it). `pick` copies a change without moving anything. `backout` replays a
change's *inverse*.

---

## Labels and the remote

A **label** is a bookmark name on the remote, shown on a commit's `labels`.
Labels are read-only in the language — only `fetch` and `push` change them.

```
j 'remote URL'                  -- set origin's URL
j fetch                         -- fetch from origin
```

`j push EXPR` evaluates `EXPR` to a list of push/drop records:

```
j 'push (label "feature" here)'         -- point the "feature" bookmark at the focus
j 'push (rename "old" "new")'           -- rename a bookmark on the remote
j 'push (unlabel "old-feature")'        -- delete a bookmark
j 'push relabel'                        -- after a rewrite, re-point the remote's bookmarks
```

---

## Advanced examples

These show the language doing things that take several commands (or aren't
possible) in other tools.

### Describe every commit in a stack at once

```
j 'forEach descendants (describe "wip")'
```

`forEach` is an ordinary function `Revset -> Edit -> Edit`: it runs an edit at
every commit a revset names, returning to the focus. This one marks every
commit below the focus `wip`.

### Find commits by any predicate

```
j 'matching (\c -> c.message == "wip") all'
```

`matching p rs` keeps the commits of revset `rs` satisfying predicate `p`, as a
revset — here, every commit whose message is `wip`. Since the result is a list
of commits, it prints (and persists nothing).

### Count things

```
j 'length . commits . top'
```

Composes three functions into `Repo -> Int`: the number of visible commits.

### An honest dry run of a complex edit

```
j 'tree . validate . rebase %main'
```

Shows the tree as it would look after rebasing onto main, *without persisting
anything*. And because `validate` crashes with the exact error persistence
would raise, this is honest: if the rebase couldn't actually happen (say `%main`
doesn't exist yet, or the destination is inside the moved subtree), you see
that crash here instead of after a real attempt.

```
j 'tree . validate . squash'
```

The same idea for a simpler edit: renders the post-squash tree, and a following
`j tree` confirms nothing changed.

### Run an edit elsewhere, then come back

```
j 'at %main (describe "released")'
```

Describes `main`, then returns the focus to where it was. `at rs e` runs edit
`e` at the commit revset `rs` names and comes back by id.

### Combine edits into a single operation

```
j 'describe "hotfix" . new . (goto %main)'
```

Go to main, open a new commit, describe it — one expression, one recorded
operation. With `or` you get a fallback:

```
j 'rebase %main or id'
```

Rebase if you can; leave the repository alone if it crashes.

### Define your own command

Anything added to `config.j` whose value is an `Edit` is a new command:

```
-- in config.j: fold the whole stack into one commit called "release"
release : Edit
release = describe "release" . (contract everything)
```

Then `j release` works like any built-in.

---

## Reserved commands

A handful of commands are built in (not expressions): they create or connect
repositories, move refs, and walk the operation log. None take flags.

| command | what it does |
|---|---|
| `j init` | create a colocated `.jj`/`.git` repository here (adopting an existing `.git`) |
| `j clone URL [DIR]` | clone `URL` into `DIR` (default: the repository's name, as `git clone` picks it) |
| `j remote URL` | set `origin`'s URL |
| `j fetch` | fetch from `origin` |
| `j push EXPR` | evaluate `EXPR` to push/drop records and push them |
| `j undo` / `j redo` | walk the operation log |
| `j ops` | print the operation log |

Everything else is an expression in the language.

---

## Documentation

| document | contents |
|---|---|
| **[docs/tutorial.md](docs/tutorial.md)** | A guided first hour: the mental model (implicit repo application, edits, revsets, filesets), dry runs, `or`, history editing, batch operations, and writing your own commands. The best place to start learning. |
| **[docs/language.md](docs/language.md)** | The `j` language in detail: values, syntax, semantics, builtins, contracts, `show`, the `Repo` zipper, and many examples. Start here to learn the language. |
| **[docs/base.md](docs/base.md)** | The complete base vocabulary: every builtin and every `config.j` type and function, grouped logically, each with a description and examples, plus an alphabetical index. |
| **[docs/comparison.md](docs/comparison.md)** | A structured three-way comparison of git, jj, and `j`: concept mappings, side-by-side command tables, and a full workflow in each. |
| **[SPEC.md](SPEC.md)** | The normative specification of the language, the CLI, and repository semantics. |
| **[config.j](config.j)** | The reference configuration: the entire base vocabulary, written in the language. |

---

## Project layout

```
config.j      the reference base vocabulary (read on every run)
src/          the j implementation (Rust, on jj-lib)
tests/        unit, property, and CLI end-to-end tests
docs/         language.md, base.md, comparison.md
SPEC.md       the specification
flake.nix     Nix package: one binary, j, with difftastic as a runtime dep
```

### Development

```
cargo build          # build
cargo test           # run the test suite
```

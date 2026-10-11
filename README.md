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

## Find a command

```
j help                  -- a quick start, grouped commands, and examples
j commands              -- common tasks and how to do them
j commands all          -- the full vocabulary in your config
j commands rebase       -- search names, signatures, and descriptions
j help describe         -- see a command's signature, documentation, and examples
j help push             -- learn a reserved command's usage
j help config           -- find your config and set your identity
```

`j --help` and `j -h` also open help. A separate trailing help argument, such
as `j abandon --help`, opens that command's help without executing it.
Help and command discovery work outside a repository and never change your
files or configuration. The catalog includes
your own definitions, even ones without a signature; when no config exists, it
uses the embedded reference config. General help and help for reserved commands
are available even if your config is broken.

Set up with your own name and email, then choose one starting point:

```sh
j setup 'Ada Lovelace' ada@example.org

# Start a new project
mkdir project && cd project
j init
```

```sh
# Or join an existing project
j clone URL project
cd project
```

`init` and `clone` leave you on a working-copy commit: the commit you are
editing now. Edit files, then run `j 'describe "first change"'` to save the
files and their description. Use `j save` to save files without changing the
description. Run `j new` **before** editing the next change. `j status` shows
both changes in the commit and edits that still need saving; `j tree` shows
where that commit sits in history. Compose commands right to left:
`j 'describe "first change" . new'` creates a commit, then describes it.
Preview an edit with `j 'preview squash'`. Search familiar tasks with
`j commands commit`, `j commands checkout`, or `j commands branch`.

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
runtime enhancement and is wired in automatically.

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

Run `j setup` to create the reference `config.j` (see next section).
[Difftastic](https://difftastic.wilfred.me.uk/) on your `PATH` improves
`review`/`difft`; without it they use a unified diff.

---

## Configuration

### Terminal colors and motion

In an interactive terminal, `j` uses vivid branch colors, a blue focus band,
colorful data and status messages, and animated progress indicators while
repository operations or expressions run. Each indicator names the current
operation and shows a spinner, a moving activity bar, and elapsed time. The
bar shows activity, not a completion percentage; on narrow terminals it makes
room for the operation's name. Colored trees and tables appear in a quick
cascade. Progress starts only when an operation takes a moment; fast commands
do not wait for a spinner.

To keep the colors but turn off animation:

```sh
J_ANIMATIONS=never j tree
```

Set `NO_COLOR=1` for plain, motion-free output. Motion is also disabled when
either output stream is redirected, `TERM=dumb`, or `CI` is nonempty. Automatic
colors follow the terminal for each stream; piped output stays plain unless
you explicitly request `treeWith`'s `color = "always"`. `NO_COLOR` takes
precedence even over that setting.

Interactive edits report `Saved changes` or `No changes`, followed by the
focused commit's short id and description. `NO_COLOR=1` and `TERM=dumb` keep
these confirmations in plain text. Success confirmations are suppressed when
either output stream is redirected or CI is set; warnings and errors still
appear.

### User config

`j setup 'Your actual name' you@your-domain.example` creates the full reference
config with your identity. `j setup` without arguments creates an editable
default and tells you where to set the identity. Neither form overwrites an
existing file; use `j help config` to find and edit it.

Expressions read `~/.config/j/config.j` (or `$XDG_CONFIG_HOME/j/config.j`).
An expression can only use a name this file declares or defines. The reference
[`config.j`](config.j) ships with the binary. Commands that execute against a
repository create an editable copy there when it is missing; help and discovery
use the embedded copy without writing it. Edit at least the identity:

```
user = { name = "Ada Lovelace", email = "ada@example.org" }
```

Use your own name and email. `j init`, `j clone`, and edits that save changes
reject empty fields and the shipped `Your Name` / `you@example.com`
placeholders. Interactive setup confirms the identity it used. `j help config`
shows the actual config path and remains available when the file is broken.

Existing configurations keep their own vocabulary. To adopt new helpers,
copy the `save` and `preview` definitions, the `unrecorded` declaration, and
the updated `status` definition from the [reference config](config.j), preserving your
identity and custom definitions. `j setup` does not upgrade existing files.

The reference config defines the entire base vocabulary (see
[docs/base.md](docs/base.md)). Four definitions are read by `j` itself: `user`
(who commits), `tree` (how a `Repo` prints), `immutable` (which commits may
never be rewritten), and `labelled` (what a `%name` literal means).

---

## How it works

Every expression run:

1. Snapshots the working directory for evaluation.
2. Builds a `Repo` value focused on the working-copy commit.
3. Evaluates your expression.
4. If the result is a function, applies it to the `Repo`.
5. If the final value is a `Repo` that changed, persists it. An unchanged
   `Repo` saves nothing; other values print without saving.

So most of what you write is an **edit** — a function `Repo -> Repo`. A result
that's plain data is printed and persists nothing (a dry run).
`j status` sees current file edits but does not record them. Use `j save` to
record those edits without changing anything else before pushing.

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

A summary of the focus: id, message, labels, changed paths, unrecorded edits,
and conflicted paths. `in commit` (the `changed` field) compares the commit
with its parent;
`unrecorded` compares the current files with their last saved state. After
`j save`, a commit can still have changed paths while all edits are recorded.

```
in commit   No changes
conflicts   No conflicts
id          zrkmkkvrvtokumkuzrkmkkvrvtokumku
labels      No labels
message     add a library
unrecorded  All edits recorded
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

A table of every commit, top first, with a `✓` on the focus, short ids, and
label names. Tables fit the terminal width, switching to fields on separate
lines on very narrow terminals. Redirected output preserves full cell text.

```
j 'review'
```

The focus's changes as one page of diffs, using difftastic when available
and a unified diff otherwise.

### Move around

Navigation checks out the selected commit's files so you can edit them.

```
j 'prev'          -- to the parent
j 'next'          -- to the only child
j 'tip'           -- to the tip of the current line
j 'by @wqzt'      -- to a specific commit, by id
j 'new . goto trunk' -- start a change on top of the protected main line
```

Use `j 'tree . goto trunk'` to inspect main without moving the working copy.
At a fork, use an id from `j tree` with `by`; `next` requires one child.
Main must exist on the remote (`j fetch`) before `trunk` can select it.

### Undo and history of actions

```
j undo            -- revert the last j expression
j redo            -- reapply it
j ops             -- the operation log, newest first
```

Undo and redo require a working copy with no unrecorded edits. If you run
`j id` to record edits first, that creates a new operation: the next `j undo`
undoes the recording, and another undo reaches the preceding edit. A new
recording also clears pending redos. Inspect `j ops` when deciding how far to
go back. Interactive confirmations name the affected operation and restored
focus.

### A safe dry run

`preview` validates an edit and returns its resulting tree without saving:

```
j 'preview squash'
j 'preview (rebase trunk)'
```

This is shorthand for `tree . validate . EDIT`. `tree . EDIT` also previews,
but does not check whether the result can be saved. `validate . EDIT` alone
**saves** a changed repository; validation is not a dry-run mode.

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

Push sends recorded content. If files have changed since the last saved edit,
it warns that those changes are omitted, then sends the recorded commits.
Run `j save` and push again to include them; `j status` alone does not record
changes.

For the first publication, choose a bookmark name with
`j 'push (label "feature" here)'`. `j push relabel` updates existing bookmarks;
it does not create the first one. Push reports when no updates were selected,
and gives counts of published, deleted, and unchanged bookmarks for a
nonempty selection in an interactive terminal.

### Resolve conflicts

A history edit can succeed with conflicts. Run `j status` to find conflicted
files, inspect them with `j review`, and edit the files to remove the conflict
markers and keep the intended content. Run `j save`, then `j status` again
to check for `No conflicts` and `All edits recorded`. There is no separate
continue command. `j conflicts` lists conflicted commits across history;
use `j 'by @id'` to work on another mutable commit when needed, and check
`j conflicts` again after resolving it.

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
-- in config.j: squash into the parent and name the combined commit "release"
release : Edit
release = describe "release" . prev . squash
```

Then `j release` works like any built-in.

---

## Reserved commands

A handful of commands are built in (not expressions): they provide help,
create or connect repositories, move refs, and walk the operation log.

| command | what it does |
|---|---|
| `j help [NAME]` | show a quick start or help for a reserved command or config name |
| `j commands [QUERY]` | show common tasks, search by task/name, or use `all` for the full vocabulary |
| `j setup [NAME EMAIL]` | create the full reference config, optionally with your identity; never overwrite an existing file |
| `j init` | create a colocated `.jj`/`.git` repository here (adopting an existing `.git`) |
| `j clone URL [DIR]` | clone `URL` into `DIR` (default: the repository's name, as `git clone` picks it) |
| `j remote URL` | set `origin`'s URL |
| `j fetch` | fetch from `origin` |
| `j push EXPR` | evaluate `EXPR` to push/drop records and push them |
| `j undo` / `j redo` | walk the operation log |
| `j ops` | print the operation log |

`--help` and `-h` open help as the first word or as a separate argument after
the command: `j --help describe`, `j describe --help`, and `j describe -h`
all open help for `describe`. Inside a single expression argument or stdin,
language comments still apply: `j 'new --help'` executes `new`. There are no
other general command flags. Everything else is an expression in the language.

Quote literal reserved-command arguments separately to keep spaces intact:
`j clone URL 'my project'` and `j remote '/path/to/shared repo.git'`. A whole
reserved command supplied as one string or on stdin is split on whitespace.

---

## Documentation

| document | contents |
|---|---|
| **[docs/tutorial.md](docs/tutorial.md)** | A guided first hour: the mental model (implicit repo application, edits, revsets, filesets), dry runs, `or`, history editing, batch operations, and writing your own commands. The best place to start learning. |
| **[docs/language.md](docs/language.md)** | The `j` language in detail: values, syntax, semantics, builtins, contracts, `show`, the `Repo` zipper, and many examples. Start here to learn the language. |
| **[docs/base.md](docs/base.md)** | The complete base vocabulary: every builtin and every `config.j` type and function, grouped logically, each with a description and examples, plus an alphabetical index. |
| **[docs/comparison.md](docs/comparison.md)** | A structured three-way comparison of git, jj, and `j`: concept mappings, side-by-side command tables, and a full workflow in each. |
| **[specs/main.md](specs/main.md)** | The normative specification of the language, the CLI, and repository semantics. |
| **[config.j](config.j)** | The reference configuration: the entire base vocabulary, written in the language. |

---

## Project layout

```
config.j      the reference base vocabulary
src/          the j implementation (Rust, on jj-lib)
tests/        unit, property, and CLI end-to-end tests
docs/         language.md, base.md, comparison.md
specs/        the language, CLI, repository, and tree specifications
flake.nix     Nix package: one binary, j, with difftastic as a runtime dep
```

### Development

```
cargo build          # build
cargo test           # run the test suite
```

# A tutorial on `j`

This is a guided first hour with `j`. It builds the mental model piece by
piece — not just *what* to type, but *why it works* — so that by the end you
can compose your own commands instead of memorizing them.

You'll want `j` installed (`nix build` or `cargo build --release`). Set up
your configuration with your own name and email:

```
j setup 'Ada Lovelace' ada@example.org
```

This creates the full reference configuration without overwriting an existing
file. If you already have a config, `j help config` shows its location so you
can edit the `user` definition. The remaining commands run in a real
repository, so you can follow along.

---

## Part 1 — Ten minutes to a working repository

Make an empty directory and create a repository:

```
mkdir play && cd play
j init
```

`j init` makes a colocated `.jj`/`.git` repository and checks out a fresh
working-copy commit: the commit you are currently editing. It already exists,
so write a file and describe that commit:

```
echo 'fn main() {}' > main.rs
j 'describe "start the project"'
```

Look at what you have:

```
j 'tree'
```

```
   ⌂ zzzz
▶ └─  ◉ pnyz ▂  start the project
```

Add a second change on top:

```
j 'new'
echo 'lib' > lib.rs
j 'describe "add a library"'
j 'tree'
```

```
   ⌂ zzzz
  └─  ● pnyz ▂  start the project
▶    └─  ◉ psnq ▂  add a library
```

That's the basic loop: **edit files, describe the current change, then `new`
before editing the next change.** `new` opens an empty change with the same
files as its parent; it does not move existing edits into that new change.
There is no `git add` step. `describe` saves both the current files and their
description; `j save` records file edits without changing the description.

**What just happened, in the model:** each expression snapshots your working
directory for evaluation, builds a `Repo` value focused on that commit,
evaluates your expression, and — because `new` and `describe` are *edits*
(functions `Repo -> Repo`) — persists the resulting repository.

`j status` has two useful lists: `in commit` compares the current commit with
its parent, while `unrecorded` lists file edits made since the last save.
After `describe` or `save`, `unrecorded` is empty; `in commit` still shows the
work in the commit. Inspecting status does not save anything.

To join an existing project instead, clone it and enter its directory:

```
j clone URL project
cd project
j status
```

---

## Part 2 — The one idea everything rests on

Here is the single most important thing to understand about `j`:

> **If your expression evaluates to a function, that function is applied to
> the repository. There is no `repo` variable — the repository is the implicit
> argument.**

`new`, `describe`, `prev`, `squash` are all functions `Repo -> Repo` (called
**edits**). When you run `j squash`, you're not invoking a subcommand; you're
evaluating the name `squash` to a function, which `j` then applies to your
repository.

This is why composition works. `.` is ordinary function composition, `(f . g)
x = f (g x)` — **right to left**:

```
j 'describe "wip" . new'
```

reads as "first `new`, then `describe`," because `(describe "wip" . new) repo =
describe "wip" (new repo)`. Run it and you've opened a new commit and set its
message, in one expression, as one recorded operation.

And it's why *inspections* are just functions too. `length . commits . top`
composes three functions into one `Repo -> Int`:

```
j 'length . commits . top'
    => 5
```

- `top` moves the focus to the top of history,
- `commits` lists every commit of the (sub)tree,
- `length` counts them.

Since the result is an `Int` (not a `Repo`), `j` prints it and **persists
nothing**. That's the rule: a `Repo` result persists; anything else prints.

> **Two outcomes.** A run either **persists** (the result is a `Repo` that
> differs from the loaded one) or **prints** (any other value). Edits persist;
> inspections print.

### A subtlety that trips people up

`j 'show (length . commits . top)'` does *not* print `5`. It prints the
function:

```
(.) ((length)) (((.) ((commits)) ((top))))
```

Why? `show (...)` applies `show` to the *function value* `length . commits .
top`, producing a `Text` *without ever applying it to the repository*. To get
the count, you want the composition itself applied: `length . commits . top`
(with no `show`). The result is a function `Repo -> Int`, `j` applies it, and
prints `5`. `show` is for rendering a *value*; here you want the value the
function produces, so don't wrap it.

---

## Part 3 — Reading and moving around

Make a few commits to play with:

```
j new; echo a > a.txt; j 'describe "A"'
j new; echo b > b.txt; j 'describe "B"'
j new; echo c > c.txt; j 'describe "C"'
```

Navigation changes the working-copy commit and checks out its files. Use
these commands to edit another mutable commit:

```
j 'prev'          -- to the parent
j 'next'          -- to the only child
j 'tip'           -- to the end of the current line
j 'by @vqok'      -- to a specific commit, by id (use a real id from j tree)
```

At a fork, choose an id from `j tree` with `j 'by @id'`; `next` only works
when there is one child. Main and the root commit are protected from editing.
Use `j 'new . goto trunk'` to start work on main, or
`j 'tree . goto trunk'` to inspect it without moving your working copy.
In a new local repository with no main bookmark, keep working from `j init`'s
commit instead.

After `j 'prev'` from C, the tree shows the focus (▶ and ◉) on B:

```
   ⌂ zzzz
  └─  ● qmwp ▂
     └─  ● nnvs ▂  A
▶       └─  ◉ vqok ▂  B
           └─  ◌ lttm  C
```

To see the focus's data, use the inspection functions:

```
j 'files'         -- the focus's files
j 'changed'       -- paths that differ from the parent
j 'status'        -- focus, changes in the commit, unrecorded edits, conflicts
j 'log'           -- a table of every commit, ✓ on the focus
j 'review'        -- the focus's changes as diffs (difftastic)
```

These inspections include current file edits without recording them. Use
`j save` to record file edits on their own (`id` is the same edit). `status` spells out empty states
such as `No changes` and `No conflicts`; `log` shows label names and short
ids and adapts to narrow terminals.

### Composition order, made concrete

```
j 'describe "edited two back" . prev'
```

`prev` runs *first* (right-to-left), then `describe`. From C, this moves to B
and renames **B**. The focus ends on B:

```
   ⌂ zzzz
  └─  ● qmwp ▂
     └─  ● nnvs ▂  A
▶       └─  ◉ vqok ▂  edited two back
           └─  ◌ lttm  C
```

If you meant "rename C," that's just `j 'describe "edited two back"'` — no
`prev`. The lesson: read `.` chains from right to left, and remember each step
operates on the repository the previous step produced.

---

## Part 4 — Dry runs: seeing before doing

Use `preview` to validate an edit and show its resulting tree without saving:

```
j 'preview squash'
```

From the A → B history, this *shows* the tree as `squash` would leave it (B
folded into A, its message gone):

```
   ⌂ zzzz
  └─  ● krly ▂
     └─  ● xspk ▂  A
▶       └─  ◌ zkyl
```

But run `j 'tree'` right after and B is still there — **nothing persisted**,
because `tree` returns a `Text`, not a `Repo`:

```
   ⌂ zzzz
  └─  ● krly ▂
     └─  ● xspk ▂  A
▶       └─  ◌ nknz  B
```

`preview` combines `tree` and `validate`. For an edit that takes arguments,
put that edit in parentheses:

```
j 'preview (rebase %main)'
```

This renders the post-rebase tree **and** crashes (without touching anything)
if that rebase couldn't actually happen — for instance, if `%main` doesn't
exist yet. That's an *honest* dry run.

The longer form is `j 'tree . validate . rebase %main'`. `validate` alone
does not make an edit a dry run: `j 'validate . squash'` returns a repository
and saves it. The final inspection (`tree`) is what prevents persistence.

---

## Part 5 — `or`: the whole error-handling model

There are no exceptions to declare and no `Maybe`. A failing operation
**crashes**, and `or` catches it:

```
head [] or 42         => 42
edit or id            -- run edit; if it crashes, run id (do nothing)
```

`id` is the identity edit, so `edit or id` means "try the edit, keep the
repository if it fails."

The reason this works for edits is **lifting**. A function value can't crash
until it's applied, so `squash or id` would catch nothing if `or` simply
returned the function. Instead, when both sides are functions, `or` builds a
new one pointwise: `(f or g) x = f x or g x`. So:

```
j 'squash or id'
```

actually runs `squash` on your repository, and only if that crashes does it
fall back to `id`. Try it on the A → B history: B gets folded into A, no crash,
so `id` never runs.

A practical use: "rebase onto main if there is a main, otherwise leave things
alone":

```
j 'rebase %main or id'
```

---

## Part 6 — Editing history for real

Now that you can navigate, dry-run, and recover, the editing commands are just
edits with clear semantics:

```
j 'describe "…"'          -- set the focus's message
j squash                  -- fold the focus into its parent
j abandon                 -- drop the focus; children join the parent
j 'split (ext "rs")'      -- push the .rs files up into a new child commit
j 'contract everything'   -- push all of the focus's files into the parent
j 'rebase %main'          -- move the focused subtree onto main
j 'pick @wqzt'            -- copy @wqzt's change here as a new commit
j 'backout @wqzt'         -- a new commit undoing @wqzt's change
```

Two things make these safe to use freely:

1. **The change id survives.** A commit keeps its `@wqzt…` id across
   `describe`, `squash`, `rebase`. You're never tracking a moving sha.
2. **Saved operations are undoable.** Use `j undo` and `j redo` to walk them.

```
j undo            -- revert the last thing
j redo            -- reapply it
j ops             -- see the operation log
```

Undo and redo stop when files have unrecorded edits. Recording those edits
with `j id` adds an operation, so the next undo removes that recording;
another undo reaches the preceding edit. Recording also clears pending
redos. Read `j ops` to choose how far to go back. Successful interactive
undo/redo messages identify the affected operation and restored focus.

### Resolve a conflict

A successful rebase can leave conflicts in the working-copy commit. The edit
is saved; there is no separate continuation command. Resolve it in four steps:

1. Run `j status` to find conflicted paths, and `j review` to inspect changes.
2. Edit those files, choosing the intended content and removing conflict markers.
3. Run `j save` to record the resolution.
4. Run `j status` again and check for `No conflicts` and `All edits recorded`.

`j conflicts` lists conflicted commits throughout the repository. If a
conflict is on another mutable commit, use its id from `j tree` with
`j 'by @id'`, then follow the same steps. Run `j conflicts` again to check
the whole history. `review` uses difftastic when
installed and falls back to a unified diff when it is unavailable.

### `split` and `contract`: moving changes by fileset

A **fileset** is a function `Path -> Bool`. These two are inverses:

- `split m` pushes the files matching `m` **up** into a new child commit.
- `contract m` pushes the files matching `m` **down** into the parent.

Suppose one commit touches `a.rs` and `b.j`, and you want the Rust change on
its own:

```
j 'split (ext "rs")'
```

The `.rs` change becomes a separate child commit (now the focus); the `.j`
change stays in the original. `squash` is defined in terms of these:
`squash = abandon . contract everything`.

Filesets compose: `under ./src`, `ext "rs"`, `both`, `either`, `neg`,
`everything`. So `split (both (under ./src) (ext "rs"))` splits off only the
Rust files under `src/`.

---

## Part 7 — Revsets: naming sets of commits

A **revset** is a function `Repo -> [Id]` — a *set* of commits computed from
the repository. You've been using them: `parents`, `kids`, `%main`.

```
here        -- the focused commit
parents     -- its parents
kids        -- its children
ancestors   -- it and everything above
descendants -- it and everything below
all         -- every visible commit
trunk       -- the main line (%main, %master, or %trunk)
```

Most commands that take a "which commits" argument take a revset. `goto` is
the one place a revset must name *exactly one* commit:

```
j 'new . goto %main'  -- select one main commit and start a mutable child
```

A missing label reports how to fetch and inspect remote labels. An ambiguous
selector lists candidate ids and descriptions so you can choose with `by`.

Revsets combine like sets:

```
j 'matching (\c -> c.message == "wip") all'
```

`matching p rs` keeps the commits of `rs` satisfying predicate `p` — here,
every commit whose message is `wip`. The result is a list of commits, so it
prints (persisting nothing).

---

## Part 8 — Batch operations with `at` and `forEach`

`at rs e` runs edit `e` at the commit revset `rs` names, then returns the
focus to where it was (by id, so `e` may move things):

```
j 'at parents (describe "parent edit")'
```

This renamed the *parent* while leaving you focused where you were — the tree
shows the parent with its new message and the focus marker still on your
original commit.

`forEach rs e` runs `e` at *every* commit of `rs`:

```
j 'forEach descendants (describe "wip")'
```

Every commit below the focus is now described `wip`. `forEach` is not special
— it's an ordinary function `Revset -> Edit -> Edit` defined in `config.j`,
and you could have written it yourself.

---

## Part 9 — Making it yours: writing a command

Everything `j` knows is defined in `config.j`. Any definition whose value is
an `Edit` is a new command. Add this to your `~/.config/j/config.j`:

```
-- squash into the parent and name the combined commit "release"
release : Edit
release = describe "release" . prev . squash
```

Now `j release` works exactly like `squash` or `new`. The signature (`: Edit`)
is optional but recommended — it makes `j` check the definition's arguments
and result at each call.

A revset example — "commits ahead of trunk":

```
ahead : Revset
ahead = minus descendants trunk
```

Then `j 'length . ahead'` prints how many commits are on your branch but not
the main line.

This is the payoff of the whole design: the vocabulary is not a fixed list of
subcommands. It's functions, in a file you can read and extend. `squash`,
`tree`, `status`, `rebase` are definitions, not builtins.

---

## Part 10 — A realistic session, start to finish

Putting it all together. Start a feature on top of main, commit in two parts,
tidy the history, and push.

This session assumes you have a remote with a `main` bookmark (so `trunk`
resolves to something) — for example after `j clone`, or `j remote URL`
followed by `j fetch`. `goto trunk` crashes if there's no main line yet;
`new . goto trunk` is how you start *on top of* it (trunk is immutable, so you
always open a new commit rather than focusing it directly).

```
# start on the main line
j 'new . goto trunk'

# first part
echo 'fn parse() {}' > parser.rs
j 'describe "add the parser"'

# second part
j 'new'
echo 'mod parser;' > lib.rs
j 'describe "wire up the parser"'

# look at it
j 'tree'

# the second commit was really part of the first; fold it in
j squash

# squash leaves an empty child focused; move to the combined commit
j prev

# rename the combined commit
j 'describe "add the parser"'

# preview the rebase, then do it
j 'preview (rebase %main)'
j 'rebase %main'

# share it
j 'push (label "parser" here)'
```

Push sends recorded commits. If you edit files after the last saved edit,
push warns that those edits are omitted. Run `j save`, then push again to
include them. Looking at `j status` does not save the edits.

And if any step goes wrong:

```
j undo
```

---

## Where next

- **[docs/language.md](language.md)** — the language in full: values, syntax,
  semantics, contracts, `show`.
- **[docs/base.md](base.md)** — every builtin and `config.j` definition, with
  examples and an alphabetical index.
- **[docs/comparison.md](comparison.md)** — how `j` maps onto git and jj, if
  you're coming from either.
- **[config.j](../config.j)** — the reference vocabulary, to read and imitate.

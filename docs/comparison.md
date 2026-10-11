# `j` vs. Git vs. Jujutsu

`j` is a functional-programming-centered interface to a Jujutsu (jj)
repository. It is not a new version-control system: the storage, the data
model, and the on-disk format are jj's (which in turn is git-compatible). What
`j` replaces is the *user interface* — instead of subcommands and flags, you
write expressions in a small functional language that transform the
repository.

This document compares the three systems side by side: pure **git**, **jj**,
and **j**. Because `j` builds directly on jj, the interesting contrasts are
git → jj (the model) and jj → j (the interface).

> **How to read the tables.** A cell gives the command or expression you'd
> typically use for the task in that system. `—` means "no direct equivalent."
> Where an analogy is approximate, it's marked *(≈)*.

---

## 1. The one-sentence model

| | |
|---|---|
| **git** | A content-addressed object store of commits, trees, and blobs, with mutable named pointers (branches/tags) and a staging area between your working tree and history. |
| **jj** | The same store, but the working copy is *itself a commit* (`@`), there is no staging area, every change is tracked by a stable *change id*, and every operation is recorded in an undoable operation log. |
| **j** | jj's model, driven by a pure functional language: the repository is a *value*, a command is a *function from repository to repository*, and work is done by composing those functions. |

---

## 2. Core concepts, mapped

This is the Rosetta Stone for the three systems.

| Concept | git | jj | j |
|---|---|---|---|
| Current position | `HEAD` | `@` (the working-copy commit) | the **focus** of the `Repo` value |
| A commit | commit (sha1) | commit (change id + commit id) | a `Commit` record `{ files, message, labels, id }` |
| Identity over rewrites | new sha each time | **change id** is stable | the commit's `id` field is the change id |
| Staging area | the index | — (working copy is a commit) | — (snapshot is automatic) |
| Branch | branch | bookmark | **label** (`[Label]` on a `Commit`) |
| Main line | `main`/`master` branch | `main`/`master` bookmark | the `trunk` revset |
| History | the DAG | the DAG | the `Repo` zipper value |
| Undo | `git reflog` (read-only) | `jj op log` + `jj undo` (first-class) | `j ops`, `j undo`, `j redo` |
| "What am I looking at" | `git status`, `git log` | `jj st`, `jj log` | `j status`, `j tree` |
| Protected history | force-push guards (manual) | — | the `immutable` revset (enforced) |

### The big three differences, in one table

| | git | jj | j |
|---|---|---|---|
| Is there a staging area? | **Yes** (the index) | No | No |
| Is the working copy a commit? | No | **Yes** | **Yes** |
| What survives a rewrite? | Nothing (new sha) | The **change id** | The **change id** (`id` field) |

The staging area is the single biggest git concept that simply does not exist
in jj or `j`: your working directory is always already a commit, so "stage" is
not a step. And the change id is what makes history editing safe to talk
about: a commit keeps its identity (`@wqzt…`) across `describe`, `squash`,
`rebase`, and friends, whereas in git each of those mints a brand-new sha.

---

## 3. Everyday commands, side by side

### 3.1 Starting and joining work

| Task | git | jj | j |
|---|---|---|---|
| Create a repository | `git init` | `jj git init` | `j init` |
| Clone | `git clone URL [DIR]` | `jj git clone URL [DIR]` | `j clone URL [DIR]` |
| Set the remote | `git remote add origin URL` | `jj git remote add origin URL` | `j remote URL` |
| Fetch | `git fetch` | `jj git fetch` | `j fetch` |
| Push | `git push origin main` | `jj git push` | `j push EXPR` |

`j push` takes an *expression* rather than flags: `j 'push (label "feature"
here)'` labels the focused commit, `j 'push relabel'` re-points the remote's
bookmarks after a rewrite.
Push sends recorded commits and warns before sending if files have
unrecorded edits. Run `j id`, then push again to include those edits.

### 3.2 Recording work

| Task | git | jj | j |
|---|---|---|---|
| Save current state | `git add … && git commit` | automatic (`jj st` shows it) | an edit records the snapshot; `j save` records *only* the snapshot |
| Set the message | `git commit -m "…"` | `jj describe -m "…"` or `jj describe` | `j 'describe "…"'` |
| Start the next change | (just keep editing) | `jj new` | `j new` |
| Amend the current commit | `git commit --amend` | `jj describe` + edit files in `@` | edit files + `j 'describe "…"'` |
| See what changed | `git diff` / `git show` | `jj diff` / `jj show` | `j review` (via difftastic) / `j status` |

In git, "commit" is a deliberate act with a staging step. jj snapshots the
working copy automatically. In `j`, expressions see current file edits,
but only a changed `Repo` result records them: `describe` saves a message
and `new` opens the next change. `j status` and other inspections save
nothing. Use `j save` (an alias for `id`) to record the working directory and
nothing else. Status displays the `changed` field as `in commit`, comparing
the current commit with its parent; `unrecorded` shows file edits made since
the last save.

### 3.3 Looking around

| Task | git | jj | j |
|---|---|---|---|
| Current state | `git status` | `jj st` | `j status` |
| History graph | `git log --graph` | `jj log` | `j tree` (compact) / `j treeFull` (with ages, authors) |
| Operation history | `git reflog` | `jj op log` | `j ops` |
| One commit | `git show <sha>` | `jj show <change>` | `j show …` on a `Commit` |
| Find commits | `git log --grep`, `--author` | `jj log -r <revset>` | revsets: `matching (\c -> …) all` |

`j tree` is the closest analog of `jj log`'s graph and of `git log --graph
--oneline`.

---

## 4. Navigating history

git navigates by naming commits (sha, branch, `HEAD~3`). jj navigates with
revsets and explicit movement commands. `j` navigates by refocusing the `Repo`
zipper with edits.

| Task | git | jj | j |
|---|---|---|---|
| To the parent | `git checkout HEAD~1` | `jj prev` *(≈, opens a new empty commit there)* | `j prev` |
| To the child | `git checkout <sha>` (manual) | `jj next` *(≈, opens a new empty commit there)* | `j next` |
| Start work on a named commit | `git checkout <branch>` | `jj new <rev>` | `j 'new . goto %main'` |
| To a specific change | `git checkout <sha>` | `jj new <change-id>` | `j 'by @wqzt'` |
| Back to your branch | `git checkout main` | `jj new main` | `j 'new . goto trunk'` |

A subtle but important difference in how movement works: `git checkout` and
`jj new <rev>` *switch* you to an existing commit (jj opens a fresh empty
working-copy commit on top of it). `jj prev`/`jj next` likewise move by
opening a new empty commit at an ancestor/descendant. `j prev`/`j next` are
different: they **refocus the existing commit** (`goto parents` / `goto
kids`) — they don't create a new commit. To start *new work* on the main line
in `j`, you compose: `new . goto trunk`, never `goto trunk` alone, because
trunk is immutable and the focus must always be a mutable commit.

---

## 5. Editing history

This is where the systems differ most. git's history editing is powerful but
fragile (rebase, interactive rebase, filter-branch); jj makes it routine
because change ids track everything; `j` makes it *compositional*.

| Task | git | jj | j |
|---|---|---|---|
| Reword a commit | `git commit --amend` / rebase -i | `jj describe <rev>` | `j 'describe "…"'` (at the focus) |
| Squash into parent | `git rebase -i` (squash) | `jj squash` | `j squash` |
| Split a commit | `git rebase -i` (edit + reset) | `jj split` | `j 'split (ext "rs")'` (by fileset) |
| Drop a commit | `git rebase -i` (drop) | `jj abandon` | `j abandon` |
| Move a commit/branch | `git rebase main` | `jj rebase -d main` | `j 'rebase %main'` |
| Cherry-pick | `git cherry-pick <sha>` | `jj duplicate` / `jj rebase` | `j 'pick @wqzt'` |
| Revert a commit | `git revert <sha>` | `jj revert <rev>` | `j 'backout @wqzt'` |
| Absorb/fixup | `git commit --fixup` + autosquash | `jj absorb` | `j 'contract (ext "rs")'` *(≈)* |

### Direct analogies for the tricky ones

**Squash.** git's `rebase -i` with `squash` folds a commit into its parent.
jj's `jj squash` does the same for `@`. `j squash` is defined as
`abandon . contract everything`: push all of the focus's files into the
parent, then drop the focus.

**Split.** git splits by pausing a rebase and committing piecemeal — error
prone. jj's `jj split` is interactive. `j 'split m'` takes a *fileset*, so
"put the `.rs` changes in a separate commit" is one expression:
`j 'split (ext "rs")'`. No other system splits by predicate.

**Contract.** `j contract` is the inverse of `split`: it pushes the files
matching a fileset *down* into the parent. There is no single git or jj verb
for "move these specific changes into the parent commit"; `jj absorb` is the
closest *(≈)*.

**Rebase.** `git rebase main` replays your branch onto main. `jj rebase -d
main` moves `@` and its descendants onto main. `j 'rebase %main'` moves the
focused subtree onto `%main`. All three replay each change onto the new base —
jj and `j` keep the change ids, git mints new shas.

**Backout.** `git revert X` adds a commit undoing X. `j 'backout @wqzt'` does
the same by replaying X's *inverse* change.

---

## 6. Branches, labels, and the remote

| Concept | git | jj | j |
|---|---|---|---|
| A named pointer | branch | bookmark | label (`[Label]`) |
| Local vs remote | both live in refs | bookmarks tracked per remote | labels are the *remote's* names, read-only locally |
| Move a pointer | `git branch -f` / `git push` | `jj bookmark set` / push | `j 'push (label "n" here)'` |
| Delete a pointer | `git branch -d` / push --delete | `jj bookmark delete` | `j 'push (unlabel "n")'` |
| Rename | `git branch -m` | `jj bookmark rename` | `j 'push (rename "old" "new")'` |
| After a rewrite | force-push each branch | bookmarks auto-track (locally) | `j 'push relabel'` |

A key difference: in git, branches are *the* way you name commits and they're
local until pushed. In jj and `j`, the *change id* is the durable name, and
bookmarks/labels are just the remote's view. In `j`, a commit's `labels` are
read-only in the language — only `fetch` and `push` change them — which makes
the remote's state explicit and impossible to mutate by accident.

---

## 7. Conflicts

| | git | jj | j |
|---|---|---|---|
| When do conflicts appear? | During merge/rebase, blocking it | Recorded *in* the commit, non-blocking | Recorded in the commit (unresolved `Blob`s) |
| Where do they live? | The working tree + index | In the commit itself | In the commit's `files`, as conflict blobs |
| How do you see them? | `git status` | `jj st`, `jj resolve` | `j conflicts`, `j status` |
| How do you resolve? | Edit files, `git add`, continue | Edit files; conflict is gone from `@` | Edit files, then `j save` and check `j status` |

git stops the world on a conflict: a merge or rebase halts until you resolve
and `git add`. jj and `j` treat a conflict as *data* — a commit can simply
*contain* conflicted files, you can keep working, and you resolve whenever you
like. `j conflicts` lists the commits that have conflicts; checking one out
writes the markers to disk; editing the files and running `j save` records
the resolution. Check `j status` for `No conflicts` and `All edits recorded`.
There is no separate continuation command. `j review` uses a unified diff
when difftastic is not installed.

---

## 8. Undo and recovery

| | git | jj | j |
|---|---|---|---|
| Mechanism | `git reflog` (a log of HEAD moves) | the operation log | the operation log |
| Undo last action | `git reset --hard HEAD@{1}` (manual, risky) | `jj undo` | `j undo` |
| Redo | find the sha in reflog, reset again | `jj redo` | `j redo` |
| Inspect | `git reflog` | `jj op log` | `j ops` |

git's reflog is a forensic tool: you can *find* an old state and `git reset`
to it, but there's no structured undo. jj and `j` record *every* operation as
a first-class, marked operation, so `undo`/`redo` are safe and precise. In
`j`, undoing a `push` restores the local labels (and leaves the remote
untouched).

`j undo` and `j redo` require no unrecorded working-copy edits. Recording
edits with `j id` adds an operation, so the next undo removes that recording
before another undo reaches the preceding edit. A new recording also clears
pending redos. Interactive confirmations identify the affected operation and
restored focus; `j ops` shows the history.

---

## 9. The interface itself

This is the deepest difference, and the reason `j` exists.

| | git | jj | j |
|---|---|---|---|
| Interface | ~150 subcommands, hundreds of flags | ~60 subcommands, revset language | **one expression language** |
| A command is a… | subcommand + flags | subcommand + revset | a **function `Repo -> Repo`** (an `Edit`) |
| Selecting commits | sha, branch, `~`/`^` syntax | revsets (`x-`, `x+`, `::`, `&`) | revsets (functions `Repo -> [Id]`) |
| Composition | pipes in the shell | revset operators | **function composition** (`.`) and `or` |
| Extensibility | aliases, scripts | aliases, templates | **just write a function** in `config.j` |
| Configuration | `git config` key/value | jj config files | `config.j` — the vocabulary itself, in the language |

### An illustrative example: "describe every commit in my stack 'wip'"

- **git**: no single command; you'd script an interactive rebase.
- **jj**: `jj describe -m wip -r '::@ & mutable() & ~trunk()'` (a revset).
- **j**: `j 'forEach descendants (describe "wip")'` — `forEach` is an ordinary
  function `Revset -> Edit -> Edit`; you could have written it yourself.

In git and jj, the set of things you can do is fixed by the subcommands that
ship with the tool. In `j`, the vocabulary is written in the language in
`config.j`, and you extend it by writing more functions. `squash`, `tree`,
`status`, `rebase` are not special — they're definitions you can read and
imitate.

---

## 10. A full workflow, three ways

Goal: start a feature, make two commits, fold the second into the first,
rebase onto main, and push.

**git**
```
git checkout -b feature
# …edit…
git add . && git commit -m "part one"
# …edit…
git add . && git commit -m "part two"
git rebase -i HEAD~2          # mark second as 'squash'
git rebase main
git push --force-with-lease origin feature
```

**jj**
```
jj new main
# …edit…
jj describe -m "part one"
jj new
# …edit…
jj describe -m "part two"
jj squash                     # folds @ into its parent
jj rebase -d main
jj git push --bookmark feature
```

**j**
```
j 'new . goto trunk'
# …edit…
j 'describe "part one"'
j new
# …edit…
j 'describe "part two"'
j squash
j prev                        # squash leaves an empty child; return to the combined commit
j 'describe "complete feature"'
j 'rebase %main'
j 'push (label "feature" here)'
```

The shapes are visibly the same — start, record, record, squash, rebase, push
— because all three are the same underlying DAG work. What changes is the
surface: git's flags and an interactive rebase, jj's subcommands, `j`'s
composed expressions.

---

## 11. Cheat sheet

Quick mapping for someone coming from git (via jj):

| You want to… | git | jj | j |
|---|---|---|---|
| See status | `git status` | `jj st` | `j status` |
| See the graph | `git log --graph --oneline` | `jj log` | `j tree` |
| Commit | `git commit` | (automatic) + `jj describe` | (automatic) + `j 'describe "…"'` |
| New change | — | `jj new` | `j new` |
| Amend message | `git commit --amend` | `jj describe` | `j 'describe "…"'` |
| Go to parent | `git checkout HEAD~` | `jj prev` | `j prev` |
| Go to main | `git checkout main` | `jj new main` | `j 'new . goto trunk'` |
| Squash last commit | `git rebase -i HEAD~2` | `jj squash` | `j squash` |
| Split a commit | `git rebase -i` | `jj split` | `j 'split <fileset>'` |
| Rebase onto main | `git rebase main` | `jj rebase -d main` | `j 'rebase %main'` |
| Cherry-pick | `git cherry-pick X` | `jj duplicate X` | `j 'pick @X'` |
| Revert | `git revert X` | `jj revert X` | `j 'backout @X'` |
| Delete a commit | `git rebase -i` (drop) | `jj abandon` | `j abandon` |
| Tag/bookmark | `git branch X` / `git tag X` | `jj bookmark set X` | `j 'push (label "X" here)'` |
| Push | `git push` | `jj git push` | `j push EXPR` |
| Undo | `git reset --hard HEAD@{1}` | `jj undo` | `j undo` |
| See history of actions | `git reflog` | `jj op log` | `j ops` |
| Resolve conflicts | edit, `git add`, continue | edit files | edit files, `j id` |

---

## 12. When to reach for which

- **Pure git** is universal and lowest-common-denominator. Every host, every
  CI system, every developer knows it. If you must interoperate with people
  who only know git, you can't avoid it — but jj and `j` both sit *on top of*
  git repositories, so interop is preserved.
- **jj** is for when you want git's ubiquity without git's footguns: no
  staging area to manage, safe history editing via change ids, first-class
  undo, and non-blocking conflicts. It's a drop-in replacement on a git repo.
- **j** is for when you also want the *interface* to be composable and
  programmable: when a multi-step operation should be one expression, when
  you'd rather write `split (ext "rs")` than drive an interactive rebase, and
  when the tool's whole vocabulary being written in its own language appeals
  to you.

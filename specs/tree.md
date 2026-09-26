### 7.11 Tree rendering — rails layout

`treeWith options r` renders the history as text. It is implemented in the
binary because it is presentation, not semantics. It reads the `Repo` value,
the `trunk` revset (§Trunk below), the immutable set (§7.5), `diff` sizes,
and jj's commit metadata by id (committer time, author).

The layout is a *rails* graph: the trunk is always the first column (lane 0),
every other commit sits in a lane to its right, rows are in time order, and a
lane stays occupied for as long as the branch it carries is alive. Text
columns are at fixed positions, independent of how deep a commit is.

#### Options

```
{ detail : Int      -- 0, 1, or 2; as before
, margin : Bool     -- age and author column
, elide  : Bool     -- fold uninteresting runs and collapse far subtrees
, icons  : Bool     -- pictographic glyph set
, color  : Text     -- "auto", "always", "never"
, lanes  : Int      -- lane columns, including the trunk lane; ≥ 1
, author : Bool     -- full author name column
, date   : Bool     -- absolute commit date column (YYYY-MM-DD)
, files  : Bool }   -- number of files changed column
```

Missing fields crash. `lanes` is the *fixed* width of the rails area in
lanes; it does not adapt to the history (see §Overflow). The reference
`config.j` sets `lanes = 4`. The `author`, `date`, and `files` columns are off
by default; `treeData` enables all three.

#### Trunk

`j` evaluates the config's `trunk` revset (this is a fifth definition read by
the binary, alongside `user`, `tree`, `immutable`, `labelled`).

* If `trunk r` names exactly one commit `h`, the trunk **T** is `h` and its
  ancestors — a single line from the root to `h`, since history is a tree.
  With the reference config this is exactly the immutable set, so lane 0 is
  "what cannot be rewritten".
* If `trunk r` is empty (no remote), **T** is the root alone.
* If it names several commits, `treeWith` crashes:
  `treeWith: trunk names N revisions`.

The root is always in T. `h` is the *trunk head*.

#### Step 1 — the display tree (elision)

The rendering works on a *display tree* derived from the whole history
(`top r`). With `elide = false` it is the history itself: one node per
commit. With `elide = true` it is the history after the rules of §Elision,
which produce three kinds of node:

| node | rows | time | children |
|---|---|---|---|
| **commit** | one | its committer time | its children in the display tree |
| **run** `╎ n` — `n` consecutive uninteresting commits on one line of descent | one | the time of its first commit | the one child of its last commit |
| **collapsed** — a commit whose descendants are hidden, shown as `⋯ n` | one | its committer time | none |

A run never straddles the boundary of T (the trunk head is interesting, so the
trunk's last commit ends any run; a single-child root is dropped, §The root).
A run of trunk commits is in T; any other run is not.

Nodes with no stored counterpart (minted in this program, as in a dry run)
have time ∞.

#### Step 2 — row order

Rows are a topological order of the display tree, oldest first:

```
ready ← { root }
while ready is non-empty:
    n ← the node of ready with the least time
        (ties: the node that comes first in preorder, using jj's sibling order)
    emit n
    ready ← ready ∪ children(n)
```

Consequently a parent is always above its children, the root is the first
row, the trunk is emitted in order, and everything with time ∞ comes after
everything with a time. Sibling order affects only ties.

The row order is computed in full before any lane is assigned; the lane
rules below refer to it ("placed before", "last child").

#### Step 3 — lanes

There are `lanes` lanes, numbered from 0. Each lane is either empty or holds
a *rail*: a pending edge from a placed node `p` downward, of one of two
kinds.

* **live** — `p`'s remaining children may still fork from it, and one of them
  will inherit the lane.
* **reserved for `d`** — only the node `d` may land in this lane.

Lane 0 belongs to T. Nodes are placed in row order; the lane of a node `n`
with parent `p` is decided as follows, first rule that applies:

1. `n` is the root → lane 0.
2. `n ∈ T` → lane 0 (it inherits `p`'s rail, which is live in lane 0).
3. Some lane holds a rail *reserved for `n`* → that lane.
4. `p ∉ T`, `p`'s rail is live in lane `ℓ`, and `n` is `p`'s **last** child
   in row order → lane `ℓ` (inherit).
5. Otherwise → **fork**: the leftmost empty lane strictly to the right of `p`'s
   lane. If there is none, see §Overflow.

After `n` is placed in lane `ℓ`:

* If `n` has children in the display tree, lane `ℓ` now holds a live rail
  owned by `n`. Otherwise (leaf, collapsed node) lane `ℓ` is empty below this
  row.
* If `n` was the last child of `p` and did not inherit `p`'s lane (rule 5 with
  `p` the trunk head, or `p` the root of an empty trunk), `p`'s lane becomes
  empty below this row: the fork on this row is drawn with `╰` instead of `├`.

**Reservations.** When a node `p ∈ T` is placed and has a trunk child `t`,
every side child of `p` that comes *after* `t` in row order needs its own
lane already, because lane 0 will belong to `t` by then. On `p`'s own row, for
each such child in row order, the leftmost empty lane to the right of 0 is
taken and holds a rail reserved for that child. Side children of `p` that
come *before* `t` fork from lane 0 just in time (rule 5) and need no
reservation.

No other node reserves lanes: a node outside T passes its lane to its last
child (rule 4), so every earlier child can fork from the still-live rail. The
trunk head has no trunk child, so all its children fork, the last with `╰`.

Invariants: the trunk occupies lane 0 and nothing else ever does; a lane is
reused as soon as the branch it carried has ended; a fork's target lane is
always to the right of its source.

#### Overflow

If rule 5 or a reservation finds no empty lane in `0 < ℓ < lanes`, the
subtree rooted at `n` is **flattened**: `n` and every descendant of it still
get their rows, in row order, with their id, message, labels and margin, but
they take no lane. Their rails area shows only the rails that pass through
and `»` in the last column. Nothing is hidden — the focus in particular is
always shown — only the shape beyond `lanes` is lost. Widen `lanes` (or
enable `elide`) to see it.

#### Step 4 — drawing a row

Columns, left to right, at fixed offsets:

1. **Gutter**, 2 characters: `▶ ` on the focus row, blank otherwise.
2. **Rails**, `2 · lanes` characters: lane `i` owns characters `2i` and
   `2i+1`.
3. One space.
4. **Id**: `@` followed by the shortest prefix unique among the commits of the
   display tree, minimum 4 — so the rendered id is itself a valid id literal
   (§3.1). Blank on a run row.
5. **Size bar** (`detail ≥ 1`, focus and its parent and children only; all
   commits at `detail = 2`): `▁ ▂ ▃ ▅ ▇`, as before.
6. **Message**: first line; nothing if empty; followed by `⋯ n` on a
   collapsed node.
7. **Labels**: right-aligned column, present only if any rendered commit has
   a label; names separated by two spaces.
8. **Margin** (`margin = true`): age and author initials, right-aligned, as
   before.
9. **Data columns** (off unless enabled): `date` shows the commit's absolute
   date `YYYY-MM-DD`; `files` shows the number of files changed as `n files`;
   `author` shows the full author name. They appear in that order, grey,
   immediately before the margin in the same right-aligned block.

Columns 4–8 start at the same offset on every row. When stdout is a terminal
and a row would exceed its width, the message is cut and ends with `…` so that
labels and the margin keep their columns; otherwise nothing is cut.

**Rails characters.** For lane `i` on the row of node `n` placed in lane
`ℓ`, character `2i` is, first rule that applies:

| condition | character |
|---|---|
| `i = ℓ` | the node glyph (§Glyphs), or `╎` for a run |
| `i` holds a reservation made on this row (`n ∈ T`) | `╮`, or `┬` if a further reservation lies to its right |
| `i` is the source lane of a fork drawn on this row (`n` forked from `p` in lane `i`) | `├`, or `╰` if `p`'s lane empties below |
| `i` lies between the two ends of a horizontal segment on this row and holds a rail | `┼` |
| `i` lies between the two ends of a horizontal segment on this row | `─` |
| `i` holds a rail passing through this row | `│` |
| flattened rows only: `i = lanes − 1` | `»` |
| otherwise | space |

Character `2i+1` is `─` if it lies inside a horizontal segment (from a fork
source to its target, or from a node to its rightmost reservation), else a
space. On a run row the count follows the `╎`, written into the rails area
and, if needed, into the empty id column.

**Detail line** (`detail = 2`, focus only): one extra row directly under the
focus. Its rails show `│` in every lane that holds a rail below the focus row
(including the focus's own lane if it has children); the text starts two
characters past the id column and lists the changed paths with a mark each:
`+` added, `~` modified, `−` deleted, `✖` unresolved.

#### Glyphs

Unchanged from before:

| glyph | meaning | `icons = true` |
|---|---|---|
| `◉` | focus | `🌸` |
| `●` | ancestor of the focus | `🌿` |
| `○` | other commit | `🍃` |
| `◆` | in the immutable set (§7.5) | `🪨` |
| `◌` | empty: no change against its parent | `🫙` |
| `⊗` | has unresolved files | `🔥` |
| `⌂` | root | `🌱` |

Precedence: `⌂`, then `⊗`, then `◌`, then `◆`, then the position glyphs.
`⊗` and `◌` are properties of the history being drawn — a commit's files, and
its parent's in that history — not of the stored commit with the same id, so
a dry run (§1.2) draws them as the edit would leave them.
Rails use `│ ├ ╰ ─ ┼ ╮ ┬ ╎ »`.

#### Elision (`elide = true`)

A commit is *interesting* if it is the focus, an ancestor of the focus off the
trunk, a child of an ancestor of the focus off the trunk, labelled, conflicted,
a leaf, or has more than one child. Runs of consecutive uninteresting commits
on a single line of descent become a run node. Subtrees whose top is not the
focus, an ancestor of it, or a child of an ancestor become a collapsed node,
`⋯ n` counting the hidden descendants (omitted when `n = 0`).

#### The root

The synthetic root (id `zzz…`, no message or author) is dropped from the
rendering when it is a pure anchor — that is, when it has exactly one child —
so the first real commit starts the tree at the top instead of a `⌂` row
floating above it. This holds whether or not elision is on: with `elide =
false` the root's row is simply omitted, and with `elide = true` it is dropped
rather than folded into a run. A root with more than one child is a genuine
branch point — it may be where the current branch forks off — so it is kept,
drawn with `⌂`.

#### Colour

Glyphs are coloured by meaning (the same precedence as their shape): root bold,
conflict red, empty dim, immutable blue, focus bold cyan, ancestor of focus
green, other light grey. Ids keep their existing colours (conflict red,
immutable blue). Messages are bold on the focus row, red on a conflict, and
dim-italic when empty. Labels are green; the margin and the data columns are
grey with author-hued initials. Rail connectors in lane 0 take the immutable
colour; connectors in other lanes are dim.

The whole focus row is highlighted with a background band that spans the full
terminal width. This is colour only: glyphs, rails, and the `▶` gutter still
carry the meaning, so a colourless rendering loses nothing — the band is a
no-op when colour is off.

#### Legend

After the tree, `treeWith` appends a *legend* explaining the symbols that
actually appear, so a reader can always decode the current rendering. Only
symbols that are used in this tree are explained; a symbol that never appears
is omitted from the legend.

The legend covers the node glyphs present (§Glyphs), the focus gutter `▶`,
run `╎ n` and collapsed `⋯ n` rows when shown, and the detail-line marks
(`+` added, `~` modified, `−` deleted, `✖` unresolved) when a detail line is
present. A collapsed node shows `⋯ n` only when `n > 0`, so the `⋯` legend
entry appears only then. The rails characters are not itemised; they are
structural (§Step 4).

The legend text is faint (dim). Placement: when stdout is a terminal and the
tree's widest line plus the legend fits the terminal width, the legend is set
on the right, one entry per tree row starting from the top, separated from the
tree by a gap; otherwise it is set at the bottom, after a blank line.

#### Worked example

History (times are ages): root `zzzz` → 14 uninteresting commits → `aaaa`
(3w, "add parser") → `kpqx` (2w, "release 1.2", `main`, trunk head).
`aaaa` also has the leaf `mnrv` (1w, "fix lexer"). `kpqx` has the focus
`wqzt` (2d, "wip", `feature`, conflicted) and the empty `ptlm` (1d, "docs",
3 hidden descendants). `wqzt` has the leaves `qrst` (5h, "spike") and `yxsk`
(1h). Options: `detail = 2, margin = true, elide = true, lanes = 3`.

Row order, and the lane state after each row (`·` empty, `L` live, `R` reserved):

| # | node | why now | rule | lane | lanes 0 1 2 after |
|---|---|---|---|---|---|
| 1 | `╎ 14` | the single-child root is dropped; the 14-commit run heads the tree | 2 | 0 | L · · |
| 2 | `aaaa` | only ready node | 2 | 0 | L R(mnrv) · — `mnrv` comes after the trunk child `kpqx`, so it is reserved here |
| 3 | `kpqx` | 2w < 1w? no — ready = {kpqx 2w, mnrv 1w}, oldest is kpqx | 2 | 0 | L R · |
| 4 | `mnrv` | ready = {mnrv 1w, wqzt 2d, ptlm 1d} | 3 | 1 | L · · — leaf, lane 1 empties |
| 5 | `wqzt` | ready = {wqzt 2d, ptlm 1d} | 5 | 1 | L L · — fork from lane 0 |
| 6 | `ptlm` | ready = {ptlm 1d, qrst 5h, yxsk 1h} | 5 | 2 | · L · — last child of the trunk head: `╰`; collapsed, lane 2 empties |
| 7 | `qrst` | ready = {qrst 5h, yxsk 1h} | 5 | 2 | · L · — leaf |
| 8 | `yxsk` | last | 4 | 1 | · · · — inherits, leaf |

Rendered:

```
  ╎ 14
  ◆─╮    @aaaa ▅  add parser                 3w  mo
  ◆ │    @kpqx ▅  release 1.2      main      2w  mo
  │ ○    @mnrv ▂  fix lexer                  1w  mo
▶ ├─⊗    @wqzt ▃  wip              feature   2d  mo
  │ │      ✖ src/lexer.rs   ~ src/parser.rs   + tests/lexer.rs
  ╰─┼─◌  @ptlm    docs  ⋯ 3                  1d  ak
    ├─○  @qrst ▁  spike                      5h  mo
    ○    @yxsk ▃                             1h  mo
```

Column offsets in this example: gutter 0–1, rails 2–7, id from 9. Lane 0 is
`◆ ◆ │ ├ ╰` from `aaaa` to `ptlm` and then empty; the reservation for `mnrv`
is drawn on `aaaa`'s row as `─╮` and consumed on `mnrv`'s row; `ptlm` forks
to lane 2 across the live lane 1, hence `┼`.

The reference `config.j` defines `tree` with `detail = 1, margin = false,
elide = false` (every commit, time-sorted), `treeCompact` with `elide = true`
(the folded view above), and `treeFull` with `detail = 2, margin = true,
elide = false`.

#### Decisions taken in this spec (change any that are wrong)

1. **Trunk source** — the `trunk` revset from `config.j`, read by the binary;
   empty trunk → root only in lane 0.
2. **Row order** — oldest first, root at the top, matching the language's
   `up`/`top` vocabulary. Newest-first (jj style) is the same algorithm with
   the rows reversed and `╭`/`╯` for `╰`/`╮`.
3. **Lane inheritance** — outside the trunk, a node's lane goes to its
   *newest* child, so older children fork just in time and no lane is ever
   reserved except by trunk commits. The alternative (oldest inherits) needs
   a reservation on every fork.
4. **Reservations drawn on the parent's row** (`◆─╮`), not on a connector row
   of their own, so a fork never costs a row.
5. **Fork targets** — leftmost empty lane to the right of the source. Allowing
   targets to the left would narrow some graphs but needs `╭─┤` shapes.
6. **`lanes` is an option**, fixed across renders. Computing it per render
   would keep columns fixed within one output but shift them between
   outputs.
7. **Overflow flattens** rather than hides, marked `»`.
8. **Focus marker** stays in the leftmost gutter, as today.
9. **Message truncation** to the terminal width, so the label and margin
   columns hold.

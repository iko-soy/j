# The `j` base vocabulary

This is the complete reference for everything `j` knows: every builtin and
every definition in the reference `config.j`. Nothing else exists — an
expression can use a name only if `config.j` declares it (a builtin,
implemented by `j`) or defines it (written in the language).

Conventions used in this document:

- **Signatures** are given as `name : Type`. A signature on its own declares a
  builtin; before a definition it documents and checks that definition.
- **Types** like `Edit`, `Revset`, `Fileset`, `Snapshot` are aliases defined
  in §1.3. Lowercase type variables (`a`, `b`, `m`) mean "any type".
- **Examples** show an expression and its result (`=>`). Pure examples are
  literal; repository examples are described, since their output depends on
  the repository.

How to read the three most common aliases:

```
Edit     = Repo -> Repo      -- a command; may crash. Applied to the repo on the command line.
Revset   = Repo -> [Id]      -- a set of commits, computed from a repo
Fileset  = Path -> Bool      -- which paths a command applies to
```

Most definitions are **Edits** — functions `Repo -> Repo`. On the command line
a function result is applied to the current repository, and a `Repo` result is
persisted. `e2 . e1` runs `e1` then `e2`; `e or id` runs `e` and leaves things
alone if it crashes.

---

## 1. Types and shapes

### 1.1 `user` — who commits

```
user : { name : Text, email : Text }
```

The identity `j` commits as. `j` reads this definition itself; it is required.
The reference value is a placeholder you must edit:

```
user = { name = "Your Name", email = "you@example.com" }
```

This is a plain value, not a function. Its signature is checked once, at load
(§4.13): if you write `user = 5`, `j` refuses to start (exit 3).

### 1.2 Paths, labels, entries, snapshots

**`Path = [Text]`** — a file path as its components. Written `./a/b`, which is
`["a" "b"]`. A trailing `/` is ignored; a bare `./` is `[]`, the repository
root. For names the literal cannot spell (spaces, unusual characters), build
one with `splitOn "/"`.

```
./src/lexer.rs        =>  ["src" "lexer.rs"]
./                    =>  []
splitOn "/" "a b/c"   =>  ["a b" "c"]
```

**`Label = Text`** — a bookmark name on the remote. A label is written `%main`
in an expression (a `Revset`, see `labelled`); as data it is just text.

**`Entry = { path : Path, content : Blob }`** — one file in a snapshot: its
path and its content.

**`Snapshot = [Entry]`** — the files of one commit. Paths are unique. A
deleted, resolved path simply has no entry (there is no "deletion" marker).

```
[({ path = ./a.txt, content = blob "hello" })]
    =>  a snapshot of one file, a.txt, containing "hello"
```

**`Change = { from : Snapshot, to : Snapshot }`** — the files before and after
a change. Used by `replay`, `invert`, `touched`, `changeOf`.

**`Meta = { hash : Text, author : Text, email : Text, time : Int }`** — what jj
records about a commit beyond the model: its git hash, author name and email,
and committer time. Produced by the `meta` builtin.

### 1.3 Commits, subtrees, and the repository

**`Commit`** — one commit:

```
Commit = { files   : Snapshot     -- the files
         , message : Text          -- the description
         , labels  : [Label]       -- the remote's names for it (read-only)
         , id      : Id }          -- its change id (unique)
```

`labels` is read-only in the language: only `fetch` and `push` change it.

**`Subtree = { root : Commit, children : [Subtree] }`** — a commit and,
recursively, everything below it.

**`Frame = { parent : Commit, left : [Subtree], right : [Subtree] }`** — one
ancestor on the path back to the top, with a hole where the focused subtree
was. `left` and `right` are the siblings on either side (presentation order
only).

**`Repo = { root : Commit, children : [Subtree], context : [Frame] }`** — the
whole history, seen from one commit (the *focus*). `root` is the focused
commit, `children` its children, `context` the path back up to the top
(`context = []` at the top of history). A `Repo` is a **zipper**.

**`Detached = { subtree : Subtree, rest : Repo }`** — the result of `detach`:
a subtree, and the repository with that subtree removed.

**`Rebase = { from : Snapshot, onto : Snapshot }`** — the base a subtree was
written on, and the new base to rewrite it onto. Used by `replayTree`.

### 1.4 Aliases for functions

```
Edit     = Repo -> Repo      -- a command; may crash
Fileset  = Path -> Bool      -- which paths a command applies to
Revset   = Repo -> [Id]      -- a set of commits, computed from a repo
```

These aliases are unfolded by the contract checker: `at : Revset -> Edit ->
Edit` checks three arguments and a `Repo` result.

### 1.5 Push records

**`Push = { id : Id, name : Label }`** — set the remote's bookmark `name` to
the commit `id`, creating it if absent.

**`Drop = { delete : Label }`** — delete the remote's bookmark of that name.

`j push EXPR` evaluates `EXPR` to a list of `Push` and `Drop` records. Built
with `label`, `unlabel`, `rename`, `relabel` (§8).

```
[({ id = @wqzt, name = "main" })]     -- a Push: point main at @wqzt
[({ delete = "old-feature" })]        -- a Drop: delete the old-feature bookmark
```

---

## 2. Functions and control flow

These are the fundamental combinators. `or` is a keyword, not a builtin.

### `(.)` — composition

```
(.) : (b -> c) -> (a -> b) -> a -> c
```

Right-to-left function composition: `(f . g) x = f (g x)`. Right-associative,
binds very tightly (`infixr 9`). This is how edits are chained.

```
((\x -> x + 1) . (\x -> x * 2)) 5
    => 11                            -- (5 * 2) + 1

describe "wip" . new                 -- start a new commit, then describe it
tree . squash                        -- squash the focus, then render the tree
```

### `id` — identity

```
id : a -> a
```

The identity function. As an edit it does nothing, which makes it the
universal fallback for `or`.

```
id 42                 => 42
(id . id) 7           => 7
edit or id                          -- run edit, keep the repository if it crashes
```

### `const` — constant function

```
const : a -> b -> a
```

`const a b = a` — ignores its second argument.

```
const 5 99            => 5
map (const 0) [1 2 3] => [0 0 0]
everything                              -- defined as  const true
```

### `crash` — abort with a message

```
crash : Text -> a
```

Aborts evaluation with the given message. Every interpreter-detected runtime
error is also a crash, and all crashes are catchable by `or`.

```
crash "something went wrong"
    -- CRASH: something went wrong

(crash "boom") or "recovered"
    => "recovered"
```

### `or` — catching crashes (keyword)

`a or b` evaluates `a`. If `a` crashes, the crash is discarded and `b` is
returned. Otherwise `a` is the result — unless `a` is a function and `b` is
too, in which case the result is pointwise: `(f or g) x = f x or g x`.

```
head [] or 42         => 42
1 or head []          => 1            -- head [] never runs
edit or id                          -- try edit, fall back to id
```

The lifting is what makes `edit or id` meaningful: a function value cannot
crash until applied, so without pointwise lifting the `or` would catch nothing.

---

## 3. Comparison, booleans, integers, `show`

### Equality: `(==)` and `(/=)`

```
(==) : a -> a -> Bool
(/=) : a -> a -> Bool
```

Structural equality on ints, texts, bools, lists, and records; identity on
`Id`; content-and-mode on `Blob`. `/=` is the negation. **Comparing two
functions crashes.**

```
2 == 2                  => true
"a" == "a"              => true
[1 2] == [1 2]          => true
{ a = 1 } == { a = 1 }  => true
1 /= 2                  => true
```

### Booleans: `(&&)`, `(||)`, `not`

```
(&&) : Bool -> Bool -> Bool     -- short-circuit
(||) : Bool -> Bool -> Bool     -- short-circuit
not  : Bool -> Bool
```

`&&` and `||` short-circuit (the right operand is not evaluated if the left
decides the result). Operands must be `Bool`.

```
1 == 1 && 2 == 2        => true
1 == 2 || 3 == 3        => true
not (1 == 2)            => true
not true                => false
```

### Integer arithmetic: `(+)`, `(-)`, `(*)`

```
(+) : Int -> Int -> Int
(-) : Int -> Int -> Int
(*) : Int -> Int -> Int
```

Arbitrary-precision: never overflows. There is no division builtin. No unary
minus — write `0 - 5`.

```
2 + 3 * 4               => 14
20 - 5 - 3              => 12
999999999999 * 999999999999
    => 999999999998000000000001
0 - 5                   => (0 - 5)
```

### Integer comparison: `(<)`, `(<=)`, `(>)`, `(>=)`

Defined on `Int` only; crash on anything else. Non-associative (cannot chain:
`1 < 2 < 3` is a parse error).

```
1 < 2                   => true
3 >= 3                  => true
(1 < 2) == true         => true     -- parenthesised, since < is non-associative
```

### `show` — render a value as text

```
show : a -> Text
```

Renders any value in the language's literal syntax, such that (except for
closures and unresolved blobs) the text parses and evaluates back to an equal
value. A top-level `Text` prints raw, so `j show …` is how you inspect values.
See §5.2 of the spec for the exact rendering.

```
show 42                 => "42"
show "hi"               => "\"hi\""
show [1 "a" true]       => "[1 \"a\" true]"
show ({ x = 1 })        => "{ x = 1 }"
show (describe "wip")   => "describe \"wip\""
show (\x -> x + 1)      => "\\x -> x + 1"
```

---

## 4. Lists and text

### Basic list operations

```
(::)    : a -> [a] -> [a]                  -- cons
map     : (a -> b) -> [a] -> [b]
filter  : (a -> Bool) -> [a] -> [a]
length  : [a] -> Int
null    : [a] -> Bool
head    : [a] -> a                         -- crash on []
tail    : [a] -> [a]                       -- crash on []
last    : [a] -> a                         -- crash on []
nth     : Int -> [a] -> a                  -- 0-based; crash out of range
take    : Int -> [a] -> [a]                -- clamp to length
drop    : Int -> [a] -> [a]                -- clamp to length
member  : a -> [a] -> Bool                 -- membership by ==
range   : Int -> Int -> [Int]              -- range a b = [a (a+1) … (b-1)]
foldl   : (b -> a -> b) -> b -> [a] -> b
```

```
1 :: [2 3]              => [1 2 3]
map (\x -> x * 2) [1 2 3]   => [2 4 6]
filter (\x -> x > 2) [1 2 3 4]  => [3 4]
length [1 2 3]          => 3
null []                 => true
head [5 6 7]            => 5
tail [5 6 7]            => [6 7]
last [5 6 7]            => 7
nth 1 [10 20 30]        => 20
take 99 [1 2]           => [1 2]
drop 99 [1 2]           => []
member 3 [1 2 3]        => true
range 1 6               => [1 2 3 4 5]
range 5 1               => []
foldl (+) 0 [1 2 3 4 5] => 15
foldl (\a -> \b -> a - b) 0 [1 2 3]
    => (0 - 6)                 -- ((0 - 1) - 2) - 3
```

`head`, `tail`, `last`, and out-of-range `nth` crash:

```
head []                 -- CRASH: head: empty list
nth 5 [1 2 3]           -- CRASH: nth: index 5 out of range
```

### `(++)` and `concat` — overloaded on lists and text

```
(++)    : m -> m -> m
concat  : [m] -> m
```

These dispatch on the kind of the argument: they concatenate lists and they
concatenate text. `concat []` crashes (nothing to dispatch on) — supply a
default with `or`, e.g. `concat xs or []`.

```
[1 2] ++ [3 4]          => [1 2 3 4]
"foo" ++ "bar"          => "foobar"
concat [[1 2] [3] []]   => [1 2 3]
concat ["foo" "bar"]    => "foobar"
concat [] or []         => []
```

### Text predicates and splitting

```
startsWith : Text -> Text -> Bool
endsWith   : Text -> Text -> Bool
splitOn    : Text -> Text -> [Text]
```

```
startsWith "he" "hello"     => true
endsWith "rs" "lexer.rs"    => true
splitOn "," "a,b,c"         => ["a" "b" "c"]
splitOn "/" "src/lexer.rs"  => ["src" "lexer.rs"]
splitOn "/" ""              => [""]
```

`splitOn` with an empty separator crashes.

---

## 5. The repository builtins

These are the builtins that touch the repository, blobs, and diffs.

### `replay` — apply a change to a snapshot

```
replay : Snapshot -> Change -> Snapshot
```

`replay onto ch` computes "`onto`, with the change from `ch.from` to `ch.to`
applied." Where the change collides with what is already there, the path's
blob becomes *unresolved* (a conflict) rather than an error. The one
exception is a collision that would make a path a file on one side and a
directory on another, as adding `./a/b` onto a file `./a` would: one entry
cannot list a directory's entries, so `replay` crashes, naming the path, and
`or` catches it. A conflict already at the path in `onto`, `ch.from` or
`ch.to` is carried, directory sides and all, so a child that resolves such a
conflict can still be squashed into it.

```
-- onto has "base", change takes "base" -> "theirs": no collision
replay [({ path = ./f, content = blob "base" })]
       ({ from = [({ path = ./f, content = blob "base" })],
          to   = [({ path = ./f, content = blob "theirs" })] })
    =>  [({ content = blob "theirs", path = ["f"] })]

-- onto has "onto-version" but the change expects "base": collision -> conflict
unresolved ((head (replay [({ path = ./f, content = blob "onto" })]
       ({ from = [({ path = ./f, content = blob "base" })],
          to   = [({ path = ./f, content = blob "theirs" })] }))).content)
    =>  true
```

Two laws to internalise it: `replay b { from = b, to = x } = x` and
`replay x { from = b, to = b } = x`.

### `unresolved`, `blob`, `text` — blob primitives

```
unresolved : Blob -> Bool
blob       : Text -> Blob
text       : Blob -> Text
```

`blob t` builds a resolved regular-file blob with content `t`. `text b` reads a
blob's content (its conflict-marker rendering if unresolved). `unresolved b`
is `true` iff the blob is a conflict.

```
blob "content"              =>  blob "content"
text (blob "content")       => "content"
unresolved (blob "x")       => false
```

### `by` — focus a commit by id

```
by : Id -> Repo -> Repo
```

Refocuses the repository on the commit with the given id; crashes if there is
no such visible commit. Backed by the change-id index. Applied to one argument
(`by @wqzt`) it is an `Edit`.

```
by @wqzt        -- an edit: refocus on the commit @wqzt
```

### `subtreeCommits` — every commit of a tree

```
subtreeCommits : a -> [Commit]
```

Every commit of a tree (anything with `root` and `children`, so a `Subtree`
or a `Repo`'s focused subtree), in preorder, in one pass. It gives exactly
what `\t -> t.root :: (concat (map subtreeCommits t.children) or [])` gives,
but in time linear in the size of the tree: written that way, every level
copies the list below it. The reference config's `commits` is this builtin
(§6).

```
subtreeCommits ({ root = 1, children = [({ root = 2, children = [] })] })
    => [1 2]
```

### `meta` — commit metadata

```
meta : Id -> Meta
```

The git hash, author name and email, and committer time of the visible commit
with change id `i`, as a `Meta` record. Crashes for an id with no stored
commit (one minted in the current program, not yet persisted).

```
meta @wqzt      =>  { hash = "74bee64e…", author = "Test User",
                      email = "test@example.com", time = 1789457265 }
```

### `validate` — dry-run persistence

```
validate : Repo -> Repo
```

Returns the repo unchanged if persisting it would succeed; otherwise crashes
with exactly the error persistence would produce (shape, labels, immutability,
focus). `tree . validate . rebase trunk` is an honest dry run.

### `diff` and `difft` — rendering differences

```
diff  : Blob -> Blob -> Text
difft : Path -> Blob -> Blob -> Text
```

`diff a b` is a unified diff from the text of `a` to the text of `b`: no header
lines, three lines of context, `""` if equal. Crashes if either is not UTF-8.

```
diff (blob "a\nb\n") (blob "a\nc\n")
    => "@@ -1,2 +1,2 @@\n a\n-b\n+c\n"
```

`difft p a b` renders the comparison with **difftastic**, treating the input
as the file `p` (its last component guides language detection). Needs `difft`
on `PATH` (the Nix package provides it); options via `DFT_*` environment
variables. Crashes naming the tool if absent. A `difft` that runs and fails
is not an error: what it printed, even nothing, is the result, and what it
says on stderr reaches the terminal.

### `treeWith` — the history tree

```
treeWith : { detail : Int, margin : Bool, elide : Bool, icons : Bool, color : Text }
           -> Repo -> Text
```

Renders the history as a tree with an options record. The reference config
provides two ready-made settings, `tree` and `treeFull` (§9).

```
treeWith ({ detail = 2, margin = true, elide = false, icons = false, color = "auto" })
    -- a full tree with ages and authors in the margin
```

### `extract` — every subvalue of a shape

```
extract : Shape -> a -> [b]
```

Returns every subvalue of a value that has shape `S`, however deeply nested.
Walks lists in order and records in ascending field-name order, descending
into every element and field (not into functions or blobs). A record is an `S`
if its field set is exactly `S`'s.

```
extract Int ({ a = 1, b = 2 })
    => [1 2]
extract Int ({ a = 1, b = [2 "x" ({ c = 3 })] })
    => [1 2 3]
extract Commit repo       -- every commit in the repository
extract Push ys           -- every Push record in a list, however nested
```

---

## 6. The zipper: moving the focus

A `Repo` is focused on one commit. These primitives move or detach the focus.
Commands never select a commit by position — they use ids, labels, or
predicates.

### `up` — to the parent

```
up : Edit
up = \repo -> …
```

Moves the focus to the parent commit, re-attaching the focused subtree among
its siblings. Crashes at the top of history. This is the raw zipper move;
`prev` (§7.4) is the friendlier version built on it.

```
up          -- refocus on the parent (crash if already at the top)
```

### `detach` — remove the focused subtree

```
detach : Repo -> Detached
```

Detaches the focused subtree; the focus moves to the parent. Crashes at the
top. Returns a `Detached` record (`{ subtree, rest }`). Used to move subtrees
around (with `attach`).

### `top` — to the top of history

```
top : Edit
top = \repo -> top (up repo) or repo
```

Moves the focus all the way to the top of history (where `context = []`). The
`or repo` is the base case: when `up` crashes at the top, stop.

```
top                     -- refocus at the very first commit
commits . top           -- every commit, preorder
```

### `commits` — every commit of a (sub)tree

```
commits : a -> [Commit]
commits = subtreeCommits
```

Every commit of a subtree (or of the focused subtree of a `Repo`), in
preorder. Takes anything with `root` and `children`, so the signature is left
open. It is the builtin `subtreeCommits` (§5), which computes
`\t -> t.root :: (concat (map commits t.children) or [])` in one pass. (There
the `or []` handles a leaf, whose `children` is empty, so `concat` crashes and
yields `[]`.) Written in the language, it copies the list below every commit,
and takes time quadratic in the length of the history.

```
length . commits . top      -- how many commits are visible?
commits . top               -- every commit, preorder from the top
```

---

## 7. Structure and focusing

### 7.1 Editing at the focus

**`mapRoot : (Commit -> Commit) -> Edit`** — apply a function to the focused
commit.

```
mapRoot = \f repo -> repo { root = f repo.root }
```

`describe` is `mapRoot` with a message setter (§8.1).

**`mapChildren : ([Subtree] -> [Subtree]) -> Edit`** — apply a function to the
focus's children.

**`attach : Subtree -> Edit`** — attach a subtree as a new child of the focus
(the focus does not move).

```
attach = \t -> mapChildren (++ [t])
```

**`remove : Edit`** — remove the focused commit; its children join the parent,
and the focus moves to the parent.

**`addChild : Commit -> Edit`** — attach a commit as a new child of the focus
and move to it.

**`newCommit : Snapshot -> Commit`** — a new commit with the given files, no
message, no labels, and a freshly minted id (`@`). Because `@` is evaluated
per application, each call mints a distinct id.

```
newCommit = \files -> { files = files, message = "", labels = [], id = @ }
```

### 7.2 Snapshots, changes, and filesets

**`restrict : Fileset -> Snapshot -> Snapshot`** — the entries matching a
fileset.

```
restrict (ext "rs") snap      -- only the .rs files in snap
```

**`select : Fileset -> Snapshot -> Snapshot -> Snapshot`** — the entries
matching `m` from the first snapshot, the rest from the second.

```
select = \m a b -> restrict m a ++ restrict (neg m) b
```

**`conflicted : Snapshot -> [Path]`** — the paths whose content is unresolved.

**`changeOf : Repo -> Change`** — what the focus changes: its parent's files
(`from`) and its own (`to`).

**`invert : Change -> Change`** — the same change, backwards (`from`/`to`
swapped). Used by `backout`.

```
invert ({ from = [], to = [({ path = ./a, content = blob "1" })] })
    => { from = [({ content = blob "1", path = ["a"] })], to = [] }
```

**Fileset combinators** — filesets are functions `Path -> Bool`, composed
pointwise:

```
everything : Fileset                  -- const true: every path
neg     : Fileset -> Fileset          -- not . m
both    : Fileset -> Fileset -> Fileset   -- m p && n p
either  : Fileset -> Fileset -> Fileset   -- m p || n p
under   : Path -> Fileset             -- inside a directory
ext     : Text -> Fileset             -- a file extension
```

```
under ./src ./src/main.rs     => true
under ./src ./README.md       => false
ext "rs" ./src/main.rs        => true
ext "rs" ./src/main.j         => false
both (ext "rs") (under ./src) ./src/main.rs
    => true
either (ext "j") (ext "rs") ./x.rs
    => true
neg (ext "rs") ./x.j          => true
everything ./anything/at/all  => true
```

### 7.3 Revsets

A revset is a function `Repo -> [Id]`: a set of commits, computed from a repo.
It can be relative to the focus (`parents`, `kids`) or absolute (`all`,
`%main`).

```
here        : Revset    -- [the focused commit's id]
parents     : Revset    -- the focus's parents (crash-safe: [] at the top)
kids        : Revset    -- the focus's children
descendants : Revset    -- the focus and everything below it
ancestors   : Revset    -- the focus and everything above it
siblings    : Revset    -- the other children of the focus's parent
all         : Revset    -- every visible commit
stack       : Revset    -- the line through the focus (ancestors ∪ descendants)
labelled    : Label -> Revset   -- %main is  labelled "main"
trunk       : Revset    -- the main line: %main, %master, or %trunk, first found
immutable   : Revset    -- commits that may never be rewritten (ancestorsOf trunk)
```

```
parents         -- the ids of the focus's parents
kids            -- the ids of the focus's children
labelled "main" -- the commit the remote calls main (== %main)
```

**Revset combinators:**

```
from          : Revset -> Revset -> Revset   -- evaluate f starting from each commit of rs
descendantsOf : Revset -> Revset
ancestorsOf   : Revset -> Revset
union         : Revset -> Revset -> Revset
intersect     : Revset -> Revset -> Revset
minus         : Revset -> Revset -> Revset
matching      : (Commit -> Bool) -> Revset -> Revset   -- filter by a predicate
conflicts     : Revset                        -- commits with unresolved files
firstOf       : [Revset] -> Revset            -- the first non-empty revset
```

```
union ancestors descendants       -- the stack (this is how `stack` is defined)
minus all trunk                   -- everything not on the main line
matching (\c -> c.message == "") all   -- every commit with an empty message
firstOf [%main %master %trunk]    -- this is `trunk`
ancestorsOf trunk                 -- this is `immutable`
conflicts                         -- the commits that have conflicts
```

(These combine revsets, not raw lists — `union [1 2] [3]` is a contract error,
because a revset is a function.)

`immutable` is special: `j` evaluates it before persisting anything and refuses
to change what it names. Merge commits and their ancestors are immutable
regardless. Because the focus must always be mutable, the way to work on trunk
is `new . goto trunk`, never `goto trunk` alone.

### 7.4 Focusing with `goto` and friends

**`goto : Revset -> Edit`** — refocus on *the one commit* a revset names. This
is the only place a revset must have exactly one member; otherwise it crashes
with `expected one revision, got N`.

```
goto = \rs repo ->
  let is = rs repo
  in if length is == 1 then by (head is) repo
     else crash ("expected one revision, got " ++ show (length is))
```

```
goto %main              -- refocus on main (crash if it names 0 or 2+ commits)
goto parents            -- == prev
```

**`commitAt : Id -> Repo -> Commit`** — the commit with a given id.

**`prev : Edit`** — to the parent (`goto parents`).

**`next : Edit`** — to the only child (`goto kids`; crashes if the focus has
other than exactly one child).

**`tip : Edit`** — the end of the line of only-children (`tip = \repo -> tip
(next repo) or repo`).

**`into : Id -> Edit`** — to a child, by id; crashes if it isn't a child of
the focus.

**`child : (Commit -> Bool) -> Edit`** — to the one child satisfying a
predicate (`goto (matching p kids)`).

**`at : Revset -> Edit -> Edit`** — run an edit somewhere else, then come back
(by id, so the edit may move things).

```
at %main (describe "released")
    -- describe main, then return the focus to where it was
```

**`forEach : Revset -> Edit -> Edit`** — run an edit at every commit of a
revset, returning to the focus.

```
forEach descendants (describe "wip")
    -- describe every commit below the focus "wip"
```

**`eachChild : Edit -> Edit`** — `forEach kids`: run an edit at every child.

**`guard : (Repo -> Bool) -> Edit`** — crash unless a predicate holds.

```
guard clean         -- refuse to continue if the focus has conflicts
```

**`clean : Repo -> Bool`** — `true` iff the focus has no unresolved files.

---

## 8. Commands

These are the everyday edits. Each is an `Edit`, applied to the repository on
the command line.

### `describe` — set the message

```
describe : Text -> Edit
describe = \m -> mapRoot (\c -> c { message = m })
```

Sets the message of the focused commit.

```
j 'describe "add the lexer"'
```

### `new` — start a new commit

```
new : Edit
new = \repo -> addChild (newCommit repo.root.files) repo
```

Starts an empty child of the focus (with the focus's files) and moves to it.

```
j 'new'
j 'describe "wip" . new'      -- new commit, then describe it
```

### `abandon` — drop a commit

```
abandon : Edit
```

Removes the focused commit. Its children are rebased onto its parent, and a
new empty commit on that parent becomes the focus. Refused (crashes) for a
commit the remote has a name for — push something else under that name first,
or forget the name.

```
j 'abandon'         -- drop the focused commit
```

### `contract` — push files down into the parent

```
contract : Fileset -> Edit
```

Pushes the files matching `m` from the focus down into its parent.

```
j 'contract (ext "rs")'     -- move the .rs changes into the parent commit
```

### `squash` — fold into the parent

```
squash : Edit
squash = abandon . contract everything
```

Folds the focus entirely into its parent; a new empty commit becomes the
focus. Defined as `contract everything` followed by `abandon`.

```
j 'squash'
```

### `split` — push files up into a new child

```
split : Fileset -> Edit
```

Pushes the files matching `m` from the focus up into a new child, which
becomes the focus and inherits the old focus's children. Labels stay with the
original commit (they are the remote's). The inverse of `contract`.

```
j 'split (\p -> p == ./a.txt)'   -- split off just a.txt into a child commit
```

### `rebase` — move a subtree

```
rebase : Revset -> Edit
```

Moves the focused subtree onto another commit (the one the revset names), and
returns to the moved commit. Crashes if the destination is inside the subtree
being moved.

```
j 'rebase %main'        -- move the current stack onto main
```

### `pick` — copy a change elsewhere

```
pick : Revset -> Edit
```

Applies the focused commit's change onto another commit, as a new child there.

```
j 'pick %main'          -- copy the focused change onto main
```

### `backout` — undo a change

```
backout : Revset -> Edit
```

Creates a new child of the focus that undoes the change of the given commit
(by replaying its inverse).

```
j 'backout @wqzt'       -- a new commit undoing @wqzt's change
```

### Rebasing internals

**`replayTree : Rebase -> Subtree -> Subtree`** — rewrite a subtree as if it
had been written on `onto` instead of `from`, replaying each commit's change
onto the new base and its children from old files to new.

**`replayOnto : Commit -> Edit`** — replay the focused subtree onto a commit,
in place (the subtree stays attached where it is; only its files change).
Crashes at the top.

**`rewrite : (Commit -> Commit) -> Edit`** — replace the focused commit with
`f` applied to it, replaying its children so they follow.

---

## 9. Pushing

`j push EXPR` evaluates `EXPR` to a list of `Push` and `Drop` records. These
functions build such lists.

**`label : Label -> Revset -> Repo -> [Push]`** — label every commit of a
revset.

```
j 'push (label "feature" here)'     -- label the focused commit "feature"
```

**`unlabel : Label -> [Drop]`** — remove a label on the remote.

```
j 'push (unlabel "feature")'        -- delete the "feature" bookmark
```

**`rename : Label -> Label -> Repo -> [b]`** — move a label.

```
j 'push (rename "old" "new")'       -- rename the bookmark "old" to "new"
```

**`relabel : Repo -> [Push]`** — every label the remote already has, at its
current local commit. After rewriting a stack, this moves the remote's
bookmarks along to the rewritten commits.

```
j 'push relabel'        -- after rewriting, re-point the remote's bookmarks
```

---

## 10. Inspection

These return printable values rather than a `Repo`.

**`focus : Repo -> Commit`** — the focused commit.

**`files : Repo -> Snapshot`** — the focus's files.

```
j 'files'               -- the working-copy commit's files
```

**`entryAt : Path -> Snapshot -> [Entry]`** — the entry at a path, as a list
(`[]` if absent).

**`contentAt : Path -> Snapshot -> Blob`** — the content at a path, or an
empty file if absent.

```
contentAt ./a [({ path = ./a, content = blob "x" })]
    => blob "x"
contentAt ./missing []    => blob ""
```

**`touched : Change -> [Path]`** — the paths a change touches.

**`changed : Repo -> [Path]`** — paths whose content differs between the focus
and its parent.

**`diffs : Repo -> [{ path : Path, diff : Text }]`** — what the focus changes,
rendered by difftastic, one record per changed path. (Replace `difft` with
`(\p -> diff)` to use the built-in unified diff instead.)

**`review : Repo -> Text`** — the same, as one page of text, or
`"no changes\n"` when the focus changes nothing. A `Text` result prints raw,
so `j review` is readable in the terminal. Like `diffs`, it crashes when
`difft` cannot be run (not on `PATH`); otherwise whatever `difft` prints,
even nothing, is the page, and what it says on stderr reaches the terminal.

```
j 'review'              -- the focus's changes, as difftastic renders them
```

**`status : Repo -> { id : Id, message : Text, labels : [Label], changed : [Path], conflicts : [Path] }`** — a summary of the focus.

```
j 'status'
-- id       @wqzt…
-- message  add the lexer
-- labels   [feature]
-- changed  [src/lexer.rs]
-- conflicts []
```

**`log : Repo -> [{ id : Id, message : Text, labels : [Label], focus : Bool }]`**
— a summary of every commit, top first, in preorder (`focus` marks the focused
commit).

**`tree : Repo -> Text`** and **`treeFull : Repo -> Text`** — the history as a
tree, built with `treeWith`. `tree` is compact (`detail = 1`, elided);
`treeFull` shows ages and authors in the margin (`detail = 2`, `margin =
true`). Edit these definitions to change what `j tree` shows.

```
tree = treeWith ({ detail = 1, margin = false, elide = true,  icons = false, color = "auto" })
treeFull = treeWith ({ detail = 2, margin = true,  elide = false, icons = false, color = "auto" })
```

```
j 'tree'
--    ⌂ zzzz
--   └─  ● rpmt ▂
--  ▶  └─  ◌ lvpl  add the lexer
```

---

## 11. Alphabetical index

| name | type | section |
|---|---|---|
| `(.)` | `(b -> c) -> (a -> b) -> a -> c` | §2 |
| `(==)` `(/=)` | `a -> a -> Bool` | §3 |
| `(&&)` `(\|\|)` `not` | booleans | §3 |
| `(+)` `(-)` `(*)` | `Int -> Int -> Int` | §3 |
| `(<)` `(<=)` `(>)` `(>=)` | `Int -> Int -> Bool` | §3 |
| `(++)` `concat` | lists & text | §4 |
| `(::)` | `a -> [a] -> [a]` | §4 |
| `abandon` | `Edit` | §8 |
| `addChild` | `Commit -> Edit` | §7.1 |
| `all` | `Revset` | §7.3 |
| `ancestors` | `Revset` | §7.3 |
| `ancestorsOf` | `Revset -> Revset` | §7.3 |
| `at` | `Revset -> Edit -> Edit` | §7.4 |
| `attach` | `Subtree -> Edit` | §7.1 |
| `backout` | `Revset -> Edit` | §8 |
| `blob` | `Text -> Blob` | §5 |
| `both` | `Fileset -> Fileset -> Fileset` | §7.2 |
| `by` | `Id -> Repo -> Repo` | §5 |
| `changeOf` | `Repo -> Change` | §7.2 |
| `changed` | `Repo -> [Path]` | §10 |
| `child` | `(Commit -> Bool) -> Edit` | §7.4 |
| `clean` | `Repo -> Bool` | §7.4 |
| `commitAt` | `Id -> Repo -> Commit` | §7.4 |
| `commits` | `a -> [Commit]` | §6 |
| `concat` | `[m] -> m` | §4 |
| `conflicted` | `Snapshot -> [Path]` | §7.2 |
| `conflicts` | `Revset` | §7.3 |
| `const` | `a -> b -> a` | §2 |
| `contentAt` | `Path -> Snapshot -> Blob` | §10 |
| `contract` | `Fileset -> Edit` | §8 |
| `crash` | `Text -> a` | §2 |
| `describe` | `Text -> Edit` | §8 |
| `descendants` | `Revset` | §7.3 |
| `descendantsOf` | `Revset -> Revset` | §7.3 |
| `detach` | `Repo -> Detached` | §6 |
| `diff` | `Blob -> Blob -> Text` | §5 |
| `diffs` | `Repo -> [{path, diff}]` | §10 |
| `difft` | `Path -> Blob -> Blob -> Text` | §5 |
| `drop` | `Int -> [a] -> [a]` | §4 |
| `eachChild` | `Edit -> Edit` | §7.4 |
| `either` | `Fileset -> Fileset -> Fileset` | §7.2 |
| `endsWith` | `Text -> Text -> Bool` | §4 |
| `entryAt` | `Path -> Snapshot -> [Entry]` | §10 |
| `everything` | `Fileset` | §7.2 |
| `ext` | `Text -> Fileset` | §7.2 |
| `extract` | `Shape -> a -> [b]` | §5 |
| `files` | `Repo -> Snapshot` | §10 |
| `filter` | `(a -> Bool) -> [a] -> [a]` | §4 |
| `firstOf` | `[Revset] -> Revset` | §7.3 |
| `focus` | `Repo -> Commit` | §10 |
| `foldl` | `(b -> a -> b) -> b -> [a] -> b` | §4 |
| `forEach` | `Revset -> Edit -> Edit` | §7.4 |
| `from` | `Revset -> Revset -> Revset` | §7.3 |
| `goto` | `Revset -> Edit` | §7.4 |
| `guard` | `(Repo -> Bool) -> Edit` | §7.4 |
| `head` | `[a] -> a` | §4 |
| `here` | `Revset` | §7.3 |
| `id` | `a -> a` | §2 |
| `immutable` | `Revset` | §7.3 |
| `intersect` | `Revset -> Revset -> Revset` | §7.3 |
| `into` | `Id -> Edit` | §7.4 |
| `invert` | `Change -> Change` | §7.2 |
| `kids` | `Revset` | §7.3 |
| `label` | `Label -> Revset -> Repo -> [Push]` | §9 |
| `labelled` | `Label -> Revset` | §7.3 |
| `last` | `[a] -> a` | §4 |
| `length` | `[a] -> Int` | §4 |
| `log` | `Repo -> […]` | §10 |
| `map` | `(a -> b) -> [a] -> [b]` | §4 |
| `mapChildren` | `([Subtree] -> [Subtree]) -> Edit` | §7.1 |
| `mapRoot` | `(Commit -> Commit) -> Edit` | §7.1 |
| `matching` | `(Commit -> Bool) -> Revset -> Revset` | §7.3 |
| `member` | `a -> [a] -> Bool` | §4 |
| `meta` | `Id -> Meta` | §5 |
| `minus` | `Revset -> Revset -> Revset` | §7.3 |
| `neg` | `Fileset -> Fileset` | §7.2 |
| `new` | `Edit` | §8 |
| `newCommit` | `Snapshot -> Commit` | §7.1 |
| `next` | `Edit` | §7.4 |
| `not` | `Bool -> Bool` | §3 |
| `nth` | `Int -> [a] -> a` | §4 |
| `null` | `[a] -> Bool` | §4 |
| `parents` | `Revset` | §7.3 |
| `pick` | `Revset -> Edit` | §8 |
| `prev` | `Edit` | §7.4 |
| `range` | `Int -> Int -> [Int]` | §4 |
| `rebase` | `Revset -> Edit` | §8 |
| `relabel` | `Repo -> [Push]` | §9 |
| `remove` | `Edit` | §7.1 |
| `rename` | `Label -> Label -> Repo -> [b]` | §9 |
| `replay` | `Snapshot -> Change -> Snapshot` | §5 |
| `replayOnto` | `Commit -> Edit` | §8 |
| `replayTree` | `Rebase -> Subtree -> Subtree` | §8 |
| `restrict` | `Fileset -> Snapshot -> Snapshot` | §7.2 |
| `review` | `Repo -> Text` | §10 |
| `rewrite` | `(Commit -> Commit) -> Edit` | §8 |
| `select` | `Fileset -> Snapshot -> Snapshot -> Snapshot` | §7.2 |
| `show` | `a -> Text` | §3 |
| `siblings` | `Revset` | §7.3 |
| `split` | `Fileset -> Edit` | §8 |
| `splitOn` | `Text -> Text -> [Text]` | §4 |
| `squash` | `Edit` | §8 |
| `stack` | `Revset` | §7.3 |
| `startsWith` | `Text -> Text -> Bool` | §4 |
| `status` | `Repo -> {…}` | §10 |
| `subtreeCommits` | `a -> [Commit]` | §5 |
| `tail` | `[a] -> [a]` | §4 |
| `take` | `Int -> [a] -> [a]` | §4 |
| `text` | `Blob -> Text` | §5 |
| `tip` | `Edit` | §7.4 |
| `top` | `Edit` | §6 |
| `touched` | `Change -> [Path]` | §10 |
| `tree` | `Repo -> Text` | §10 |
| `treeFull` | `Repo -> Text` | §10 |
| `treeWith` | `{…} -> Repo -> Text` | §5 |
| `trunk` | `Revset` | §7.3 |
| `under` | `Path -> Fileset` | §7.2 |
| `union` | `Revset -> Revset -> Revset` | §7.3 |
| `unlabel` | `Label -> [Drop]` | §9 |
| `unresolved` | `Blob -> Bool` | §5 |
| `up` | `Edit` | §6 |
| `user` | `{ name : Text, email : Text }` | §1.1 |
| `validate` | `Repo -> Repo` | §5 |

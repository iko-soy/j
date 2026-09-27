# The `j` language

`j` is a small, pure, functional language. Every `j` command evaluates one
expression in this language against the current repository. The language is
the whole interface: there are no subcommands beyond a handful of reserved
words, and no flags.

This document describes the language itself — its values, syntax, semantics,
and the builtins — with examples throughout. Every example shows an expression
and what it evaluates to, exactly as `j` prints it.

> **A note on the examples.** Where an example evaluates to a `Repo` (the
> repository), the output depends on the repository state, so those examples
> are described rather than shown literally. Everything else is shown exactly.

---

## 1. The big picture

A run of `j` looks like this:

1. Read the expression (from the command line or stdin).
2. Snapshot the working directory into the working-copy commit.
3. Build a value `repo` describing the repository, focused on the
   working-copy commit.
4. Evaluate the expression.
5. If the result is a function, apply it to `repo`.
6. If the final value is a `Repo` that differs from the one loaded, persist
   the difference; otherwise print the value and change nothing.

So the most common thing you write is an **edit** — a function `Repo -> Repo`
(or a value that becomes one). `describe "wip"` is an edit. `new` is an edit.
`squash` is an edit. Composing edits with `.` builds bigger edits.

The repository is applied **implicitly**: if your expression evaluates to a
function, that function is applied to the `Repo`. There is no `repo` variable
in scope — write a function (an edit, a revset, or a lambda) and it receives
the repository. `length . commits . top` is a function `Repo -> Int`, so it
gets applied and prints the count.

```
j 'describe "wip"'              -- set the message of the focused commit
j 'new'                         -- start a new commit on top
j 'describe "wip" . new'        -- start a new commit, then describe it
```

A result that is plain data is printed and persists nothing:

```
j 'length . commits . top'      -- how many commits are visible?
j 'tree'                          -- print the history tree (a Text)
```

---

## 2. Values

There are eight kinds of value.

| kind | example | notes |
|---|---|---|
| `Int` | `42`, `0 - 7` | arbitrary-precision integer |
| `Text` | `"hello"` | Unicode string |
| `Bool` | `true`, `false` | |
| list | `[1 2 3]` | finite, ordered |
| record | `{ a = 1, b = "x" }` | unordered named fields |
| function | `\x -> x + 1` | closure, builtin, or partial application |
| `Id` | `@wqzt` | a jj change id |
| `Blob` | `blob "…"` | the content of one file |
| `Shape` | `Int`, `Commit` | a type name used as a value |

### Integers

Integers are arbitrary precision — they never overflow.

```
999999999999 * 999999999999
    => 999999999998000000000001
```

There is no unary minus. Write `0 - 5` for negative five. A negative integer
renders as `(0 - 5)` so that it round-trips:

```
0 - 5
    => (0 - 5)
```

### Text

Text is a Unicode string in double quotes, with escapes `\"`, `\\`, `\n`,
`\t`, `\r`.

```
"hello world"
    => "hello world"
"line one\nline two"
    => "line one\nline two"
```

### Booleans

`true` and `false`. Conditions in `if`, and the operands of `&&` and `||`,
must be `Bool` — anything else is a crash (see §8).

### Lists

A list is written with square brackets. Elements are separated by
**whitespace**, not commas.

```
[1 2 3]
    => [1 2 3]
["a" "b" "c"]
    => ["a" "b" "c"]
[]                              -- the empty list
    => []
```

Each element is an atom (with optional selectors and updates). An application
or operator expression as an element must be parenthesised:

```
[(1 + 2) (3 * 4)]
    => [3 12]
[("a" ++ "b") "c"]
    => ["ab" "c"]
```

### Records

A record is an unordered set of named fields, written with braces. Fields are
separated by commas.

```
{ name = "j", version = 1 }
    => { name = "j", version = 1 }
```

Records are **structural**: a record is whatever fields it has. There is no
record *type* you must declare. Two records are equal if they have the same
field names with equal values:

```
{ a = 1, b = 2 } == { b = 2, a = 1 }
    => true
```

Select a field with `.name`:

```
{ name = "j", version = 1 }.version
    => 1
```

Update fields with `record { field = value, … }`. Update returns a copy and
**never adds fields** — updating a field that isn't there is a crash:

```
let r = { a = 1, b = 2 } in r { b = 99 }
    => { a = 1, b = 99 }
```

> **The record-literal gotcha.** Postfix selectors and updates bind tighter
> than application. So `f r { a = 1 }` means `f (r { a = 1 })` — the braces
> attach to `r`, not to `f`. A record *literal* passed as an argument after
> another atom must be parenthesised:
>
> ```
> show ({ a = 1 })              -- OK
> extract Int ({ a = 1 })       -- OK
> (\r -> r.name) ({ name = "j" })   -- OK
> ```
>
> The same applies inside lists: `[{ root = c, children = [] }]` — write the
> braces as a parenthesised element `[(…)]` if it follows another element.

### Functions

A function is made with a lambda: `\x -> body`. Multiple parameters are
curried: `\x y -> body` takes two.

```
(\x -> x + 1) 5
    => 6
(\x y -> x - y) 10 3
    => 7
```

A lambda with `_` ignores the parameter:

```
(\_ -> 42) 99
    => 42
```

Functions are first-class: they can be passed, returned, stored in lists, and
partially applied.

```
let inc = (+ 1) in inc 41
    => 42
map (\x -> x * 2) [1 2 3]
    => [2 4 6]
```

### Ids

An `Id` is an opaque jj change id. A literal `@kpqx` (letters `k`–`z`) refers
to an existing commit by unique prefix. The bare token `@` mints a fresh,
distinct id each time it is evaluated (see §7).

### Blobs

A `Blob` is the content of one file, either resolved or unresolved (a
conflict). You rarely write them directly; you get them from a commit's
`files`. `blob "…"` builds a resolved one, `text` reads it back:

```
text (blob "file content")
    => "file content"
unresolved (blob "x")
    => false
```

### Shapes

A type name used in an expression is a `Shape` value — `Int`, `Text`,
`Commit`, and so on. Shapes are mostly used with `extract` (§9.4).

---

## 3. Definitions and scope

### `let`

`let` introduces local bindings. Bindings on one line are separated by `;`;
across lines they line up by column.

```
let x = 10 in x * 2
    => 20

let square = \n -> n * n in square 9
    => 81

let a = 1; b = 2 in a + b
    => 3
```

All the bindings of one `let` block are **mutually recursive**, so you can
write recursive and mutually recursive functions:

```
let even = \n -> if n == 0 then true else odd (n - 1)
    odd  = \n -> if n == 0 then false else even (n - 1)
in even 10
    => true
```

> **Layout in `let` blocks.** After `let`, the column of the first token opens
> the block. A later line whose first token is in *exactly that column* starts
> a new binding; a line indented further continues the current one. So the
> `even`/`odd` example above works because `odd` lines up with `let`'s first
> binding column.

Because evaluation is strict (call by value), a non-function binding is
evaluated immediately, though after the bindings it needs, whatever order
they are written in: `let a = b; b = 5 in a` is 5. Bindings that need each
other's values, as in `let a = b; b = a in a`, crash; recursion works through
*functions*, whose bodies aren't evaluated until applied.

### No shadowing

A `let` binding or lambda parameter may **not** reuse a name that's already
bound in an enclosing scope (including top-level definitions and builtins).
This is an error, not silent shadowing:

```
let x = 1 in let f = \x -> x in f 2     -- parse error: `x` is already bound
```

The one exception is inside `config.j`: a binder there may reuse the name of
one of the config's own top-level definitions, as the reference config's
`newCommit = \files -> …` does, and within that binder's scope the name means
the binder. Builtin names and names bound by an enclosing lambda or `let` are
still refused there.

### Lexical scope

Names resolve lexically: innermost `let` bindings, then lambda parameters,
then top-level definitions and builtins. Closures capture their defining
environment:

```
let x = 100 in let f = \y -> x + y in f 5
    => 105
```

Field names are *not* in any namespace — they exist only after `.`, in record
literals, and in record updates. So a field can be called `id` even though
`id` is the identity function.

---

## 4. Operators

The operator set is fixed — you cannot declare new ones. Precedence and
associativity:

```
infixr 9  .                            -- composition (tightest)
infixl 7  *
infixl 6  +  -
infixr 5  ++  ::
infix  4  ==  /=  <  <=  >  >=         -- non-associative
infixr 3  &&
infixr 2  ||
infixl 0  or                           -- loosest
```

```
2 + 3 * 4
    => 14                              -- * binds tighter than +

20 - 5 - 3
    => 12                              -- - is left-associative

1 :: 2 :: []
    => [1 2]                           -- :: is right-associative

1 == 1 || 1 == 2 && 1 == 3
    => true                            -- && binds tighter than ||
```

Non-associative operators (`==`, `<`, …) cannot be chained; use parentheses:

```
1 < 2 < 3             -- parse error: `<` is non-associative
(1 < 2) == true       -- OK
```

### Sections and operators as functions

Any operator can be used as a function, or partially applied as a *section*:

```
(+)               -- the addition function
(+ 1) 5           -- right section: \x -> x + 1
    => 6
(1 +) 5           -- left section: \x -> 1 + x
    => 6
map (2 *) [1 2 3]
    => [2 4 6]
```

### The selector as a function

A bare `.name` is the accessor function `\x -> x.name`:

```
.name ({ name = "j" })
    => "j"
map (.id) ([{ id = 1 } { id = 2 }])
    => [1 2]
```

(Parenthesise `.id` when it follows a function name, just like a record
literal — otherwise the selector attaches to that name.)

---

## 5. Control flow

### `if`

```
if 1 < 2 then "yes" else "no"
    => "yes"
```

The condition must be a `Bool`. `if 1 then …` is a crash.

### `or`: catching crashes

`a or b` evaluates `a`. If `a` **crashes**, the crash is discarded and `b` is
returned instead. Otherwise `a` is the result (and `b` is not evaluated) —
unless `a` is a function, in which case the two are combined pointwise (see
below).

```
head [] or 42
    => 42                            -- head [] crashes, so 42

1 or head []
    => 1                             -- 1 is fine; head [] never runs

"x" or crash "boom"
    => "x"
```

**The function-lifting rule** is what makes `or` useful for edits. A function
value cannot crash until it is applied, so `edit or id` would catch nothing if
`or` just returned the function. Instead, when `a` is a function, `b` is
evaluated too, and if it's also a function the result is `\x -> a x or b x`:

```
(\x -> x.missing) or (\x -> 0)
    => a function that yields 0 for any input

-- the classic: try an edit, keep the repository if it fails
edit or id
```

This is the whole error-handling model: there are no exceptions to declare,
no `Maybe` — just crash, and catch with `or`.

---

## 6. Working with lists

```
head [5 6 7]        => 5
tail [5 6 7]        => [6 7]
last [5 6 7]        => 7
nth 1 [10 20 30]    => 20           -- 0-based
take 2 [1 2 3 4]    => [1 2]
drop 2 [1 2 3 4]    => [3 4]
length [1 2 3]      => 3
null []             => true
member 3 [1 2 3]    => true
1 :: [2 3]          => [1 2 3]      -- cons
```

`take` and `drop` clamp to the list length:

```
take 99 [1 2]       => [1 2]
drop 99 [1 2]       => []
```

`head`, `tail`, `last`, and out-of-range `nth` crash on empty/out-of-bounds:

```
head []             -- CRASH: head: empty list
nth 5 [1 2 3]       -- CRASH: nth: index 5 out of range
```

### `map`, `filter`, `foldl`

```
map (\x -> x * 2) [1 2 3 4]
    => [2 4 6 8]

filter (\x -> x > 2) [1 2 3 4]
    => [3 4]

foldl (+) 0 [1 2 3 4 5]
    => 15
```

`foldl f z xs` is a left fold: `foldl f z [x y] = f (f z x) y`, and `z` for
the empty list.

```
foldl (\a -> \b -> a - b) 0 [1 2 3]
    => (0 - 6)                       -- ((0 - 1) - 2) - 3
```

### `range`

```
range 1 6           => [1 2 3 4 5]  -- [a (a+1) … (b-1)]
range 5 1           => []           -- empty if b <= a
```

### `concat` and `++`

These two dispatch on the kind of the argument — they work on lists *and* on
text.

```
[1 2] ++ [3 4]      => [1 2 3 4]    -- list concatenation
"foo" ++ "bar"      => "foobar"     -- text concatenation

concat [[1 2] [3] []]   => [1 2 3]  -- flatten [[a]] -> [a]
concat ["foo" "bar"]    => "foobar" -- join [Text] -> Text
```

`concat []` crashes — there is nothing to dispatch on.

---

## 7. Working with text

```
splitOn "," "a,b,c"     => ["a" "b" "c"]
splitOn "/" "src/lexer.rs"  => ["src" "lexer.rs"]
splitOn "/" ""          => [""]
startsWith "he" "hello" => true
endsWith "rs" "lexer.rs"    => true
```

`splitOn` with an empty separator crashes.

---

## 8. Equality, comparison, and crashes

`==` is structural on ints, texts, bools, lists, and records; identity on
`Id`; content-and-mode on `Blob`. `/=` is its negation.

```
2 == 2              => true
"a" == "a"          => true
[1 2] == [1 2]      => true
{ a = 1 } == { a = 1 }  => true
```

Comparing two **functions** crashes — there is no meaningful equality on
closures.

Ordering operators (`<`, `<=`, `>`, `>=`) are defined on `Int` only and crash
otherwise.

### Crashes

`crash msg` aborts evaluation with `msg`. Every runtime error the interpreter
detects is also a crash: unbound name, wrong kind of value, missing field,
out-of-range index, `head []`, comparing functions, a non-`Bool` condition,
arity/kind errors in builtins. All of these are catchable with `or`.

```
crash "my message"
    -- CRASH: my message

(crash "boom") or "recovered"
    => "recovered"
```

Parse errors, configuration errors, and `Id`-resolution failures happen
*before* evaluation and are **not** catchable.

---

## 9. Functions in depth

### Currying and partial application

Every function is curried. Applying a function to fewer arguments than it
takes yields a partial application, which is itself a value:

```
let add = \x y -> x + y in (add 3) 4
    => 7

map (nth 0) [["a" "b"] ["c" "d"]]
    => ["a" "c"]
```

### Composition

`.` composes two functions: `(f . g) x = f (g x)`. It is right-associative
and binds very tightly.

```
((\x -> x + 1) . (\x -> x * 2)) 5
    => 11                            -- (5 * 2) + 1

((\x -> x * 2) . (\x -> x + 1)) 5
    => 12                            -- (5 + 1) * 2
```

Composition is how edits are chained. `describe "wip" . new` first makes a
new commit, then describes it.

```
let compose = \f g -> \x -> f (g x)    -- `.` is just this, built in
```

### `id` and `const`

```
id 42               => 42
(id . id) 7         => 7
const 5 99          => 5             -- const a b = a
```

`id` is the identity edit — `edit or id` falls back to it. `const` ignores its
second argument.

---

## 10. Ids, blobs, and shapes

### Existing ids: `@prefix`

`@wqzt` refers to the commit whose change id begins with `wqzt`, resolved
*before* evaluation. If the prefix matches no commit, or more than one, the
program stops with an error listing the candidates — nothing is evaluated.

```
by @wqzt            -- an edit: refocus the repository on that commit
```

After resolution the literal is an ordinary full `Id`, so `==` on ids is pure
identity.

### New ids: `@`

The bare token `@` mints a fresh, distinct `Id` every time it is evaluated.
In a lambda body (evaluated per application) each call mints a new id; that is
how `newCommit` gives each new commit a unique id.

### Blobs and `unresolved`

```
blob "content"          -- a resolved regular-file blob
text (blob "content")   => "content"
unresolved (blob "x")   => false
```

`unresolved b` is `true` iff the blob is a conflict (produced by a clashing
`replay`). `text` on an unresolved blob yields its conflict-marker rendering.

### Shapes and `extract`

A type name in an expression is a `Shape`. `extract S v` returns every
subvalue of `v` that is an `S`, walking lists in order and records in
ascending field-name order, descending into elements and fields (but not into
functions or blobs).

```
extract Int ({ a = 1, b = 2 })
    => [1 2]

extract Int ({ a = 1, b = [2 "x" ({ c = 3 })] })
    => [1 2 3]

extract Text ({ a = "x", b = [1 "y"] })
    => ["x" "y"]
```

A record is an `S` if its field set is *exactly* `S`'s. This is how
`extract Commit repo` finds every commit in a repository, however deeply
nested.

---

## 11. Contracts

A function that has a **signature** (a builtin, or a definition with one in
`config.j`) is checked on every application: each argument against its
parameter type as it is supplied, and the final result against the result
type.

The check is one level deep — it never inspects inside a list or a record's
field values (that would make every call cost the size of the value).

| declared type | check |
|---|---|
| `Int`, `Text`, `Bool`, `Id`, `Blob`, `Shape` | the value is of that kind |
| `[T]` | the value is a list (elements not inspected) |
| `{ a : T, … }` or a shape name | a record with exactly those field names |
| `A -> B` | the value is a function |
| a type variable | anything |
| an alias (`Edit`, `Revset`, `Snapshot`) | what it stands for; function aliases are unfolded |

A violation is a crash, catchable by `or`:

```
describe 3
    -- CRASH: contract: describe expected Text as argument 1, got Int

nth "x" [1]
    -- CRASH: contract: nth expected Int as argument 1, got Text
```

Lambdas without a signature, and the command-line expression itself, are
unchecked except through the functions they call.

---

## 12. `show`: rendering values as text

`show v` renders any value as an expression in the language, such that
(except for closures and unresolved blobs) the result parses and evaluates
back to an equal value. Since `show` returns a `Text`, and a top-level `Text`
prints raw, `j show …` is how you inspect values.

```
show 42                 => "42"
show "hi"               => "\"hi\""
show [1 "a" true]       => "[1 \"a\" true]"
show ({ x = 1 })        => "{ x = 1 }"
show (0 - 5)            => "(0 - 5)"
show [[1 2] [3]]        => "[[1 2] [3]]"
```

Functions render readably:

```
show (map)              => "map"
show (+)                => "(+)"
show (describe "wip")   => "describe \"wip\""
show (\x -> x + 1)      => "\x -> (x + 1)"
```

Output longer than 80 columns breaks across lines (one element per line,
two-space indented, leading commas for records) — and still round-trips.

---

## 13. The `Repo` value and navigation

The repository appears as a value of shape:

```
Repo     = { root : Commit, children : [Subtree], context : [Frame] }
Commit   = { files : Snapshot, message : Text, labels : [Text], id : Id }
Snapshot = [Entry]
Entry    = { path : Path, content : Blob }
Subtree  = { root : Commit, children : [Subtree] }
```

`Repo` is a **zipper**: `root` is the focused commit, `children` its children,
and `context` the path back up to the top of history. You almost never build
one by hand; you navigate and edit the one you're given.

The reference `config.j` defines the whole vocabulary on top of this. A tour:

```
here                  -- the focused commit's id, as a Revset
top                   -- refocus at the top of history
tip                   -- refocus at the tip
prev                  -- to the parent (crashes at the top)
next                  -- to the only child (crashes if not exactly one)
up                    -- like prev, the raw zipper move
by @wqzt              -- refocus on a specific commit
goto %main            -- refocus on what the remote calls main
parents               -- the focus's parents, as a Revset
kids                  -- its children
ancestors             -- it and its ancestors
descendants           -- it and its descendants
siblings              -- the other children of its parent
all                   -- every visible commit
```

Edits — functions `Repo -> Repo`:

```
new                       -- a new empty child of the focus
describe "wip"            -- set the focus's message
squash                    -- fold the focus into its parent
abandon                   -- drop the focus; children join the parent
split (\p -> p == ./a.txt)  -- split the focus by a fileset
rebase %main              -- move the focused subtree onto main
pick @wqzt                -- apply a commit's change elsewhere as a new child
backout @wqzt             -- a new child undoing a commit's change
contract everything       -- collapse a subtree (see config.j)
```

And the inspections:

```
tree                    -- the history as a tree (a Text)
status                  -- { id, message, labels, changed, conflicts }
review                  -- the focus's changes as one page of diffs
files                   -- the focus's snapshot (its files)
commits . top           -- every commit, preorder
```

### A realistic session

```
-- start some work
j 'new'
-- (edit files in your editor)
j 'describe "add the lexer"'

-- more work on top
j 'new'
-- (edit more files)
j 'describe "add the parser"'

-- look at what you have
j 'tree'

-- the second commit was a mistake; fold it into the first
j 'squash'

-- rename the combined commit
j 'describe "add the front end"'

-- undo the squash entirely
j undo

-- move back to the first commit to fix something
j 'prev'
```

Because edits compose with `.`, multi-step operations are single expressions:

```
-- start a new commit on top of main and describe it, in one step
j 'describe "hotfix" . new . (goto %main)'

-- describe every commit below the focus
j 'forEach descendants (describe "wip")'
```

---

## 14. `diff`, `difft`, and `treeWith`

`diff a b` produces a unified diff (three lines of context, no headers)
between two blobs' text:

```
diff (blob "a\nb\n") (blob "a\nc\n")
    => "@@ -1,2 +1,2 @@\n a\n-b\n+c\n"
```

`difft p a b` renders the comparison with **difftastic** (which must be on
`PATH`; the Nix package provides it). `review` uses `difft` per changed path.

`treeWith options repo` renders the history tree with an options record:

```
treeWith ({ detail = 2, margin = true, elide = false, icons = false, color = "auto" })
```

The reference config gives two ready-made versions: `tree` (compact) and
`treeFull` (with ages and authors in the margin).

---

## 15. Writing your own definitions

Everything `j` knows is defined in `config.j` — builtins are *declared* there,
and the whole vocabulary (`new`, `describe`, `squash`, `tree`, …) is *written*
there in the language. You can add your own. Any definition whose value is an
`Edit` is a new command.

```
-- in config.j:

-- squash the whole stack into one commit called "release"
release : Edit
release = describe "release" . (contract everything)

-- count the commits ahead of main
ahead : Repo -> Int
ahead = \repo -> length (descendants repo) - 1
```

Then:

```
j release
j 'show ahead'
```

A definition with a signature is contract-checked (§11); one without is not
(except through what it calls). A signature with no definition declares a
builtin that `j` implements.

---

## 16. Laws worth knowing

These identities hold for the reference `config.j` and are a good mental model
(`=` means "equal results, or both crash"):

```
prev (next r)                  = r      (when r has exactly one child)
next (prev r)                  = r      (when r is an only child)
top (top r)                    = top r
(abandon . new) r              = r      (up to the new focus's minted id)
rebase parents r               = r
goto here r                    = r
at rs id r                     = r
(e or id) r                    = r      (when e r crashes)
edit or id                                  -- the universal fallback
replay b { from = b, to = x }  = x
invert (invert ch)             = ch
concat [xs]                    = xs
```

They capture the intent: navigation is reversible where it's defined, `id` is
a neutral edit, `or` recovers from failure, and `replay`/`invert` behave like
the algebra they are.

---

## 17. Quick reference

**Special forms:** `if … then … else …`, `let … in …`, `\x -> …`, `or`.

**Operators:** `.` `*` `+` `-` `++` `::` `==` `/=` `<` `<=` `>` `>=` `&&`
`||`, and sections `(op e)` / `(e op)` / `(op)`.

**Core builtins:** `id` `const` `crash` `show` `not` `.`

**Lists:** `map` `filter` `foldl` `head` `tail` `last` `nth` `take` `drop`
`length` `null` `member` `range` `::` `concat` `++`

**Text:** `splitOn` `startsWith` `endsWith` `concat` `++`

**Blobs/repos:** `blob` `text` `unresolved` `replay` `by` `meta` `extract`
`diff` `difft` `treeWith` `validate`

**Reserved commands** (not expressions): `init` `clone URL [DIR]` `remote URL`
`fetch` `push EXPR` `undo` `redo` `ops`.

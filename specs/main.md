# j — specification

`j` is a command-line interface to Jujutsu (jj) repositories. It is an
interpreter for a small, pure, dynamically typed functional language. A
repository is a value in that language; every command is an expression that
maps the current repository value to a new one, or to something to print.

Nothing is implicit beyond the language itself. Every shape, builtin, and
command is declared or defined in one user-owned file, `~/.config/j/config.j`,
written in the language. The binary provides the language (its syntax, its
operators and their fixities, its primitive kinds), the implementations of the
declared builtins, eight reserved words, and the bridge to jj repositories.
`j` does not require the `jj` binary; it uses jj's on-disk formats through
jj-lib.

This document is the complete specification for implementing `j`. Section 9 is
the reference `config.j`; it is part of the specification and ships with the
binary.

---

## 1. Invocation

```
j EXPRESSION…
echo EXPRESSION | j
```

`j` takes the expression from its arguments or from standard input.

1. If standard input is not a terminal and contains at least one byte, the
   expression is the entire content of standard input.
2. Otherwise the expression is the arguments joined with single spaces,
   verbatim. Nothing is quoted or wrapped on the user's behalf: a `Text` is
   a text only if it is quoted in the language, so a message is written
   `j 'describe "fix login"'` (or `j describe '"fix"'`), while `j new` is
   `new` and `j tree . squash` is `tree . squash`. Shell quoting is needed
   only for characters the shell owns: double quotes, parentheses, brackets,
   braces, a backslash, `<` `>` `|` `&` `;` `*` `$`. The operators `++`,
   `::`, `or`, `.`, and the literals `%name`, `@id`, `./a/b` are all safe
   unquoted.
3. If arguments are present and rule 1 also applies, exit 2 with
   `j: expression given both as arguments and on stdin`.
4. If neither is present, print a one-line usage message to stderr and exit 2.
5. `j` accepts no flags. Any argument, including ones beginning with `-`, is
   part of the expression.

### 1.1 Reserved commands

Before the expression is parsed, its text is trimmed of leading and trailing
whitespace and split on whitespace into words. If the first word is one of the
eight reserved words below, the whole text is a reserved command.

| command | operation |
|---|---|
| `init` | create a repository in the current directory; §7.8 |
| `clone URL [DIR]` | clone `URL` into `DIR` (default: last path component of `URL`, minus `.git`); §7.8 |
| `remote URL` | set the URL of the remote `origin`, creating it if absent; §7.8 |
| `fetch` | fetch from `origin`; §7.6 |
| `push EXPR` | set or delete the remote bookmarks that `EXPR` selects; §7.6 |
| `undo` | restore the repository as it was before the newest operation; §7.7 |
| `redo` | reverse the newest undo; §7.7 |
| `ops` | print the operation log; §7.7 |

For every reserved command except `push`, the remaining words are literal
arguments (no quoting, no escaping); a wrong number is a usage error (exit 2)
and the language is not involved. For `push`, the remainder of the text is an
expression, evaluated as in §1.2 steps 2–7 but against the repository as
recorded, without snapshotting the working directory; its value must be a list
of push records (§7.6).

Matching is on the first word only, so an expression whose first token is one
of these words cannot be written on the command line. `(push)` or `x fetch`
are ordinary expressions. `config.j` may define functions with these names;
they are reachable only inside larger expressions.

### 1.2 Evaluating the expression

1. Locate the repository (§7.1). If none, exit 2.
2. Parse the expression (§3). On parse error, exit 3.
3. Load and validate `config.j` (§6), except for its `Id` literals. If it is
   missing or invalid, exit 3.
4. Build the current repository value `r` (§7.2). This includes snapshotting
   the working directory into the focused commit (§7.4).
5. Resolve every `Id` literal in `config.j` and in the expression (§4.10).
   A failure in `config.j` is a configuration error (exit 3); one in the
   expression is a crash (exit 1). Nothing has been evaluated.
6. Evaluate `config.j`'s definitions (§4.1), then the expression, to a value
   `v`.
7. If `v` is a function, apply it to `r`, once, and let `v` be the result.
8. If `v` is a `Repo` (§4.4): if it equals the repository value *as loaded,
   before the snapshot of §7.4*, do nothing at all; otherwise persist it
   (§7.5) and check out its focused commit's files (§7.4). Print nothing.
   Exit 0.
9. Otherwise display `v` (§5.1) on stdout. Persist nothing. Exit 0.

If evaluation crashes (§4.7) at any point, print the crash message to stderr,
persist nothing, leave the working directory untouched, and exit 1.

Step 7 applies exactly once. If the result of applying a function to `r` is
again a function, that function is displayed, not applied again.

Step 8's equality test means `j id` is a no-op when the working directory is
unchanged and records it when it is not: the snapshot makes `r` differ from
the loaded value, and that difference is what gets persisted. The test may be
implemented by comparing commit and tree hashes rather than values.

Two consequences worth knowing:

- **Dry run.** A `Text` result is displayed and persists nothing, and `.`
  composes a renderer after an edit, so `j tree . rebase trunk` shows
  what a rebase would produce without doing it. `validate` in the chain,
  `j tree . validate . rebase trunk`, additionally raises whatever
  persistence would refuse, so the preview is of something that could
  actually be persisted.
- **Do, then look.** `j squash` prints nothing. `j squash && j tree` is the
  idiom; there is no flag for it.

### 1.3 Interrupts

There is no timeout. A non-terminating expression runs until interrupted.
Interrupting `j` (SIGINT) kills it; because nothing is written until
evaluation completes, an interrupted run persists nothing and leaves the
working directory untouched.

### 1.4 Exit status

| status | meaning |
|---|---|
| 0 | success |
| 1 | the expression crashed, or a reserved command was refused |
| 2 | usage error: bad arguments, no repository, unsupported repository (§7.2), another `j` running (§7.7) |
| 3 | configuration or parse error |

If standard output is a pipe whose reader has exited (`j tree | head -1`), the
rest of the output is dropped and `j` exits 0; any other failure to write
output is an error, status 1. A message that cannot be written to stderr, for
whatever reason, is dropped and the run keeps its status:
`j 'crash "boom"' 2>&1 | true` still exits 1.

Errors are written to stderr as a single line beginning with `j: `. Crash
messages are the text given to `crash` (or generated by the interpreter),
prefixed with `j: crash: `, followed on a second line by the innermost base
definition that was executing and the top-level expression. A definition is
executing while the body of its lambda runs, whether it has a signature or
not, and one with a signature also while its value runs when that is a
builtin (`squash`) and while an argument or result of it is checked (§4.13),
so `describe 3` is in `describe`. A lambda's body is its own definition's
under any other name: `prev = goto parents` runs goto's body, so `prev . top`
is in `goto`. A builtin is not a definition, so a crash in one is in
whichever definition applied it, and with none executing the line is only
`   from <expression>`.

---

## 2. Values

The language has these kinds of values. The primitive kinds are part of the
language and are not declared anywhere.

| kind | notes |
|---|---|
| `Int` | arbitrary-precision integer |
| `Text` | Unicode string |
| `Bool` | `true` or `false` |
| list | finite, ordered, homogeneous by convention, `[a b c]` |
| record | unordered set of named fields, `{ a = 1, b = "x" }` |
| function | closure, builtin, or partial application |
| `Id` | opaque: a jj change id. Literal `@kpqx` for an existing one (a unique prefix is enough); bare `@` for a new one (§4.10) |
| `Blob` | opaque: the content of one file, resolved or unresolved |
| `Shape` | a type name used as a value (§4.12) |

Records are structural. A record is not declared; it is whatever fields it
has. The shapes in `config.j` name field sets, and a record *is* a `Commit`,
say, exactly when its field set is `Commit`'s (§4.12). Two records are the
same value if they have the same field names with equal values.

---

## 3. Syntax

### 3.1 Lexical structure

- **Comments.** `--` to end of line. `{-` … `-}`, nesting. A comment is
  whitespace: a block comment ends a line (§3.3) only by containing a line break.
- **Identifiers.** `[a-z_][A-Za-z0-9_']*`. Keywords are excluded:
  `let in if then else or true false`.
- **Type names.** `[A-Z][A-Za-z0-9_]*`. Declared as shapes in `config.j`;
  usable in expressions as values (§4.12).
- **Integer literals.** `[0-9]+`. There is no unary minus; write `0 - 1`.
- **Text literals.** `"…"` or `'…'` with escapes `\"`, `\'`, `\\`, `\n`,
  `\t`, `\r`. The closing quote matches the opening one, so a single-quoted
  literal may contain unescaped `"` and a double-quoted one unescaped `'`.
- **Id literals.** `@` immediately followed by one or more of the letters
  `k`–`z` (jj's change-id alphabet), e.g. `@wqzt`: an existing id, given in
  full or by a unique prefix.
- **New id.** `@` followed by anything other than an identifier character
  is the token `NEWID`. `@` followed by an identifier character outside
  `k`–`z` (`@main`) is a lexical error.
- **Label literals.** `%` immediately followed by a git branch name
  (`[A-Za-z0-9._/-]+`, not starting with `.` or `-`, not containing `..`,
  not ending in `/` or `.lock`), e.g. `%main`.
- **Path literals.** `./` at the start of a token, extending to the next
  whitespace or one of `( ) [ ] { } , ;`: `./src/lexer.rs`. This rule runs
  before the selector and operator rules for `.`, so a `.` inside a path is
  part of the path. The literal denotes the list of components after the
  `./`; a trailing `/` is ignored and a bare `./` is `[]`, the root. Names
  the literal cannot spell are built with `splitOn "/"`.
- **Operators.** Exactly these tokens, and no others (§3.2):
  `.  *  +  -  ++  ::  ==  /=  <  <=  >  >=  &&  ||`, plus the keyword
  `or`, which is an operator syntactically.
- **Selector.** A `.` immediately followed (no whitespace) by an identifier is a
  single selector token `.name`. Any other `.` is the operator `.`.
- **Punctuation.** `( ) [ ] { } , ; : \ = -> _`. The colon `:` is punctuation:
  it separates a name from its type in declarations and record types, and
  appears nowhere else.

### 3.2 Operators and fixities

The operator set and its fixities are part of the language. No others exist
and none can be declared.

```
infixr 9  .
infixl 7  *
infixl 6  +  -
infixr 5  ++  ::
infix  4  ==  /=  <  <=  >  >=
infixr 3  &&
infixr 2  ||
infixl 0  or
```

Non-associative (`infix`) operators cannot be chained. Each symbolic operator
is still a builtin that must be declared in `config.j` to be usable (§6.2);
the table fixes how they parse, not whether they are in scope. `or` is a
keyword and is always available.

### 3.3 Layout

Three layout rules and nothing else.

1. **Top level.** A top-level item begins with a token in column 1. Every
   following line whose first token is in a column greater than 1 continues
   the item. Blank and comment-only lines are ignored.
2. **`let` blocks.** After `let`, the column of the first token opens a block.
   A subsequent line whose first token is in exactly that column begins a new
   binding; a line indented further continues the current binding; the block
   ends at `in` or at the first token in a smaller column. An `in` that begins
   a line obeys the enclosing layout like any other token: in an item it must
   be past column 1, and in an outer block's binding past that block's column.
   Bindings on a single line are separated by `;`. Blank and comment-only
   lines are ignored here too.
3. **Selectors and updates.** A `SELECTOR` or `{` that is the first token of
   a line never selects from or updates what precedes it. On a continuation
   line it begins a new `postfix` (§3.4), which after another `postfix` is
   the next application argument or list element: `r` followed by a line
   `  .a` is `r (.a)`, and followed by `  { a = 2 }` it is `r ({ a = 2 })`.
   So `r.a` and `r { a = 2 }` need the `.a` or `{` on the line `r` ends on.
   `show` relies on this to break a list of records one per line with nothing
   between them (§5.2).

An expression given on the command line or on stdin is a single `expr`, not a
list of items, so rule 1 does not apply to it; rules 2 and 3 do.

### 3.4 Grammar

```
config     := item*
item       := typedecl | signature | definition

typedecl   := TYPENAME '=' type
signature  := (ident | '(' op ')') ':' type
definition := ident '=' expr

type       := atype ('->' type)?
atype      := TYPENAME | ident | '[' type ']' | '{' (ident ':' type (',' ident ':' type)*)? '}' | '(' type ')'

pattern    := ident | '_'

expr       := '\' pattern+ '->' expr
            | 'if' expr 'then' expr 'else' expr
            | 'let' bindings 'in' expr
            | opexpr
bindings   := binding ((NEWLINE|';') binding)*
binding    := ident '=' expr

opexpr     := app (op app)*             -- resolved by the fixity table
app        := postfix+                  -- juxtaposition; left-associative
postfix    := atom ( SELECTOR | '{' fields '}' )*   -- no SELECTOR or '{' after a line break (§3.3)
atom       := ident | TYPENAME | INT | TEXT | IDLIT | NEWID | LABELLIT | PATHLIT | 'true' | 'false'
            | '(' expr ')' | '(' op ')' | '(' op expr ')' | '(' expr op ')'
            | '[' postfix* ']'
            | '{' fields '}'
            | SELECTOR
fields     := (ident '=' expr (',' ident '=' expr)*)?
```

Notes:

- A definition binds a name to a value. Functions are values made with `\`;
  there is no separate function-definition syntax. `\x y -> e` takes two
  parameters.
- A signature immediately preceding a definition of the same name (blank
  lines and comments between are fine) documents and checks that definition
  (§4.13). A signature with no definition declares a builtin. At most one
  signature per name.
- `(op e)` is a right section (`\x -> x op e`), `(e op)` a left section
  (`\x -> e op x`), `(op)` the operator as a function.
- A bare `SELECTOR` atom `.name` is the accessor function `\x -> x.name`.
- `postfix` binds tighter than application, as in Haskell: `f r { a = 1 }`
  is `f (r { a = 1 })`. Consequently a record *literal* passed as an argument
  after another atom must be parenthesised: `attach ({ root = c, children = [] }) r`.
- List elements are separated by whitespace, not commas, and each element is
  a `postfix` (an atom with selectors and updates): `[1 2 3]`,
  `["src" "lexer.rs"]`, `[{ root = c, children = [] }]`. An application or
  operator expression as an element is parenthesised: `[(up repo).root.id]`.
  Records keep commas.
- Lambda, `if`, `let`, and `or` extend as far right as possible.
- Patterns are variables and `_` only. No literal, list, or record patterns.
- Nesting is bounded: an expression more than 40,000 levels deep is a parse
  error (§1.4), never a stack overflow. A name, literal, `(op)` or `.name`
  accessor is one level; a parenthesis or section, and the braces of a record
  or update, are two levels above what they enclose; every other construct
  is one level above its highest part. A chain `a + b + c …`, `f x y …` or
  `r.a.b …` is thus one level deeper per link, as are right-associative
  operators, `or`, and `\`/`if`/`let` bodies. Lists nest 40,000 deep,
  parentheses and records about 20,000, and a `Subtree` as `show` renders it
  (§5.2) about 13,000: three levels a commit, the braces of its record and
  the list around them, as a list broken over lines holds records without
  parentheses (§3.3). A type in `config.j` is bounded the same way: one
  level, plus one for each bracket, parenthesis, field or `->` around it.

---

## 4. Semantics

### 4.1 Evaluation

Strict, call by value, left to right. Function application is curried. All
top-level definitions in `config.j` and all bindings of one `let` block are
mutually recursive. A top-level definition is evaluated once, at load, in
dependency order: a definition is evaluated after every definition its body
refers to outside a lambda. A cycle among such references (two non-lambda
definitions each needing the other's value) is a configuration error;
references inside lambda bodies never form cycles, since a lambda is a value
before it is applied. The bindings of a `let` block are evaluated by the same
rule, with a cycle being a crash. A top-level definition is moreover evaluated
after every definition that it reaches through a chain of references of
either kind but that does not reach it back. Among definitions that do reach
each other, one that refers to none of the others outside a lambda, and whose
every lambda outside other lambdas that refers to one of them is stored, is
evaluated before the rest, since it cannot apply their functions; an
expression is stored when it is the whole definition, or a record field, list
element, `if` branch or `let` body of a stored expression. So a function
applied at load finds the definitions its body refers to, save one that
reaches back to the definition applying the function and is not evaluated
before the rest; the order among the rest is otherwise unspecified. A `let`
binding is ordered the same way among the bindings of its block. A lambda
body is evaluated on each application.
Recursion is unbounded; the interpreter must handle deep recursion without
stack overflow (or turn exhaustion into a crash).

### 4.2 Scoping

Names resolve lexically: `let` bindings, then lambda parameters, then
top-level definitions and declared builtins. Field names are not in any
namespace; they occur only after `.`, in record literals, and in record
updates, so a field may be called `id` while `id` remains the identity
function.

There is no shadowing. A `let` binding or lambda parameter may not use a name
already bound in an enclosing scope, including top-level definitions and
builtins (reserved whether or not declared, §6.2); in `config.j` that is a
configuration error (exit 3), on the command line a parse error. A lambda's
earlier parameters enclose its later ones, and all bindings of a `let` block
enclose each of its expressions, since they are mutually recursive (§4.1), so
`\x x -> x` and `let a = \b -> b; b = 2 in a 1` are errors too. The one
exception is within `config.j`, whose binders may reuse the names of its own
top-level definitions, as the reference config's `\files ->` and
`\old new repo ->` do: within the binder's scope the name means the binder,
for evaluation order (§4.1) as for lookup, so a `let` binding used before it
is evaluated is a crash there as anywhere, never the definition. Defining a
top-level name twice, or defining a name that is also a builtin, is a
configuration error.

### 4.3 Records

- `e.f` selects field `f`; crashes if absent.
- `e { f = v, … }` returns a copy with the listed fields replaced; crashes if
  any listed field is absent. Update never adds fields.
- Duplicate field names in a literal or update are a parse error.

### 4.4 Which values are a `Repo`

A value is treated as a `Repo` in §1.2 step 8 if and only if its shape is
`Repo` and its `root`'s shape is `Commit`, in the sense of §4.12 (exact field
sets). Deeper shape errors surface as crashes during persistence.

### 4.5 Equality

`==` is structural on ints, texts, bools, lists, and records; identity on
`Id`; content-and-mode on `Blob`; on shapes, equal field sets or the same
primitive kind. Comparing functions crashes. `/=` is its
negation. Ordering operators are defined on `Int` only and crash otherwise.

### 4.6 Special forms

`if`, `or`, `&&`, and `||` do not evaluate all operands. `&&` and `||`
short-circuit. The interpreter recognises `&&` and `||` by name; `or` is a
keyword.

`a or b` evaluates `a`. If that crashes, the crash is discarded and `b` is
evaluated and returned. Otherwise, if the value of `a` is a function, `b` is
evaluated too, and if it is also a function the result is the function
`\x -> a x or b x`, with the same rule applying again to its result. In every
other case the result is the value of `a` and `b` is not evaluated. The
lifting is what makes `edit or id` mean "try the edit, keep the repository if
it crashes": a function value cannot crash until it is applied, so without
lifting the `or` would catch nothing.

### 4.7 Crashes

`crash msg` aborts evaluation with `msg`. Every runtime error the interpreter
detects (unbound name, wrong kind of value, missing field, out-of-range index,
`head []`, `concat []`, comparing functions, non-`Bool` condition, arity or
kind errors in builtins) is a crash with an interpreter-generated message, and
is catchable by `or` exactly like a user crash. Nothing else is catchable: parse
and configuration errors, and `Id` literal resolution, happen before
evaluation.

### 4.8 Overloading

Two builtins dispatch on the kind of an argument; functions compose with `.`:

| builtin | list | Text | function |
|---|---|---|---|
| `++` | concatenation | concatenation | crash |
| `concat` | flatten `[[a]] -> [a]` | join `[Text] -> Text` | crash |

`concat []` crashes (there is nothing to dispatch on). `map`, `filter`,
`length`, `null`, `head`, `tail`, `nth`, `take`, `drop`, `last`, `member`,
`foldl`, and `::` are defined on lists only.

### 4.9 Builtins

The following table is normative for behaviour. Each builtin is available only
if `config.j` declares it (§6.2); the declared type is enforced at each call
as in §4.13.

| name | behaviour |
|---|---|
| `.` | composition: `(f . g) x = f (g x)` |
| `id` | identity |
| `const` | `const a b = a` |
| `crash` | §4.7 |
| `==`, `/=` | §4.5 |
| `&&`, `||`, `not` | booleans; `&&`/`||` short-circuit |
| `+`, `-`, `*` | integer arithmetic |
| `<`, `<=`, `>`, `>=` | integer comparison |
| `show` | renders any value as text in literal syntax (§5.2) |
| `::` | cons |
| `map`, `filter` | as usual |
| `length`, `null` | as usual |
| `head`, `tail`, `last` | crash on empty |
| `nth i xs` | 0-based; crash out of range |
| `take n`, `drop n` | clamp to list length |
| `member x xs` | membership by `==` |
| `range a b` | `[a (a+1) … (b-1)]`, empty if `b <= a` |
| `foldl f z xs` | left fold: `foldl f z [x y] = f (f z x) y`; `z` for `[]` |
| `concat`, `++` | §4.8 |
| `startsWith p t`, `endsWith s t` | text predicates |
| `splitOn sep t` | the pieces of `t` between occurrences of `sep`; `splitOn "/" "a/b"` is `["a" "b"]`, `splitOn "/" ""` is `[""]` |
| `replay onto ch` | replay the change `ch` (`{ from, to }`) onto the snapshot `onto`; §7.3 |
| `unresolved b` | `true` iff the blob is a conflict |
| `blob t` | a resolved regular-file blob with content `t` |
| `text b` | the content of a blob as text; for an unresolved blob, its conflict-marker rendering |
| `by i repo` | the repo refocused on the commit with id `i`; crashes if absent. Backed by the change-id index; its reference definition is in §10 |
| `subtreeCommits t` | every commit of the tree `t` (anything with `root` and `children`) in preorder, in time linear in its size: the value of `t.root :: (concat (map subtreeCommits t.children) or [])`. So it crashes only if `t` has no `root`, and a node whose `children` is not a list of values that have a `root` contributes only its own. The reference `commits` is this builtin (§9) |
| `diff a b` | a unified diff from the text of `a` to the text of `b`, split into lines after each `\n` (not at a lone `\r`), no `---`/`+++` header lines, three lines of context, each hunk headed `@@ -s,n +s,n @@` with the first line and the number of lines it shows of each side, as `diff -u` writes it, a line that has no trailing newline followed by `\ No newline at end of file`; `""` if equal; crashes if either is not UTF-8 |
| `difft p a b` | the output of difftastic comparing `a` to `b` as the file `p`; §7.10. Crashes if `difft` is not on `PATH` |
| `treeWith o r` | the history rendered as a tree with options record `o`; §7.11 |
| `extract S v` | every subvalue of `v` that is an `S`; §4.12 |
| `meta i` | `{ hash, author, email, time }` for the visible commit with change id `i`: its current git hash, author name and email, committer time. Crashes for an id with no stored commit (one minted in this program) |
| `validate repo` | `repo` unchanged if persisting it would succeed; otherwise the crash persistence would raise (§7.5 steps 1–3 and 6). Persistence runs the same checks |

### 4.10 Ids: existing and new

**Existing.** An `Id` literal `@kpqx` denotes the existing commit whose id
begins with `kpqx`. It is resolved before evaluation, not during it:

- In the command-line expression, every literal is resolved after parsing and
  before evaluation (§1.2 step 5), against the ids visible when the program
  started. A literal that matches no id or more than one is a crash that lists
  the candidates in commit-table form (§5.1); nothing is evaluated.
- In `config.j`, every literal is resolved at load, against the same ids. A
  literal that fails to resolve is a configuration error (exit 3) naming the
  definition it appears in.

After resolution a literal is an ordinary `Id` value holding the full id, so
`==` on ids is identity and never involves prefixes.

**New.** The bare token `@` is an expression that yields a distinct, freshly
minted `Id` each time it is evaluated. In a script that writes `@` once, this
is indistinguishable from replacing it with a new id before the run. In a
lambda body it is evaluated on each application, so `newCommit` (§9) mints a new
id per call. A top-level definition that is a bare record containing `@` is
evaluated once at load and holds one id for the whole program (§4.1). `show`
renders a minted id like any other.

### 4.11 Label literals

`%main` denotes `labelled "main"`, a `Revset`, as if written out where it
stands: in the scope of a `config.j` binder named `labelled` (§4.2) it applies
the binder, and it is a reference to it for evaluation order (§4.1). It is a
function, evaluated when applied, so it never fails at parse time: a name the
remote does not have yields `[]`, and `goto %nope` crashes with "expected one
revision, got 0" as any revset would.

### 4.12 Shapes as values, and `extract`

A type name used in an expression evaluates to a *shape*: for a `typedecl`
whose type is a record, the set of its field names; for a primitive kind
(`Int`, `Text`, `Bool`, `Id`, `Blob`), that kind; for a `typedecl` that is an
alias of another type, that type's shape. A shape whose type is a function
(`Edit`, `Revset`), a list, or undeclared is a crash when used, as is an
alias that never reaches a type (`A = B` with `B = A`).

`extract S v` returns every subvalue of `v` that is an `S`, as a list. It
walks lists in order and records in ascending field-name order, descending
into every element and field; it does not enter functions or blobs. A record
is an `S` if its field set is exactly `S`'s (the same rule display uses to
recognise shapes); a primitive value is an `S` if it is of that kind. Matches
are collected in traversal order, and a matching record is also searched
inside for nested matches.

```haskell
extract Commit repo       -- every commit reachable from a Repo, Subtree, or Frame
extract Id repo           -- every id in it
extract Push xs           -- the Push records in a list, however nested
```

`extract` does not promise a meaningful order for values with several
positional fields; a `Repo` yields its `children` before its `context` before
its `root`. Where order matters, `commits (top repo)` is preorder.

### 4.13 Contracts

Every application of a function that has a signature — a builtin or a
definition with a signature — is checked against it: each argument as it is
supplied, against the corresponding parameter type, and the result once the
function's own application completes (its body has run or the builtin has
all its arguments), against the result type.
Lambdas without a signature and the command-line expression are unchecked
except through the functions they call. A signature covers all the arguments
it lists, however the definition's value is curried: `describe : Text -> Edit`
is `\m -> mapRoot …`, and the `Repo` that `describe m` is then applied to is
checked as its second argument, and the result against `Repo`; with
`addT : Int -> Int -> Text` and `addT = (+) . (\x -> x + 1)`, the `5` of
`addT 1 5` is checked as its second argument, and the `7` against `Text`. A
signature only adds checks and never changes when a body is evaluated
(§4.1): `describe m` evaluates `mapRoot …` at once, so an `or` around it
catches a crash there (§4.6); the value must be a function, since the
signature lists a further argument, and shows as `describe m` (§5.2);
`addT 1` likewise shows as `addT 1`, not as the `(+) 2` it holds. Arguments
are numbered as the signature lists them, even when the definition's value is
a partial application or a composition holding arguments of its own (`tree 5`
is a violation at argument 1); when the value is an `or` of functions (§4.6),
the check is around the lifted function, so a result that violates the
signature is a crash, not a reason to try the other side. A signature on
a definition that is not a function (`user`, `conflicts`) is checked once,
against the value, when the definition is evaluated at load.

The check is one level deep and never inspects inside a list or a record's
field values:

| declared type | check |
|---|---|
| `Int`, `Text`, `Bool`, `Id`, `Blob`, `Shape` | the value is of that kind |
| `[T]` | the value is a list; elements are not inspected |
| `{ a : T, … }`, or a shape name | the value is a record with exactly those field names |
| `A -> B` | the value is a function |
| a type variable | anything |
| an alias (`Edit`, `Revset`, `Snapshot`) | the type it stands for; an alias of a function type is unfolded so that further arguments and the final result are checked against it (`at : Revset -> Edit -> Edit` checks three arguments and a `Repo` result); an alias the unfolding has already unfolded is not unfolded again, so `Stream = Int -> Stream` checks one argument and a function result |

Field values and list elements are not checked because that would make every
call cost the size of the value and would force lazily backed snapshots and
histories (§7.2).

A violation is a crash, catchable by `or` like any other:

```
j: crash: contract: describe expected Text as argument 1, got Int
   in describe, from describe 3
```

For records the expected and actual field sets are shown:
`expected { files, labels, id, message }, got { children, context, root }`.

---

## 5. Output

Two renderers. **Display** is what the CLI prints; it is designed to be read.
**`show`** is the builtin; it renders literal syntax designed to be pasted
back. They share glyphs and colours with `treeWith` (§7.11) and are
implemented beside it.

### 5.1 Display

Every kind of value has a *line* form and a *block* form. The top-level value
prints in block form. Values nested inside another print in line form,
indented under their key or bullet. Colour follows §7.11's rules and is used
only when stdout is a terminal; nothing is conveyed by colour alone.

Shapes are recognised by their exact field set, as in §4.12: `Commit`, `Entry`,
`Subtree`, `Repo`, `Frame`, `Change`. Any other record is generic.

| kind | line form | block form |
|---|---|---|
| `Int` | decimal | same |
| `Bool` | `true` / `false` | same |
| `Text` | raw, first line, `…` if truncated | raw, all of it, trailing newline added if absent |
| `Id` | shortest unique prefix (bold), remainder (dim) | same |
| `Blob` | `‹214 B›`, `‹✖ 3.1 KB›` if unresolved | its content, raw (markers if unresolved) |
| function | name, lambda source, or `f arg …`; first line, `…` if truncated | same, all of it |
| `Shape` | its type name | same |
| `Commit` | `glyph id  message  labels` — a `tree` line without rails | that line, then `author · age · n files`, then one changed path per line with `+ ~ − ✖` marks |
| `Entry` | `path  size`, `✖` prefix if unresolved | same |
| `Subtree` | its top commit's line form | a tree rooted at it, no focus |
| `Repo` | its focus's line form | `treeWith` with the config's `tree` options |
| `Frame` | its parent's line form | parent's line, then its children's line forms in a row with `▢` at the hole |
| `Change` | `n paths` | one touched path per line with `+ ~ − ✖` marks, as in a `Commit` block |
| generic record | `{ a, b, … }` (field names only) | one field per line, names left-aligned and dim, values indented if multi-line |
| list of `Id` | `n commits` | a commit table: one commit line per id, in list order |
| list of generic records with identical field sets | `n rows` | a column table with a dim header; `true` shows as `✓`, `false` as blank; columns empty in every row are dropped |
| list of `[Text]` | `n paths` | one per line, joined with `/` |
| list of `Text` | `n items` | one per line |
| list of other scalars | `a, b, c` | same, wrapped at the terminal width |
| any other list | `n items` | one block per item, separated by a blank line |

An empty list of any kind prints `none` (dim) in block form and `0 …` in
line form. Author and age in a `Commit` block are read from jj's commit
metadata by id;
they are not fields of the value. Nor is where it stands, which is read by id
from `r`, the repository the expression was given (§1.2 step 4, after the
snapshot): a block's paths are those it changes against its parent's files
there, and its glyph, in either form, is the one `tree` draws for it there,
with `⊗` and `◌` judged by the value's own files. What a commit with no
parent there (the root, or one the expression made) changes is not known:
its block lists its files unmarked (but for `✖` on an unresolved one), and
its glyph claims no `◌`. One the expression made has no metadata either
(§7.11), so its block has no `author · age · n files` line. One the
expression moved is still compared with its parent there. A `Repo`'s
line draws its focus as the focus. A list of `[Text]` prints as paths because
that is what such a list is in every base function; `show` is available
when the literal is wanted. Alignment in tables and columns uses display
width (East Asian wide characters count two cells, combining marks zero), not
code-point count.

Examples:

```
$ j status
changed    src/lexer.rs
           src/parser.rs
conflicts  src/lexer.rs
id         wqzt·kpqxmnrvyxskptlmzzzzabcd
labels     feature
message    wip

$ j conflicts
⊗ wqzt  wip     feature
⊗ ptlm  docs

$ j log
   id    message     labels     focus
   zzzz
   kpqx  add parser  main
   mnrv  fix lexer
◉  wqzt  wip         feature    ✓
   yxsk
   ptlm  docs

$ j focus
◉ wqzt  wip                              feature
  Montelot · 2h ago · 2 files

  ✖ src/lexer.rs
  ~ src/parser.rs

$ j files
  Cargo.toml       214 B
✖ src/lexer.rs     3.1 KB
  src/parser.rs    8.7 KB

$ j clean
false
```

(`·` marks the bold/dim boundary in an id; there is no character there.)

Crashes print the message, then the innermost base definition that was
executing and the top-level expression:

```
j: crash: expected one revision, got 0
   in goto, from goto %nope
```

### 5.2 `show`

`show v` renders `v` as an expression in the language, such that for every
value except closures, unresolved blobs, and values nested past the parser's
bound (§3.4: a list 40,000 deep, a record about 20,000), `show v` parses and
evaluates to a value equal to `v` under the same `config.j`. It never uses
colour and never truncates. A value holding a part the store cannot supply (a
`files` list or a file's content, §7.2) crashes `show` as it crashes any other
use: `show` writes no stand-in for the part.

| kind | rendering |
|---|---|
| `Int` | decimal |
| `Text` | double-quoted, escaped as in §3.1 |
| `Bool` | `true` / `false` |
| list | `[v1 v2 …]` |
| record | `{ a = v, b = w }`, fields in ascending lexicographic order |
| `Id` | `@` followed by the full id, e.g. `@wqztkpqxmnrvyxskptlmzzzzabcd` |
| resolved `Blob` | `blob "…"` |
| unresolved `Blob` | `{- unresolved -} blob "…"` with the conflict-marker rendering |
| builtin | its declared name, operators parenthesised: `map`, `(++)` |
| selector | the selector itself: `.id` |
| top-level definition with a signature whose value is a function | its name |
| revset from a label literal | `%name` |
| shape | its type name: `Commit`, `Id` |
| lambda closure | its source text, exactly as written |
| partial application | the function's rendering followed by the arguments' renderings, each parenthesised if not atomic: `describe "wip"`, `at (parents) (describe "x")` |

A lambda's source text runs from its `\` to the last token of its body,
comments within it included, and a section's is the section itself, which
unlike a lambda is an atom: `map (+ 1)`, `(\x y -> x + y) 1`.

A top-level definition with a signature whose value is a function renders as
its name whatever that function is built from, and its partial application
as its name followed by the arguments it was given, never by those its value
holds: `prev`, not `prev (parents)` (with `prev = goto parents`), `tree`, not
`treeWith ({ … })`, and `addT 1` (§4.13). Any other definition renders as its
value: `user` as its record, and without signatures `inc = \x -> x + 1` as
`\x -> x + 1` and `incAll = map inc` as `map (\x -> x + 1)`.

Line breaking: one line if it fits in 80 columns (display width, as in §5.1);
otherwise the outermost list or record breaks one element per line, two-space
indented, with leading commas for records and nothing between list elements,
recursively.

Since `show` returns a `Text` and a top-level `Text` displays raw,
`j show . focus` prints the literal of the focused commit, and
`show . f` does the same for any `f`.

---

## 6. Configuration

### 6.1 Location and loading

`j` reads `$XDG_CONFIG_HOME/j/config.j`, defaulting to `~/.config/j/config.j`.
It is read by every invocation except `remote`, `fetch`, `undo`, `redo`, and
`ops`: expression evaluation and `push` need its definitions, `init` and
`clone` need `user`. When there is no repository yet (`init`, `clone`), only
`user` is evaluated; `Id` literals are not resolved and no other definition
is evaluated. If it does not exist, `j` exits 3 with
`j: no config at <path>; the reference config.j is installed at <share-path>`.
The binary is distributed with the reference `config.j` (§9) at
`<prefix>/share/j/config.j`; `j` never writes to the config path itself.

### 6.2 Validation

Loading fails (exit 3) if any of these hold:

1. The file does not parse (§3).
2. A signature without a definition names something the interpreter does not
   implement; or a name has two signatures; or a signature is not immediately
   followed by its definition when one exists.
3. A name is defined twice, or a definition uses the name of a builtin the
   interpreter implements (those names are reserved whether or not declared).
4. A `typedecl` name is declared twice.
5. An `Id` literal fails to resolve (§4.10).
6. `user`, `immutable`, `tree`, or `labelled` is absent; or `user` is not a
   record with non-empty `name` and `email` texts; or `immutable`, `tree`, or
   `labelled` is not a function.

Loading does **not** check types statically. Signatures are enforced at
each call, one level deep (§4.13). The *presence* of a signature with no
definition is what brings a builtin into scope; an implemented builtin that is
not declared is simply unbound, and using it is a runtime crash.

Definitions may reference each other in any order.

### 6.3 Definitions the interpreter reads

Four names in `config.j` are read by the interpreter itself, not only by
expressions:

- `user`, a record `{ name, email }`: the identity that authors commits
  (§7.9).
- `immutable`, a `Revset` that persistence evaluates to decide what may not be
  rewritten (§7.5).
- `tree`, used by display to render a `Repo` (§5.1).
- `labelled`, which a label literal `%name` desugars to (§4.11).

All four are ordinary definitions; the reference `config.j` provides them and
a user edits or redefines them.

---

## 7. Repositories

`j` operates on jj repositories through jj-lib. It creates jj commits,
operations, and working-copy state that `jj` itself can read, and vice versa,
but nothing here requires the `jj` binary to be installed.

### 7.1 Locating the repository

Walk up from the current directory to the first directory containing `.jj`.
Only the default workspace is supported; if the working directory belongs to
another workspace, exit 2.

### 7.2 Building the `Repo` value

The history of the repository is loaded at the current operation and mapped to
a value as follows.

- **Commits.** Every visible commit (an ancestor of a visible head), including
  jj's root commit, becomes a `Commit` record: `files` from its tree (§7.3),
  `message` from its description, `labels` from the remote bookmarks of
  `origin` pointing at it (§7.6), `id` from its change id.
- **Tree.** The root commit is the top of the tree. Each commit's parent is
  its **first** jj parent. A commit with two or more parents (a merge) is
  therefore placed under its first parent and its other parents are not
  represented; such a commit is *read-only* (§7.5). If two visible commits
  share a change id, exit 2 with
  `j: change <id> has two visible commits; j cannot operate on this repository`.
- **Child order.** A commit's children are ordered by committer timestamp
  ascending, ties broken by change-id text ascending. Within a program,
  `attach` appends. The order is presentation only: no command and no law
  depends on which child is first, and the language has no way to select a
  child by position.
- **Focus.** The zipper is focused on the default workspace's working-copy
  commit, after snapshotting (§7.4). `context` is computed from the path to the
  root. If that commit is in the immutable set (§7.5) — typically because a
  `fetch` found the remote's `main` pointing at it — the snapshot cannot go
  into it; the focus is instead a new empty child of it holding the snapshot,
  as `abandon` would leave it, and persisting records that child. Nothing
  else is written by loading. The value as loaded (§1.2 step 8) has the same
  child holding the commit's own files, so `j id` records the child exactly
  when the working directory differs from that commit.

The record is the *semantics*. An implementation may back `files`, `children`,
and `context` with lazily materialised structures, provided every observation
an expression can make agrees with the eager value. If materialising one of
them, or a file's content, fails (an object missing from the store), every
later use of it fails the same way: a crash caught by `or` (§4.7) never leaves
some other value in its place.

### 7.3 Files, blobs, and `replay`

- A commit's `files` has one `Entry` per tracked path in its tree. `path` is
  the list of path components; components must be valid UTF-8, and a
  repository with a path that is not exits 2. `content` is a `Blob` carrying the file's
  content and type (regular, executable, symlink) and, for conflicted paths,
  jj's conflict value for that path. The entries' order carries no meaning,
  as a tree has none: persistence (§1.2 step 8, §7.5), the laws of §8 and
  the tree's empty mark compare snapshots path by path, though `==` compares
  them as lists (§4.5).
- A path that is deleted and resolved has no entry. A path that is in a
  deletion conflict has an entry whose blob is unresolved. Each side of a
  conflict is either absent or content with its own file type, so a side
  that deleted the path is not an empty file. In a conflict between a file
  and a directory, which jj keeps as one path, a directory side reads as
  empty content but is kept whole, and is written back as that directory.
- `replay onto { from, to }` is jj's tree merge: for each path present in
  any of the three snapshots, the three-way merge of that path's values with
  `from` as the common ancestor, replaying the change from `from` to `to` on
  top of `onto`. Conflicts are kept as unresolved blobs, and resolved where
  jj's merge would resolve them (identical changes, changes from the current
  content, non-overlapping line-level hunks). A path unresolved in any of the
  three takes part with all its sides, as when jj rebases a conflicted commit,
  so sides that cancel drop out and a replay can resolve a conflict as well
  as keep one. It never crashes on content, except where a file meets a
  directory (below). It crashes if any of the three is not a well-formed
  snapshot (a list of `Entry` records with unique paths, none of them the
  root `./`, and none both a file and a directory as `./a` beside `./a/b`
  would be).
- A path that is a file in one of the three snapshots and a directory in
  another is merged as one value, as jj does, the directory with everything
  below it. When at most one of `onto` and `to` changed it from `from`, that
  value is taken whole. When both did and none of the three holds a
  conflict at or below the path, the result is their value taken whole if
  they made the same change, and otherwise a conflict at the path, its
  directory sides kept whole. A conflict one of the three holds there takes
  part with all its sides, as above, so the result is a conflict at the path
  unless sides cancel, even where both made the same change: a child that
  resolves a conflict at `./a` into `./a/b`, replayed with `from` that
  conflict onto the same resolution, keeps a conflict at `./a`. `replay`
  crashes, naming the path, when its result would be a conflict at the path
  with a file on one side and on another a directory that `onto` or `to`
  holds, as replaying the addition of `./a/b` onto a file `./a` would: one
  entry cannot list the directory's entries. It does not crash where one of
  the three already holds a conflict at that path (an unresolved blob there,
  not one below it): the sides came in with that conflict and are carried
  like any others, so a child that resolves such a conflict can be squashed
  into it. The in-memory backend of §10 merges such a path file by file,
  like any other, keeps no directory as a side, and crashes when its result
  would hold both `./a` and `./a/b`. So here, as where jj merges lines, the
  two backends may differ: it resolves some merges that jj keeps as a
  conflict or refuses (a change from a file `./a` to `./a/b` replayed onto a
  snapshot without `./a` gives `./a/b`, and so does one from a conflict at
  `./a` to `./a/b` replayed onto `./a/b`; the deletion of `./a/z` replayed
  onto a snapshot where `./a` is a file gives that file), and it may crash
  where jj carries a conflict that one of the three held at `./a`.
- `unresolved`, `blob`, and `text` are as in §4.9. `blob` produces a regular
  file. File type (executable, symlink) travels with a `Blob` but cannot be
  changed in the language. `text` of a symlink is its target; `text` of a
  non-UTF-8 blob crashes.

### 7.4 Working-directory invariant

At every program boundary the files of the focused commit equal the working
directory.

- **Before evaluation**, the working directory is snapshotted into the
  working-copy commit, and that is the `files` of the focus in the `Repo`
  value. The snapshot tracks every file not matched by `.gitignore` files,
  with no size limit, symlinks as symlinks; these rules are fixed, since `j`
  reads no jj configuration. If the snapshot changed the commit, that change
  is part of the operation persisted afterwards, or discarded with the rest if
  nothing is persisted. A crashed or printing program leaves the working copy
  and the repository exactly as before; `j id` is the way to record the
  working directory and nothing else (§1.2).
- **After persistence**, the focus of the result is checked out: its files are
  written to the working directory, unresolved blobs materialised with jj's
  conflict markers, and the workspace's working-copy commit set to the focus.
  A later snapshot reads such a file back as the conflict its markers
  describe, edits included, while they parse at the length they were written
  with (longer than 7 when a side holds a marker-like line); once they do
  not, the file is resolved to its text.

Reserved commands never snapshot.

### 7.5 Persistence

Given the loaded value `old` and the result `new`, one jj operation is
recorded, described by the expression text (truncated to 200 characters).
Within it:

1. **Validate shape.** `new` has the shape of a `Repo`; every `id` occurs
   exactly once; every snapshot has unique paths, none of them the root `./`
   and none both a file and a directory (§7.3); every path component of the
   focus, of every other commit that is not in `old`, and of every path any
   other commit holds that it did not hold in `old` (§7.3), is a name a
   checkout can create: not empty, `.`, `..`, `.git` or `.jj`, without `/`
   or NUL, and a name the workspace's filesystem can hold. A name the working
   directory holds (§7.4) is one. A name longer than 255 bytes, the limit of
   ext4, is tried on the filesystem, since exFAT, NTFS and APFS count 255
   UTF-16 units or characters instead; where a filesystem's limit is lower,
   or a whole path outgrows the system's limit, the checkout of step 7
   refuses it instead. Where the working directory's filesystem folds case
   (`.JJ` names its `.jj`), `.git` and `.jj` are refused in any letter case.
   (Any other path is neither checked out nor written with a name jj has not
   already stored, and a fetched branch can hold any name git can, even one
   above the focus that a snapshot rebases.) Violations crash with a message
   naming the rule.
2. **Validate labels.** The set of `(id, label)` pairs in `new` equals the set
   in `old`. Labels are the remote's names (§7.6); a script cannot add, move,
   or remove one. In particular a commit carrying a label cannot be abandoned.
3. **Validate immutability.** The *immutable set* is the config's `immutable`
   revset evaluated against `old`, together with every merge commit (§7.2)
   and all of its ancestors. Every id in it must be present in `new` with the
   same parent id, files, and message. Violations crash naming the commit.
   (Ancestors of merges are included because rewriting an ancestor rewrites
   the merge, whose second parent the model cannot represent.) They are the
   merge's jj ancestors through every parent, not only the first, so a
   commit reached only through a second parent is immutable too.
4. **Walk `new` top-down.** For each commit `c` (root first):
   - If `c.id` is the root commit's change id, it must be the top of the tree
     and must be unchanged; otherwise crash.
   - If `c.id` is not in `old`: create a jj commit with parent = the jj commit
     written for `c`'s parent in this walk, tree from `c.files`, description
     `c.message`, author and committer from `user` (§7.9) and the current
     time, change id `c.id`.
   - Otherwise, if `c`'s parent's *written* commit hash, or `c.files`, or
     `c.message` differs from the stored commit: rewrite it (new commit hash,
     same change id, stored as a successor so jj records predecessors),
     keeping the author and author date, with committer `user` and the
     current time.
   - Otherwise keep the stored commit.
5. **Abandon** every change id in `old` that is not in `new`.
6. **Focus.** The focus must be mutable: if `new.root.id` is the root commit
   or in the immutable set, crash with
   `the focus must be a mutable commit; compose with new`. Otherwise set the
   default workspace's working-copy commit to the commit carrying
   `new.root.id`. (`abandon` and `squash` end with `new` for this reason, so
   they are safe on a commit directly above `main`; navigation onto `main`
   is written `new . goto trunk`.)
7. Perform the checkout of §7.4, then commit the operation. A checkout
   that fails (a path the filesystem cannot hold, a full disk) crashes
   without committing it, so nothing is recorded. Unless the working
   directory changed while the program ran, what the checkout wrote is
   then put back: each path at which the focus differs from the working
   directory gets back what the directory held there, and each directory
   the checkout created is removed while empty. No other path is touched: a
   file created or edited elsewhere meanwhile keeps its content, and an
   ignored file stays, even where a `.gitignore` the checkout wrote no
   longer ignores it or where the focus adds a file of that name (the
   checkout does not write over it). When something is in the way of a
   path, the crash says the directory could not be put back, and the next
   run records what it holds.

Labels are not written by persistence at all; they are derived from remote
refs, which only `fetch` and `push` change. The interpreter may run any part
of the walk through jj-lib's rebase machinery provided the stored result
equals what the steps above produce.

### 7.6 Labels and the remote

A label is the name of a bookmark on the remote `origin`, as of the last
`fetch` or `push`. There is one namespace. Labels are read-only in the
language: `fetch` and `push` are the only things that change them.

- **`fetch`** fetches from `origin` over git and imports refs. Afterwards
  each remote bookmark `name` is the label `name` on the commit it points at;
  labels for bookmarks that no longer exist on the remote are removed; commits
  reachable from remote bookmarks become visible. Fetching never moves the
  focus or touches the working directory. It records one operation.
- **`push EXPR`** evaluates `EXPR` (§1.1); a function result is applied to
  the recorded repository. The value must be a list whose elements are records
  of two shapes:
  - `{ id : Id, name : Text }` — set bookmark `name` to the commit `id`,
    creating it if absent;
  - `{ delete : Text }` — delete the bookmark of that name.

  Duplicates are merged; two records with the same `name` and different
  effects are a crash. It is refused (exit 1, nothing pushed) if:
  - a `name` is not a valid git branch name, an `id` is not a visible commit,
    or a delete names a bookmark the remote does not have;
  - any commit that would be sent has an unresolved file or an empty
    description (the commits that would be sent are the records' `id`s and
    their ancestors that no bookmark on `origin` reaches as of the last
    `fetch` or `push`; a delete sends none);
  - the bookmark's current target is in the immutable set (§7.5) and the
    record would move it to a commit that does not descend from it, or delete
    it while no bookmark left on `origin` after the push reaches that target
    (those left are the bookmarks no record names, where they are as of the
    last `fetch` or `push`, and the records' new targets);
  - the deletes would wait (below) and a bookmark it sets and one it deletes
    are a name and a name under it, such as `master` and `master/legacy`:
    git cannot hold both, so the first git push could not succeed.

  Targets and descent are commits as the remote has them: a target rewritten
  here since the last `fetch` or `push` is hidden, and not in the immutable
  set although the visible rewrite carries its change id and its label.
  Moving a bookmark whose current target is not immutable to an unrelated
  commit is allowed; that is how a rewritten stack is re-pushed. Deleting a
  bookmark on an immutable commit that another bookmark still reaches is how
  a merged feature's name is removed. It loses no history as long as that
  other bookmark is still where the last `fetch` or `push` left it: `push`
  does not ask the remote, so it does not notice a bookmark moved or deleted
  there since. The remote applies each update of a git push on its own, so
  when only records' new targets reach the commit, as when
  `rename "master" "main"` renames trunk's only bookmark, the deletes wait:
  the other updates are sent first, and the deletes in a second git push
  only once the remote has accepted every one of them. To rename trunk's
  only bookmark to a name under it, push a temporary label on its commit
  first, `push (label "tmp" (labelled "master"))`: with it on `origin`
  nothing waits, and `rename "master" "master/legacy"` goes as one git
  push; then `push (unlabel "tmp")`. After a successful push the labels
  reflect the new positions. It records one operation, even when it sends
  two git pushes. An empty list sends nothing to the remote.
  If the remote accepts some of the updates and rejects others, that
  operation records the accepted ones, whose labels move, and `push` exits 1
  naming the rejected bookmarks and any deletes it did not send.

Only the remote named `origin` is supported in this version; `fetch` and
`push` exit 1 with `j: no remote origin` when it is not configured.
Authentication is git's: the ssh agent for ssh URLs, git credential helpers
for https, as jj-lib's git backend provides them; `j` adds no prompting.
Bringing work up to date is a separate expression, e.g. `rebase trunk`.

### 7.7 Operations, undo, redo, and the lock

Every persisting `j` program, and each of `fetch` and `push`, records exactly
one jj operation, described by the expression or command text. Crashes record
nothing.

**Kinds.** `j` marks each operation it records, in the operation's metadata,
as *normal*, *undo* (naming the operation it undid), or *redo* (naming the
undo it reversed). Operations recorded by other tools are treated as normal,
except that an operation whose description carries jj's own undo or redo
marker is treated accordingly, so mixed use chains correctly where the marker
is present and degrades to "undo the newest operation" where it is not.

**`undo`** finds the operation to undo: starting at the head, while the
current operation is an undo, move to the operation it undid and then to that
operation's parent; while it is a redo, move past the operation it reapplied.
The first normal operation reached is undone by recording a new operation
that restores its parent's view, marked as an undo of it. If none is found,
or the one found is the operation `init` or `clone` recorded (§7.8), before
which there was no repository, exit 1 with `j: nothing to undo`.

**`redo`** requires the head to be an undo (possibly reached through redos);
it records an operation restoring the view of the operation that undo
discarded, marked as a redo. Otherwise exit 1 with `j: nothing to redo`.

Both refuse, before doing anything, if the working directory differs from the
focused commit's files: `j: working copy has changes not in @; run \`j id\`
to record them or discard them`. Both check out the restored focus before
recording their operation; when that checkout fails they record nothing,
and what it wrote is put back as §7.5 step 7 says.
Undoing a `push` restores the recorded labels and leaves the remote untouched.

**`ops`** prints the operation log, newest first, one line each: relative
time, the operation's description (a `j` expression, a command, or another
tool's description), and `undo`/`redo` where applicable; the current
operation is marked. Rendered in the binary like `tree`.

**Lock.** A run that may persist (any expression evaluation, `fetch`, `push`,
`undo`, `redo`) takes an exclusive lock, the file `.jj/j.lock`, for its
duration.
A second `j` exits 2 with `j: another j is running in this repository`.
Display-only runs are not distinguishable in advance, so they take the lock
too; only reserved commands that need no repository (`init`, `clone`) do not.
The lock prevents `j` from ever producing two commits with one change id.
Other tools operating concurrently are outside `j`'s control; if they produce
that state, §7.2 applies.

### 7.8 Creating and connecting repositories

- **`init`** creates a colocated repository (both `.jj` and `.git`) in the
  current directory. If `.git` already exists and `.jj` does not, jj is
  initialised on top of the existing git repository and its history is
  imported (subject to §7.2). Either way a working-copy commit is created as a
  child of the current head (or of the root commit for an empty repository)
  and checked out; the current head is git's `HEAD`, also when it is
  detached. Over an existing git repository the checkout writes no file: the
  working directory is taken as it is, and what it holds beyond the head
  becomes the working-copy commit's change. It exits 2 if the directory is
  already inside a jj repository, or if `.git` exists but is not a directory
  (a git worktree or submodule). It requires `user` (§7.9).
- **`clone URL [DIR]`** creates `DIR` and its missing parents (exit 2 if
  `DIR` exists and is non-empty, or cannot be created, which includes a `DIR`
  such as `new/..` that names an existing directory once its parents exist),
  clones `URL` into it as a colocated repository, sets `origin` to `URL`,
  fetches, and creates a working-copy commit as a child of the remote's
  default bookmark target (or of the root commit if the remote has none),
  holding that target's files (see below), and checks it out. The default
  bookmark is the branch the remote's `HEAD` names. A clone that fails, its
  checkout included, records nothing and removes the `DIR` it created and each
  parent it created that is then empty, or empties the `DIR` it found empty,
  and nothing else. It requires `user`.
- **`remote URL`** sets the URL of `origin`, creating the remote if it does not
  exist. It does not fetch.

Besides jj's own record of the new workspace, `init` and `clone` each record
at most one operation, described `init` and `clone URL`, holding what they
imported and the working-copy commit; `undo` never undoes it (§7.7). That
commit holds its parent's files less any at a path a checkout cannot create
(§7.5 step 1), such as a committed `.jj` directory: their removal is its
change.

### 7.9 Identity

Commits are authored and committed as `user.name <user.email>` from
`config.j` (§6.3). `j` reads no other configuration; jj-lib's settings object
is constructed by `j` from `user` and fixed defaults. `init`, `clone`, and
persistence exit 3 if `user` is missing or malformed, since that is a
configuration error.

### 7.10 External diff rendering

`difft p a b` runs difftastic. It is the only builtin that executes another
program.

1. Write the content of `a` and `b` to two temporary files whose names end in
   the last component of `p` (so difftastic's language detection sees the
   real filename), inside a fresh temporary directory.
2. Run `difft OLD NEW` with the process's stdin closed and its environment
   inherited unchanged, except: if `DFT_COLOR` is unset and `j`'s stdout is a
   terminal, set `DFT_COLOR=always`; if `DFT_WIDTH` is unset and `j`'s stdout
   is a terminal, set it to the terminal width.
3. Capture stdout. Delete the temporary directory.
4. Return stdout as `Text`. A non-zero exit status is not an error (difftastic
   uses it to signal "differences found" under some options); a failure to
   execute the program is a crash naming it.

All rendering options are difftastic's own, set through its `DFT_*`
environment variables. `j` adds no options of its own. `diff` remains
available as a dependency-free unified diff; the reference `config.j` uses
`difft` for display, so a user without difftastic edits one line.

Packaging: difftastic is a runtime dependency; on Nix, wrap `j` with `difft`
on its `PATH`.

### 7.11 Tree rendering

`treeWith options r` renders the history as text. It is implemented in the
binary because it is presentation, not semantics. It reads the `Repo` value,
the immutable set (§7.5), `diff` sizes, and (for the margin, ages, and the
sibling order) jj's commit metadata by id.

#### Options

```
{ detail : Int      -- 0, 1, or 2; see below
, margin : Bool     -- age and author column
, elide  : Bool     -- fold uninteresting runs
, icons  : Bool     -- pictographic glyph set
, color  : Text }   -- "auto", "always", "never"
```

Commits with no stored counterpart — minted in this program, as in a dry
run — have no metadata: their margin cells are blank, their size bar is
computed from the value, and they sort last among their siblings. The same
holds for a `Commit` block in display (§5.1).

Missing fields crash. `color = "auto"` means colour iff stdout is a terminal.
If the environment variable `NO_COLOR` is set and non-empty, colour is never
used regardless of `color`. `config.j` fixes the defaults by defining `tree`.

#### Layout

Root at top, one line per commit, plus optional detail lines under the focus.

```
   ⌂ zzzz
   ╎ 14
   ◆ kpqx ▅  add parser                  main                   3d  mo
   ├─ ○ mnrv ▂  fix lexer                                        1d  mo
 ▶ ├─ ⊗ wqzt ▃  wip                      feature                2h  mo
   │      ✖ src/lexer.rs   ~ src/parser.rs   + tests/lexer.rs
   │  └─ ○ yxsk ▃                                                1h  mo
   └─ ◌ ptlm    docs  ⋯ 3                                       5d  ak
```

(That is `detail = 2, margin = true`. With `detail = 1, margin = false`, the
size bars, the margin, and the detail line are absent.)

Columns, left to right:

1. **Gutter**, two characters: `▶` on the focus line, blank otherwise.
2. **Rails**: `│ ├─ └─`, two columns per depth; `╎ n` for an elided run
   (§Elision).
3. **Node glyph**: what kind of commit.
4. **Id**: shortest prefix unique among visible commits, minimum 4.
5. **Size bar** (`detail ≥ 1`, focus and its parent and children only; all
   commits at `detail = 2`): one of `▁ ▂ ▃ ▅ ▇`, lines added plus removed
   by a shortest line diff against the parent, thresholds 1, 10, 50, 200,
   1000. Absent for empty commits. A changed path with no line added or
   removed (an empty file, a new file type) counts as one line.
6. **Message**: first line; nothing if empty.
7. **Labels**: right-aligned column, present only if any rendered commit has
   a label; names separated by two spaces.
8. **Margin** (`margin = true`): age and author initials, right-aligned.
   Age is the committer time, rendered `Ns`, `Nm`, `Nh`, `Nd`, `Nw`, `Ny`.
   Initials are the first letters of the author name's words, at most two.
9. **Collapse marker** `⋯ n` after the message of a collapsed subtree's top.

**Detail line** (`detail = 2`, focus only): indented under the focus, the
changed paths with a mark each: `+` added, `~` modified, `−` deleted,
`✖` unresolved. Renames are not detected; they appear as `−` and `+`.

#### Glyphs

| glyph | meaning | `icons = true` |
|---|---|---|
| `◉` | focus | `🌸` |
| `●` | ancestor of the focus | `🌿` |
| `○` | other commit | `🍃` |
| `◆` | in the immutable set (§7.5) | `🪨` |
| `◌` | empty: no change against its parent | `🫙` |
| `⊗` | has unresolved files | `🔥` |
| `⌂` | root | `🌱` |

Precedence when several apply: `⌂`, then `⊗`, then `◌`, then `◆`, then the
position glyphs; position is then carried by colour (and, for the focus, by
`▶`).

#### Elision (`elide = true`)

A commit is *interesting* if it is the focus, an ancestor of the focus, a
child of an ancestor of the focus, labelled, conflicted, a leaf, or has more
than one child. Runs of consecutive uninteresting commits on a single line of
descent are folded into `╎ n`. Subtrees whose top commit is not the focus, an
ancestor of it, or a child of an ancestor are rendered as their top commit
followed by `⋯ n` (omitted when `n = 0`), where `n` counts hidden descendants.
With `elide = false` everything is shown.

#### Colour

Applied only when colour is on. Glyphs always carry the meaning; colour only
adds emphasis, so a colourless rendering loses nothing.

| element | treatment |
|---|---|
| focus line | bold, faint background |
| ancestors of the focus | normal |
| all other lines | dim |
| rails on the path from the top to the focus | cyan |
| other rails, `╎ n`, `⋯ n` | dim |
| id of a conflicted commit | red |
| id of an immutable commit | blue |
| message of an empty commit | dim italic |
| labels | green |
| size bar | one accent colour, brighter with size |
| detail marks `+` `−` `~` `✖` | green, red, yellow, red |
| age | grey scale: brighter when more recent |
| author initials | a hue chosen per author, stable across runs (hash of the name) |

Messages are never coloured except dim (cousins) and dim italic (empty).

---

## 8. Laws

These hold for the reference `config.j` and should be property-tested against
an in-memory implementation of the builtins (§10). `=` means the results are
equal, with snapshots compared path by path (§7.3), or both crash.

```
by r.root.id r                      = r
prev (next r)                       = r          (when r has exactly one child)
next (prev r)                       = r          (when r is an only child)
top (top r)                         = top r
tip (tip r)                         = tip r
(abandon . new) r                   = r          (up to the minted id of the new focus)
(abandon . contract m . split m) r  = r          (up to minted ids)
rebase parents r                    = r
goto here r                         = r
into i r                            = by i r     (when i is a child of the focus)
at rs id r                          = r
(e or id) r                         = r          (when e r crashes)
ancestors r                         = r.root.id :: ancestors (up r)   (when up r exists)
forEach (const []) e r              = r
eachChild id r                      = r
replayOnto (up r).root r            = r
rewrite id r                        = r
replay b { from = b, to = x }       = x
replay x { from = b, to = b }       = x
invert (invert ch)                  = ch
touched (invert ch)                 = touched ch
from here f                         = f
concat [xs]                         = xs
concat (xs ++ ys)                   = concat xs ++ concat ys    (both non-empty)
e or x                              = e          (when e does not crash)
crash m or x                        = x
```

---

## 9. Reference `config.j`

This file is the complete definition of the language's vocabulary. It ships
with the binary and is the version installed to `~/.config/j/config.j`.

```haskell
-- config.j — everything j knows.
--
-- j reads this file on every run. An expression can use a name only if this
-- file declares it (a builtin, implemented by j) or defines it (written here,
-- in the language). Nothing else exists.
--
-- Conventions in this file:
--   Name = …          a shape: a record layout, matched by its field names
--   name : Type       a signature. Alone, it declares a builtin implemented
--                     by j. Before a definition, it documents that definition.
--                     Either way j checks arguments and results against it at
--                     each call, one level deep: kind, list-ness, field names.
--   name = expr       a definition. Functions are lambdas: name = \x -> …
--
-- Edits.  Most definitions are Edits: functions from a Repo to a Repo. On the
-- command line a function result is applied to the current repository; a Repo
-- result is persisted. Anything else is printed. `e2 . e1` runs e1 then e2;
-- `e or id` runs e and leaves things alone if it crashes.
--
-- Four definitions here are read by j itself: `user` (who commits), `tree`
-- (how a Repo is printed), `immutable` (which commits may never be
-- rewritten), and `labelled` (what a %name literal means).


------------------------------------------------------------------------------
-- You
------------------------------------------------------------------------------

user : { name : Text, email : Text }
user = { name = "Your Name", email = "you@example.com" }


------------------------------------------------------------------------------
-- Shapes
------------------------------------------------------------------------------

Path     = [Text]                     -- a file path as its components; written ./a/b
Label    = Text                       -- a bookmark name on the remote

Entry    = { path : Path, content : Blob }
Snapshot = [Entry]                    -- the files of one commit; paths unique;
                                      -- a deleted, resolved path has no entry
Change   = { from : Snapshot, to : Snapshot }   -- the files before and after

Commit   = { files   : Snapshot
           , message : Text
           , labels  : [Label]       -- the remote's names for this commit;
                                     -- read-only: only fetch and push change them
           , id      : Id }          -- each id is on exactly one commit

Subtree  = { root : Commit, children : [Subtree] }

-- A Repo is the whole history, seen from one commit: the focused subtree plus
-- the path back to the top. Each Frame is one ancestor with a hole where the
-- focused subtree was; left and right are the siblings on either side. Sibling
-- order is presentation only.
Frame    = { parent : Commit, left : [Subtree], right : [Subtree] }
Repo     = { root : Commit, children : [Subtree], context : [Frame] }
                                      -- context = [] at the top of history

Detached = { subtree : Subtree, rest : Repo }
Rebase   = { from : Snapshot, onto : Snapshot }   -- the base a subtree was written on, and the new one
Meta     = { hash : Text, author : Text, email : Text, time : Int }
                                      -- what jj records about a commit beyond the model

Edit     = Repo -> Repo               -- a command; may crash
Fileset  = Path -> Bool               -- which paths a command applies to
Revset   = Repo -> [Id]               -- a set of commits, computed from a repo

Push     = { id : Id, name : Label }  -- set the remote's bookmark `name` to `id`
Drop     = { delete : Label }         -- delete the remote's bookmark


------------------------------------------------------------------------------
-- Builtins: functions and control
------------------------------------------------------------------------------

(.)     : (b -> c) -> (a -> b) -> a -> c   -- composition, right to left
id      : a -> a
const   : a -> b -> a
crash   : Text -> a                        -- abort the program with a message

-- `a or b` is a keyword, not a builtin: the value of a; or, if evaluating a
-- crashed, the value of b. When a is a function and b is one too, the result
-- is pointwise, (f or g) x = f x or g x, so `edit or id` tries an edit and
-- keeps the repository if it fails. Edits compose with `.`: `tree . squash`
-- squashes, then renders.

------------------------------------------------------------------------------
-- Builtins: comparison, booleans, integers
------------------------------------------------------------------------------

(==)    : a -> a -> Bool                   -- structural; functions crash
(/=)    : a -> a -> Bool
(&&)    : Bool -> Bool -> Bool             -- short-circuit
(||)    : Bool -> Bool -> Bool             -- short-circuit
not     : Bool -> Bool
(+)     : Int -> Int -> Int
(-)     : Int -> Int -> Int
(*)     : Int -> Int -> Int
(<)     : Int -> Int -> Bool
(<=)    : Int -> Int -> Bool
(>)     : Int -> Int -> Bool
(>=)    : Int -> Int -> Bool
show    : a -> Text                        -- any value, in this language's syntax

------------------------------------------------------------------------------
-- Builtins: lists and text
------------------------------------------------------------------------------

(::)    : a -> [a] -> [a]                  -- cons
map     : (a -> b) -> [a] -> [b]
filter  : (a -> Bool) -> [a] -> [a]
length  : [a] -> Int
null    : [a] -> Bool
head    : [a] -> a                         -- crash on []
tail    : [a] -> [a]                       -- crash on []
last    : [a] -> a                         -- crash on []
nth     : Int -> [a] -> a                  -- 0-based; crash out of range
take    : Int -> [a] -> [a]
drop    : Int -> [a] -> [a]
member  : a -> [a] -> Bool
range   : Int -> Int -> [Int]              -- range a b = [a (a+1) … (b-1)]
foldl   : (b -> a -> b) -> b -> [a] -> b   -- foldl f z [x y] = f (f z x) y

-- ++ and concat work on lists and on text. concat [] crashes: supply a
-- default with or, e.g.  concat xs or []
(++)    : m -> m -> m
concat  : [m] -> m

startsWith : Text -> Text -> Bool
endsWith   : Text -> Text -> Bool
splitOn    : Text -> Text -> [Text]        -- splitOn "/" "a/b" = ["a" "b"]

------------------------------------------------------------------------------
-- Builtins: the repository
------------------------------------------------------------------------------

-- Replay a change onto a snapshot: "onto, with the change from `from` to `to`
-- applied." Where the change collides with what is already there, the path's
-- blob is unresolved rather than an error, except that a collision making a
-- path a file on one side and a directory on another, as adding ./a/b onto a
-- file ./a would, crashes, naming the path (§7.3).
replay     : Snapshot -> Change -> Snapshot
unresolved : Blob -> Bool
blob       : Text -> Blob                  -- a resolved regular file
text       : Blob -> Text                  -- its content (markers if unresolved)
by         : Id -> Repo -> Repo            -- focus the commit with this id; crash if absent
subtreeCommits : a -> [Commit]             -- every commit of a tree in preorder, in one
                                           -- pass: what `commits` (below) is
meta       : Id -> Meta                    -- git hash, author, email, committer time;
                                           -- crash for a commit not yet persisted
validate   : Repo -> Repo                  -- the repo unchanged, or the crash persisting
                                           -- it would produce (shape, labels, immutability,
                                           -- focus). `tree . validate . rebase trunk` is an
                                           -- honest dry run.
diff       : Blob -> Blob -> Text          -- unified diff from the first to the second
difft      : Path -> Blob -> Blob -> Text  -- difftastic's rendering; needs difft on PATH.
                                           -- Options via DFT_* environment variables.
treeWith   : { detail : Int, margin : Bool, elide : Bool, icons : Bool, color : Text }
             -> Repo -> Text              -- the history as a tree; see the spec

-- Every subvalue of a given shape, however deeply nested. A shape is a type
-- name used as a value: extract Commit x, extract Id x, extract Push ys.
extract    : Shape -> a -> [b]

-- Ids: an existing commit is written @kpqx (any unique prefix; checked before
-- the program runs). A bare @ is a new id, distinct each time it is evaluated.
-- A label is written %main and is a Revset: the commit the remote calls "main".


------------------------------------------------------------------------------
-- Zipper. A Repo is focused on one commit; these move or detach the focus.
-- Commands never select a commit by position: use ids, labels, or predicates.
------------------------------------------------------------------------------

-- To the parent. Crashes at the top.
up : Edit
up = \repo ->
  let f = head repo.context
  in { root     = f.parent
     , children = f.left ++ [{ root = repo.root, children = repo.children }] ++ f.right
     , context  = tail repo.context }

-- Detach the focused subtree; the focus moves to the parent. Crashes at the top.
detach : Repo -> Detached
detach = \repo ->
  let f = head repo.context
  in { subtree = { root = repo.root, children = repo.children }
     , rest    = { root = f.parent, children = f.left ++ f.right, context = tail repo.context } }

-- To the top of history.
top : Edit
top = \repo -> top (up repo) or repo

-- Every commit of a subtree (or of the focused subtree of a Repo), in preorder.
-- Takes anything with `root` and `children`, so the signature is left open.
-- The builtin is  \t -> t.root :: (concat (map commits t.children) or [])
-- in one pass; written that way it copies the list below every commit, and
-- so takes time quadratic in the length of the history.
commits : a -> [Commit]
commits = subtreeCommits


------------------------------------------------------------------------------
-- Structure. Editing at the focus.
------------------------------------------------------------------------------

mapRoot : (Commit -> Commit) -> Edit
mapRoot = \f repo -> repo { root = f repo.root }

mapChildren : ([Subtree] -> [Subtree]) -> Edit
mapChildren = \f repo -> repo { children = f repo.children }

-- Attach a subtree as a new child of the focus; the focus does not move.
attach : Subtree -> Edit
attach = \t -> mapChildren (++ [t])

-- Remove the focused commit; its children join the parent. Focus to the parent.
remove : Edit
remove = \repo -> let x = detach repo
                  in x.rest { children = x.rest.children ++ x.subtree.children }

-- Attach a commit as a new child of the focus, and move to it.
addChild : Commit -> Edit
addChild = \c repo -> by c.id (attach ({ root = c, children = [] }) repo)

-- A new commit with these files, no message, no labels, and a new id.
newCommit : Snapshot -> Commit
newCommit = \files -> { files = files, message = "", labels = [], id = @ }


------------------------------------------------------------------------------
-- Snapshots, changes, and filesets
------------------------------------------------------------------------------

-- A path is written ./src/lexer.rs, which is ["src" "lexer.rs"]; ./ alone is
-- the root, []. For names the literal cannot spell, splitOn "/" "…" builds one.

restrict : Fileset -> Snapshot -> Snapshot                      -- entries matching m
restrict = \m -> filter (\e -> m e.path)

select : Fileset -> Snapshot -> Snapshot -> Snapshot            -- m from a, the rest from b
select = \m a b -> restrict m a ++ restrict (neg m) b

conflicted : Snapshot -> [Path]
conflicted = \s -> map (.path) (filter (\e -> unresolved e.content) s)

-- What the focus changes: its parent's files, and its own.
changeOf : Repo -> Change
changeOf = \repo -> { from = (up repo).root.files or [], to = repo.root.files }

-- The same change, backwards.
invert : Change -> Change
invert = \ch -> { from = ch.to, to = ch.from }

everything : Fileset
everything = const true

neg : Fileset -> Fileset
neg = \m -> not . m

both : Fileset -> Fileset -> Fileset
both = \m n p -> m p && n p

either : Fileset -> Fileset -> Fileset
either = \m n p -> m p || n p

under : Path -> Fileset                                         -- inside directory d
under = \d p -> take (length d) p == d

ext : Text -> Fileset                                           -- file extension e
ext = \e p -> endsWith ("." ++ e) (last p)


------------------------------------------------------------------------------
-- Revsets. A revset is a function from a Repo to a list of ids, so it can be
-- relative to the focus (parents, kids) or absolute (all, %main).
------------------------------------------------------------------------------

here : Revset
here = \repo -> [repo.root.id]

parents : Revset
parents = \repo -> [(up repo).root.id] or []

kids : Revset
kids = \repo -> map (\t -> t.root.id) repo.children

descendants : Revset                                            -- includes the focus
descendants = \repo -> map (.id) (commits repo)

ancestors : Revset                                              -- includes the focus
ancestors = \repo -> repo.root.id :: map (\f -> f.parent.id) repo.context

siblings : Revset
siblings = \repo -> filter (\i -> i /= repo.root.id) (kids (up repo)) or []

all : Revset
all = \repo -> descendants (top repo)

stack : Revset                                                  -- the line through the focus
stack = union ancestors descendants

labelled : Label -> Revset                                      -- %main is labelled "main"
labelled = \n repo -> map (.id) (filter (\c -> member n c.labels) (commits (top repo)))

-- Evaluate f starting from each commit of rs, and collect.
from : Revset -> Revset -> Revset
from = \rs f repo -> concat (map (\i -> f (by i repo)) (rs repo)) or []

descendantsOf : Revset -> Revset
descendantsOf = \rs -> from rs descendants

ancestorsOf : Revset -> Revset
ancestorsOf = \rs -> from rs ancestors

union : Revset -> Revset -> Revset
union = \a b repo -> let xs = a repo in xs ++ filter (\i -> not (member i xs)) (b repo)

intersect : Revset -> Revset -> Revset
intersect = \a b repo -> let ys = b repo in filter (\i -> member i ys) (a repo)

minus : Revset -> Revset -> Revset
minus = \a b repo -> let ys = b repo in filter (\i -> not (member i ys)) (a repo)

matching : (Commit -> Bool) -> Revset -> Revset
matching = \p rs repo -> filter (\i -> p (commitAt i repo)) (rs repo)

conflicts : Revset
conflicts = matching (\c -> not (null (conflicted c.files))) all

-- The first of several revsets that is non-empty.
firstOf : [Revset] -> Revset
firstOf = \rss repo -> foldl (\acc rs -> if null acc then rs repo else acc) [] rss

-- The main line: whichever of these names the remote has, first one wins.
-- Empty in a repository with no remote, in which case nothing is immutable.
trunk : Revset
trunk = firstOf [%main %master %trunk]

-- Commits that may never be rewritten, abandoned, or focused. j evaluates this
-- before persisting anything and refuses to change what it names. Merge
-- commits and their ancestors are immutable regardless. The focus must always
-- be mutable, so it is `new . goto trunk`, never `goto trunk` alone.
immutable : Revset
immutable = ancestorsOf trunk


------------------------------------------------------------------------------
-- Focusing. `goto` is the only place a revset must have exactly one member.
------------------------------------------------------------------------------

goto : Revset -> Edit
goto = \rs repo ->
  let is = rs repo
  in if length is == 1
     then by (head is) repo
     else crash ("expected one revision, got " ++ show (length is))

commitAt : Id -> Repo -> Commit
commitAt = \i repo -> (by i repo).root

prev : Edit                                                     -- the parent
prev = goto parents

next : Edit                                                     -- the only child
next = goto kids

tip : Edit                                                      -- end of the line of only-children
tip = \repo -> tip (next repo) or repo

into : Id -> Edit                                               -- a child, by id
into = \i repo -> if member i (kids repo) then by i repo
                  else crash "into: not a child of the focus"

child : (Commit -> Bool) -> Edit                                -- the one child satisfying p
child = \p -> goto (matching p kids)

-- Run an edit somewhere else, then come back (by id, so the edit may move things).
at : Revset -> Edit -> Edit
at = \rs e repo -> let me = repo.root.id in by me (e (goto rs repo))

-- Run an edit at every commit of a revset, returning to the focus.
forEach : Revset -> Edit -> Edit
forEach = \rs e repo -> foldl (\acc i -> at (const [i]) e acc) repo (rs repo)

eachChild : Edit -> Edit
eachChild = forEach kids

guard : (Repo -> Bool) -> Edit                                  -- crash unless p holds
guard = \p repo -> if p repo then repo else crash "guard"

clean : Repo -> Bool
clean = \repo -> null (conflicted repo.root.files)


------------------------------------------------------------------------------
-- Rebasing
------------------------------------------------------------------------------

-- Rewrite a subtree as if it had been written on `onto` instead of `from`.
-- Each commit's change is replayed onto the new base; its children are then
-- replayed from its old files onto its new ones.
replayTree : Rebase -> Subtree -> Subtree
replayTree = \rb t ->
  let c  = t.root
      c' = c { files = replay rb.onto ({ from = rb.from, to = c.files }) }
  in { root = c', children = map (replayTree ({ from = c.files, onto = c'.files })) t.children }

-- Replay the focused subtree onto the commit newP, in place. The subtree stays
-- attached where it is; only its files change. Move it afterwards with detach
-- and attach. Crashes at the top.
replayOnto : Commit -> Edit
replayOnto = \newP repo ->
  let t = replayTree ({ from = (up repo).root.files, onto = newP.files })
                     ({ root = repo.root, children = repo.children })
  in repo { root = t.root, children = t.children }

-- Replace the focused commit with f applied to it, replaying its children.
rewrite : (Commit -> Commit) -> Edit
rewrite = \f repo ->
  let c  = repo.root
      c' = f c
  in (eachChild (replayOnto c') repo) { root = c' }


------------------------------------------------------------------------------
-- Commands
------------------------------------------------------------------------------

-- Set the message of the focused commit.
describe : Text -> Edit
describe = \m -> mapRoot (\c -> c { message = m })

-- Start an empty child of the focus and move to it.
new : Edit
new = \repo -> addChild (newCommit repo.root.files) repo

-- Remove the focused commit. Its children are rebased onto its parent, and a
-- new empty commit on that parent becomes the focus (the parent may be
-- immutable, and the focus never is). Refused for a commit the remote has a
-- name for: push something else under that name first, or forget the name.
abandon : Edit
abandon = \repo ->
  let c = repo.root
      p = (up repo).root
  in if not (null c.labels)
     then crash ("abandon: the remote names this commit " ++ show c.labels)
     else new (remove (eachChild (replayOnto p) repo))

-- Push the files matching m from the focus down into its parent.
contract : Fileset -> Edit
contract = \m repo -> let a = repo.root
                      in at parents (rewrite (\q -> q { files = select m a.files q.files })) repo

-- Fold the focus entirely into its parent; a new empty commit becomes the focus.
squash : Edit
squash = abandon . contract everything

-- Push the files matching m from the focus up into a new child, which becomes
-- the focus and inherits the old focus's children. Labels stay with the
-- original commit, since they are the remote's.
split : Fileset -> Edit
split = \m repo ->
  let a  = repo.root
      p  = (up repo).root
      a' = a { files = select m p.files a.files }
      x  = newCommit a.files
  in by x.id (repo { root = a', children = [{ root = x, children = repo.children }] })

-- Move the focused subtree onto another commit. Returns to the moved commit.
rebase : Revset -> Edit
rebase = \rs repo ->
  let d = (goto rs repo).root
  in if member d.id (descendants repo)
     then crash "rebase: the destination is inside the subtree being moved"
     else let me = repo.root.id
              x  = detach (replayOnto d repo)
          in by me (attach x.subtree (by d.id x.rest))

-- Apply the focused commit's change onto another commit, as a new child there.
pick : Revset -> Edit
pick = \rs repo ->
  let ch = changeOf repo
      t  = (goto rs repo).root
  in at rs (addChild (newCommit (replay t.files ch))) repo

-- A new child of the focus that undoes the change of the given commit.
backout : Revset -> Edit
backout = \rs repo ->
  let ch = changeOf (goto rs repo)
  in addChild (newCommit (replay repo.root.files (invert ch))) repo


------------------------------------------------------------------------------
-- Pushing. `j push EXPR` evaluates EXPR to a list of Push and Drop records.
------------------------------------------------------------------------------

-- Label every commit of a revset. `j 'push (label "feature" here)'` labels @.
label : Label -> Revset -> Repo -> [Push]
label = \n rs repo -> map (\i -> { id = i, name = n }) (rs repo)

-- Remove a label on the remote. `j 'push (unlabel "feature")'` after the merge.
unlabel : Label -> [Drop]
unlabel = \n -> [{ delete = n }]

-- Move a label: `j 'push (rename "old" "new")'`.
rename : Label -> Label -> Repo -> [b]
rename = \old new repo -> label new (labelled old) repo ++ unlabel old

-- Every label the remote already has, at its current local commit.
-- `j push relabel` after rewriting a stack moves the remote's bookmarks along.
relabel : Repo -> [Push]
relabel = \repo -> concat (map (\c -> map (\n -> { id = c.id, name = n }) c.labels)
                              (extract Commit repo)) or []


------------------------------------------------------------------------------
-- Inspection. These return printable values rather than a Repo.
------------------------------------------------------------------------------

focus : Repo -> Commit                                          -- the focused commit
focus = \repo -> repo.root

files : Repo -> Snapshot                                        -- its files
files = \repo -> repo.root.files

-- The entry at path p in snapshot s, as a list: [] if absent.
entryAt : Path -> Snapshot -> [Entry]
entryAt = \p s -> filter (\e -> e.path == p) s

-- The content at path p, or an empty file if absent.
contentAt : Path -> Snapshot -> Blob
contentAt = \p s -> (head (entryAt p s)).content or blob ""

-- The paths a change touches.
touched : Change -> [Path]
touched = \ch ->
  let paths = map (.path) ch.to ++ filter (\p -> not (member p (map (.path) ch.to))) (map (.path) ch.from)
  in filter (\p -> entryAt p ch.to /= entryAt p ch.from) paths

-- Paths whose content differs between the focus and its parent.
changed : Repo -> [Path]
changed = \repo -> touched (changeOf repo)

-- What the focus changes, rendered by difftastic, one record per changed path.
-- Replace difft with (\p -> diff) to use the built-in unified diff instead.
diffs : Repo -> [{ path : Path, diff : Text }]
diffs = \repo ->
  let ch = changeOf repo
  in map (\p -> { path = p, diff = difft p (contentAt p ch.from) (contentAt p ch.to) }) (touched ch)

-- The same, as one page of text. A Text result prints raw, so `j review` is
-- readable in the terminal.
review : Repo -> Text
review = \repo -> concat (map (\d -> d.diff) (diffs repo)) or "no changes\n"

-- A summary of the focus.
status : Repo -> { id : Id, message : Text, labels : [Label], changed : [Path], conflicts : [Path] }
status = \repo ->
  { id        = repo.root.id
  , message   = repo.root.message
  , labels    = repo.root.labels
  , changed   = changed repo
  , conflicts = conflicted repo.root.files }

-- A summary of every commit, top first, in preorder.
log : Repo -> [{ id : Id, message : Text, labels : [Label], focus : Bool }]
log = \repo ->
  map (\c -> { id      = c.id
             , message = c.message
             , labels  = c.labels
             , focus   = c.id == repo.root.id })
      (commits (top repo))

-- The history as a tree. Edit these records to change what `j tree` shows.
-- (The parentheses are required: `f { … }` would be a record update of f.)
tree : Repo -> Text
tree = treeWith ({ detail = 1, margin = false, elide = true,  icons = false, color = "auto" })

treeFull : Repo -> Text
treeFull = treeWith ({ detail = 2, margin = true,  elide = false, icons = false, color = "auto" })
```

---

## 10. Testing

The interpreter must be testable without a jj repository. Provide an in-memory
implementation of the domain builtins (`replay`, `unresolved`, `blob`,
`text`, `by`, `extract`, `meta`, `difft` as a stub, and the minting
of `@`) in which a blob is text, a
conflict is a record of the three sides, and `replay` resolves a path exactly
when at most one side changed it or both made the same change, path by path
even where a file meets a directory, which jj merges as one value (§7.3).
Property-test the laws of §8 over randomly generated `Repo` values (bounded
depth and width, random labels, random file edits) using the reference
`config.j` unmodified.

The builtin `by` must agree, on every generated repo, with this in-language
reference definition, which uses positional moves the public base does not
expose:

```haskell
down = \i repo ->
  let k = nth i repo.children
  in { root = k.root, children = k.children
     , context = { parent = repo.root, left = take i repo.children, right = drop (i + 1) repo.children }
                 :: repo.context }
locs = \l -> l :: (concat (map (\i -> locs (down i l)) (range 0 (length l.children))) or [])
by   = \i repo -> head (filter (\l -> l.root.id == i) (locs (top repo))) or crash "by: no such commit"
```

Golden tests: parse the reference `config.j`; `show` round-trips through the
parser for generated values of every kind except closures; every example in
§11 produces the stated result.

---

## 11. The development flow, end to end

Each step of ordinary development, and what covers it. Anything not listed
here is out of scope (§12). Nothing here uses the `jj` binary.

**Getting a repository**

```
$EDITOR ~/.config/j/config.j              -- set `user` once
j init                                    -- new repository here, or adopt an existing .git
j clone git@host:org/repo.git             -- or an existing one
j remote git@host:org/repo.git            -- attach a remote to an init'd repo
```

**Looking**

```
j tree                                    -- the history as a tree, focus marked
j treeFull                                -- the same with sizes, ages, authors, changed paths
j status                                  -- id, message, labels, changed paths, conflicts of @
j log                                     -- every commit as a record
j review                                  -- difftastic rendering of @ against its parent
j review . by @kpq                        -- the same for another commit, without moving
j diffs                                   -- the same, one record per path
j here                                    -- @'s id
j kids                                    -- @'s children, as a commit table
j conflicts                               -- every conflicted commit
j 'map (.path) . files'                   -- paths tracked in @
j 'text . contentAt ./README.md . files'
j focus                                   -- the focused commit
j 'meta . (.id) . focus'                  -- its git hash, author, time
```

**Working**

```
j 'describe "fix login"'
j new                                     -- start the next commit
j prev                                    -- to the parent
j next                                    -- to the only child
j new . goto trunk                        -- start on the main line; the focus must be a mutable commit
j by @kpq                                 -- any unique id prefix
j into @kpq                               -- a child, by id
j 'child (\c -> startsWith "fix" c.message)'
j 'split (ext "rs")'                      -- *.rs changes into a new child
j 'contract (under ./docs)'               -- docs/ changes down into the parent
j squash
j abandon
j rebase trunk
j guard clean . rebase trunk or id        -- only if it comes out clean
j pick @kpq
j 'forEach (descendantsOf here) (describe "wip")'
j id                                      -- record the working directory, change nothing else
j tree . squash                           -- dry run: show the result, persist nothing
j tree . validate . squash                -- the same, refusing what persistence would refuse
j squash && j tree                        -- do, then look
```

**Sharing**

```
j 'push (label "feature" here)'           -- name @ on the remote
j push relabel                            -- move the remote's existing names to the rewritten stack
j 'push (unlabel "feature")'             -- delete a name on the remote
j 'push (rename "old" "new")'           -- move a name
j fetch                                   -- refresh the remote's names and commits
j rebase trunk                            -- bring the stack up to date
```

**Conflicts**

A rebase or fetch that conflicts leaves unresolved files in the affected
commits. `j conflicts` lists them; `j by @…` moves there, which checks the
files out with conflict markers; editing the files and running any persisting
expression (`j id` at minimum) records the resolution.

**Mistakes**

```
j ops                                     -- what happened, newest first
j undo                                    -- reverts one j expression
j redo
```

**Own commands**

Any definition added to `config.j` whose value is an `Edit` is a command:

```haskell
pr = \n -> describe n . squash
```

```
j 'pr "fix login"' && j 'push (label "fix-login" here)'
```

---

## 12. Out of scope for this version

Second parents of merge commits (merges are placed under their first parent
and are read-only); hunk-level `split`; conflict resolution inside the
language (blobs are opaque); workspaces other than the default; remotes other
than `origin`; tags; submodules and LFS; ignore rules beyond `.gitignore`;
changing file modes or symlinks from the language; repairing a repository in
which one change id has two visible commits (refused; §7.2); a `--help` flag
or any flag at all; static typing. None of these are precluded by the design.

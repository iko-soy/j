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
touchedPaths : Change -> [Path]            -- paths whose content differs, one pass;
                                           -- blobs compared by content id, bytes never read
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
treeWith   : { detail : Int, margin : Bool, elide : Bool, icons : Bool, color : Text
             , lanes : Int, author : Bool, date : Bool, files : Bool }
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
touched = touchedPaths

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
-- readable in the terminal. A focus that changes nothing reads "no changes";
-- a difft that cannot be run (not on PATH) is a crash saying so, and
-- otherwise whatever difft prints, even nothing, is the page.
review : Repo -> Text
review = \repo ->
  let ds = diffs repo
  in if null ds then "no changes\n" else concat (map (\d -> d.diff) ds)

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
-- The default `tree` shows every commit, sorted by time (parents always above
-- their children); `treeCompact` folds uninteresting runs and far subtrees.
tree : Repo -> Text
tree = treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = "auto", lanes = 4, author = false, date = false, files = false })

treeCompact : Repo -> Text
treeCompact = treeWith ({ detail = 1, margin = false, elide = true,  icons = false, color = "auto", lanes = 4, author = false, date = false, files = false })

treeFull : Repo -> Text
treeFull = treeWith ({ detail = 2, margin = true,  elide = false, icons = false, color = "auto", lanes = 4, author = false, date = false, files = false })

-- The full tree plus the extra data columns: date, files changed, author.
treeData : Repo -> Text
treeData = treeWith ({ detail = 2, margin = true,  elide = false, icons = false, color = "auto", lanes = 4, author = true, date = true, files = true })

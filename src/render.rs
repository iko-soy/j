//! Display (§5.1) and tree rendering (§7.11).

use crate::domain::ROOT_ID;
use crate::eval::Interp;
use crate::value::{Crash, ShapeKind, Value};
use std::collections::{BTreeMap, BTreeSet};
use unicode_width::UnicodeWidthChar;

// ----------------------------------------------------------------------
// colour
// ----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub struct Palette {
    pub on: bool,
}

impl Palette {
    fn wrap(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{}m{}\x1b[0m", code, s)
        } else {
            s.to_string()
        }
    }
    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn bold_bg(&self, s: &str) -> String {
        self.wrap("1;48;5;236", s)
    }
    pub fn dim_italic(&self, s: &str) -> String {
        self.wrap("2;3", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.wrap("36", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.wrap("31", s)
    }
    pub fn blue(&self, s: &str) -> String {
        self.wrap("34", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn grey(&self, bright: u8, s: &str) -> String {
        // grey scale: brighter when more recent
        self.wrap(&format!("38;5;{}", 245 + bright.min(10)), s)
    }
    pub fn accent(&self, level: u8, s: &str) -> String {
        // size bar: one accent colour, brighter with size
        let codes = ["38;5;240", "38;5;36", "38;5;32", "38;5;39", "38;5;87"];
        self.wrap(codes[level.min(4) as usize], s)
    }
    pub fn author(&self, name: &str, s: &str) -> String {
        // stable hue per author
        let mut h: u32 = 5381;
        for b in name.bytes() {
            h = h.wrapping_mul(33).wrapping_add(b as u32);
        }
        let hue = (h % 6) as u8;
        let code = ["31", "32", "33", "34", "35", "36"][hue as usize];
        self.wrap(code, s)
    }
    /// colour with a raw ANSI code (used for meaning-coloured glyphs)
    pub fn code(&self, code: &str, s: &str) -> String {
        self.wrap(code, s)
    }
    /// Apply a background band across a whole line. The line already contains
    /// colour codes whose `\x1b[0m` resets would clear the background mid-line,
    /// so the background is re-applied after every reset. No-op when colour is
    /// off, so the focus stays marked only by the `▶` gutter (§Colour).
    pub fn bg_line(&self, bg: &str, s: &str) -> String {
        if !self.on {
            return s.to_string();
        }
        let onset = format!("\x1b[{}m", bg);
        let reapply = format!("\x1b[0m{}", onset);
        let body = s.replace("\x1b[0m", &reapply);
        format!("{}{}\x1b[0m", onset, body)
    }
}

pub fn color_enabled(mode: &str) -> bool {
    if std::env::var_os("NO_COLOR")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    match mode {
        "always" => true,
        "never" => false,
        _ => crate::show::stdout_is_tty(),
    }
}

// display width with East Asian wide = 2, combining = 0
pub fn width(s: &str) -> usize {
    let mut w = 0;
    let mut esc = false;
    for c in s.chars() {
        if esc {
            if c == 'm' {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            esc = true;
            continue;
        }
        w += UnicodeWidthChar::width(c).unwrap_or(0);
    }
    w
}

fn pad_right(s: &str, w: usize) -> String {
    let d = width(s);
    if d >= w {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(w - d))
    }
}

fn human_size(n: usize) -> String {
    if n < 1024 {
        format!("{} B", n)
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    }
}

// ----------------------------------------------------------------------
// shape recognition (§4.12/§5.1)
// ----------------------------------------------------------------------

fn field_set(v: &Value) -> Option<BTreeSet<String>> {
    match v {
        Value::Record(m) => Some(m.keys().cloned().collect()),
        _ => None,
    }
}

fn shape_of_record(interp: &Interp, v: &Value) -> Option<String> {
    let fs = field_set(v)?;
    for (name, _) in &interp.shapes.decls {
        if let Some(s) = interp.shapes.shape_of(name) {
            if let ShapeKind::Record(want) = &s.kind {
                if want == &fs {
                    return Some(name.clone());
                }
            }
        }
    }
    None
}

pub fn is_commit(interp: &Interp, v: &Value) -> bool {
    shape_of_record(interp, v).as_deref() == Some("Commit")
}

// ----------------------------------------------------------------------
// display
// ----------------------------------------------------------------------

pub fn display(interp: &mut Interp, v: &Value, color: bool) -> Result<String, Crash> {
    let pal = Palette { on: color };
    let mut out = String::new();
    let mut loaded = None;
    display_block(interp, v, &pal, 0, &mut out, &mut loaded)?;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn display_id(interp: &Interp, id: &str, pal: &Palette) -> String {
    let p = interp.unique_prefix(id);
    let n = p.len();
    format!("{}{}", pal.bold(p.as_str()), pal.dim(&id[n..]))
}

fn display_line(
    interp: &mut Interp,
    v: &Value,
    pal: &Palette,
    loaded: &mut Option<Loaded>,
) -> Result<String, Crash> {
    let v = &v.forced()?;
    match v {
        Value::Thunk(_) => unreachable!("forced never returns a thunk"),
        Value::Int(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Text(t) => Ok(first_line(t)),
        Value::Id(id) => Ok(display_id(interp, id, pal)),
        Value::Blob(b) => {
            let size = human_size(b.size());
            if b.is_unresolved() {
                Ok(format!("‹✖ {}›", size))
            } else {
                Ok(format!("‹{}›", size))
            }
        }
        // a lambda renders as its source (§5.2), which may run over several
        // lines and hold comments; the line form keeps only the first
        Value::Fun(_) => Ok(first_line(&crate::show::show(&Interp::dummy(), v)?)),
        Value::Shape(s) => Ok(s.name.clone()),
        Value::Record(_) => {
            if let Some(shape) = shape_of_record(interp, v) {
                match shape.as_str() {
                    "Commit" => return line_in(interp, v, pal, loaded, false),
                    "Entry" => return entry_line(interp, v, pal),
                    "Subtree" => return line_in(interp, &v.field("root")?, pal, loaded, false),
                    // its focus's line, drawn as the focus
                    "Repo" => return line_in(interp, &v.field("root")?, pal, loaded, true),
                    "Frame" => return line_in(interp, &v.field("parent")?, pal, loaded, false),
                    "Change" => {
                        let n = touched_paths(v)?.len();
                        return Ok(format!("{} paths", n));
                    }
                    _ => {}
                }
            }
            let names: Vec<String> = field_set(v).unwrap().into_iter().collect();
            Ok(format!("{{ {} }}", names.join(", ")))
        }
        Value::List(xs) => list_line(interp, xs, pal),
    }
}

/// The line form of text that may run over several lines (§5.1): its first
/// line, `…` if there are more.
fn first_line(s: &str) -> String {
    let mut lines = s.lines();
    let first = lines.next().unwrap_or("");
    if lines.next().is_some() {
        format!("{}…", first)
    } else {
        first.to_string()
    }
}

fn list_line(_interp: &Interp, xs: &[Value], pal: &Palette) -> Result<String, Crash> {
    if xs.is_empty() {
        return Ok("0 items".into());
    }
    let all_id = xs.iter().all(|x| matches!(x, Value::Id(_)));
    if all_id {
        return Ok(format!("{} commits", xs.len()));
    }
    let all_path = xs.iter().all(|x| match x {
        Value::List(p) => p.iter().all(|c| matches!(c, Value::Text(_))),
        _ => false,
    });
    if all_path {
        return Ok(format!("{} paths", xs.len()));
    }
    let all_text = xs.iter().all(|x| matches!(x, Value::Text(_)));
    if all_text {
        return Ok(format!("{} items", xs.len()));
    }
    let all_record = xs.iter().all(|x| matches!(x, Value::Record(_)));
    if all_record {
        let first = field_set(&xs[0]).unwrap();
        let same = xs
            .iter()
            .all(|x| field_set(x).map(|s| s == first).unwrap_or(false));
        if same {
            return Ok(format!("{} rows", xs.len()));
        }
    }
    let all_scalar = xs.iter().all(|x| {
        matches!(x, Value::Int(_) | Value::Bool(_))
    });
    if all_scalar {
        let parts: Vec<String> = xs
            .iter()
            .map(|x| match x {
                Value::Int(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => unreachable!(),
            })
            .collect();
        return Ok(parts.join(", "));
    }
    let _ = pal;
    Ok(format!("{} items", xs.len()))
}

fn entry_line(interp: &Interp, v: &Value, pal: &Palette) -> Result<String, Crash> {
    let path = path_string(&v.field("path")?)?;
    let content = v.field("content")?;
    let (size, unresolved) = match &content {
        Value::Blob(b) => (human_size(b.size()), b.is_unresolved()),
        _ => (String::new(), false),
    };
    let _ = interp;
    if unresolved {
        Ok(format!("{} {}  {}", pal.red("✖"), path, size))
    } else {
        Ok(format!("  {}  {}", path, size))
    }
}

fn path_string(p: &Value) -> Result<String, Crash> {
    let comps = p.as_list()?;
    let parts: Vec<String> = comps
        .iter()
        .map(|c| c.as_text().map(|s| s.to_string()))
        .collect::<Result<_, _>>()?;
    Ok(parts.join("/"))
}

/// entries of one snapshot, keyed by path components
type SnapMap = BTreeMap<Vec<String>, Value>;

fn touched_paths(change: &Value) -> Result<Vec<(String, char, bool)>, Crash> {
    let from = snapshot_map_of(&change.field("from")?)?;
    let to = snapshot_map_of(&change.field("to")?)?;
    Ok(touched_in_maps(&from, &to)?
        .into_iter()
        .map(|(p, mark, unresolved)| (p.join("/"), mark, unresolved))
        .collect())
}

/// The changed paths between two snapshots that have already been keyed, as
/// (path components, mark, unresolved). Both maps are sorted, so the union is
/// a merge — the previous version collected every key of both sides into a
/// `BTreeSet`, cloning each one.
fn touched_in_maps(from: &SnapMap, to: &SnapMap) -> Result<Vec<(Vec<String>, char, bool)>, Crash> {
    let mut out = Vec::new();
    let mut fi = from.iter().peekable();
    let mut ti = to.iter().peekable();
    loop {
        let ord = match (fi.peek(), ti.peek()) {
            (None, None) => break,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some((fp, _)), Some((tp, _))) => fp.cmp(tp),
        };
        let (path, mark, t) = match ord {
            std::cmp::Ordering::Less => {
                let (p, _) = fi.next().unwrap();
                (p, '−', None)
            }
            std::cmp::Ordering::Greater => {
                let (p, v) = ti.next().unwrap();
                (p, '+', Some(v))
            }
            std::cmp::Ordering::Equal => {
                let (p, a) = fi.next().unwrap();
                let (_, b) = ti.next().unwrap();
                if crate::value::value_eq(a, b)? {
                    continue;
                }
                (p, '~', Some(b))
            }
        };
        let unresolved = t
            .map(|b| matches!(b, Value::Blob(bl) if bl.is_unresolved()))
            .unwrap_or(false);
        out.push((
            path.clone(),
            if unresolved { '✖' } else { mark },
            unresolved,
        ));
    }
    Ok(out)
}

/// The line count from which the size bar draws its last glyph, `▇` (§7.11
/// column 5): no count past it changes the bar, so none is computed.
const SIZE_BAR_TOP: usize = 1000;

/// Lines added plus removed between two versions of one path (§7.11 size
/// bar), either possibly absent, counted up to `cap`: the length of a
/// shortest line diff, or `cap` if that is at least `cap`. Lines are split
/// after each `\n` as `line_count` counts them. A conflict counts as its
/// materialized text, as it is checked out.
///
/// A line diff costs the product of the file's length and the size of the
/// change, so a plain one of a rewritten or reordered long file takes
/// seconds. The search here stops at the cap rather than at a clock, so the
/// same two versions always give the same count, but that only bounds its
/// cost by the product of the file's length and the cap: at most one pass
/// over the lines for each of the `2 × cap + 1` diagonals it reaches
/// (`edit_distance`). Every step before it keeps the count exact: the common
/// prefix and suffix are equal lines of some shortest diff, and the
/// difference in length is a lower bound. A small change is then settled by
/// a short search over the lines as they are. Otherwise a line that does not
/// occur on the other side at all cannot be part of any common subsequence,
/// so, as git's xdiff does, it counts as changed up front, and the rest is
/// searched unless `reorder_bound` already puts it past the cap. On most
/// content a diagonal soon stops at a line that differs; a long file of a
/// few lines repeating pays a pass for many of them when the bounds cannot
/// settle the change: one just under the cap, or one that leaves how many
/// times each line occurs as it was. A million lines of two alternating,
/// with 480 pairs of neighbours swapped, take some 5 × 10⁸ comparisons.
fn changed_lines(from: Option<&Value>, to: Option<&Value>, cap: usize) -> Result<usize, Crash> {
    use std::collections::HashMap;
    // the rounds of the first, short search
    const QUICK: usize = 64;
    let text = |v: Option<&Value>| -> Result<Vec<u8>, Crash> {
        match v {
            Some(Value::Blob(b)) => b.bytes(),
            _ => Ok(Vec::new()),
        }
    };
    let (a, b) = (text(from)?, text(to)?);
    let a: Vec<&[u8]> = a.split_inclusive(|c| *c == b'\n').collect();
    let b: Vec<&[u8]> = b.split_inclusive(|c| *c == b'\n').collect();
    let (a, b) = trim_common(&a, &b);
    if a.len().abs_diff(b.len()) >= cap {
        return Ok(cap);
    }
    let quick = cap.min(QUICK);
    let d = edit_distance(a, b, quick);
    if d < quick || quick == cap {
        return Ok(d);
    }
    // number each distinct line, so the rest compares numbers; the lines
    // numbered while reading `a` are those that occur in it
    fn number<'l>(ids: &mut HashMap<&'l [u8], usize>, lines: &[&'l [u8]]) -> Vec<usize> {
        lines
            .iter()
            .map(|l| {
                let next = ids.len();
                *ids.entry(*l).or_insert(next)
            })
            .collect()
    }
    let mut ids = HashMap::new();
    let a = number(&mut ids, a);
    let in_a = ids.len();
    let b = number(&mut ids, b);
    let mut in_b = vec![false; ids.len()];
    for l in &b {
        in_b[*l] = true;
    }
    let a_common: Vec<usize> = a.iter().copied().filter(|l| in_b[*l]).collect();
    let b_common: Vec<usize> = b.iter().copied().filter(|l| *l < in_a).collect();
    let unmatched = (a.len() - a_common.len()) + (b.len() - b_common.len());
    if unmatched >= cap {
        return Ok(cap);
    }
    let (a, b) = trim_common(&a_common, &b_common);
    let max = cap - unmatched;
    if reorder_bound(a, b, ids.len()) >= max {
        return Ok(cap);
    }
    Ok(unmatched + edit_distance(a, b, max))
}

/// `a` and `b` without the lines they start and end with in common.
fn trim_common<'s, T: PartialEq>(a: &'s [T], b: &'s [T]) -> (&'s [T], &'s [T]) {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[pre..], &b[pre..]);
    let suf = a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count();
    (&a[..a.len() - suf], &b[..b.len() - suf])
}

/// The number of lines deleted plus inserted by a shortest diff of `a` to
/// `b`, or `max` if that is at least `max`: Myers' greedy search, which
/// reaches the end on round `d` exactly when the shortest diff has `d` steps,
/// run for `max` rounds at most. A round extends each of its diagonals along
/// equal lines from where the round before left off, so the work is at most
/// `max` squared steps plus one pass over the lines per diagonal.
fn edit_distance<T: PartialEq>(a: &[T], b: &[T], max: usize) -> usize {
    let (n, m) = (a.len() as isize, b.len() as isize);
    // the furthest `x` reached on diagonal `k = x - y`, at `v[k + max]`;
    // round `d` writes diagonals `-d..=d` from those round `d - 1` wrote, and
    // round 0 starts from the zero on diagonal 1
    let mut v = vec![0isize; 2 * max + 1];
    let at = |k: isize| (k + max as isize) as usize;
    for d in 0..max as isize {
        for k in (-d..=d).step_by(2) {
            // from diagonal `k + 1` by inserting a line of `b`, or from
            // `k - 1` by deleting one of `a`, whichever gets further
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                return d as usize;
            }
        }
    }
    max
}

/// A lower bound on the lines deleted plus inserted by any diff of `a` to
/// `b` (lines numbered below `ids`), cheap where the search is dearest: a
/// long file whose lines were reordered, or one of a few lines repeating
/// that now occur a different number of times. A line that occurs once on
/// each side can only be kept by pairing those two, and the pairs kept keep
/// their order on both sides, so at most the longest sequence of them in the
/// same order on both sides is kept (patience sorting finds its length); any
/// other line is kept at most as many times as it occurs on the side where
/// it occurs fewer times.
fn reorder_bound(a: &[usize], b: &[usize], ids: usize) -> usize {
    // per line: its occurrences in `a` and in `b`, and where it is in `a`
    let mut seen = vec![(0usize, 0usize, 0usize); ids];
    for (i, l) in a.iter().enumerate() {
        seen[*l].0 += 1;
        seen[*l].2 = i;
    }
    for l in b {
        seen[*l].1 += 1;
    }
    // over the lines once on each side, in `b`'s order, by where they are in
    // `a`: `ends[i]` is the least last place of an increasing sequence of
    // `i + 1` of them
    let mut ends: Vec<usize> = Vec::new();
    for l in b {
        if let (1, 1, i) = seen[*l] {
            let at = ends.partition_point(|e| *e < i);
            if at == ends.len() {
                ends.push(i);
            } else {
                ends[at] = i;
            }
        }
    }
    let others: usize = seen
        .iter()
        .filter(|(in_a, in_b, _)| (*in_a, *in_b) != (1, 1))
        .map(|(in_a, in_b, _)| in_a.min(in_b))
        .sum();
    let kept = ends.len() + others;
    a.len() + b.len() - 2 * kept
}

fn snapshot_map_of(snap: &Value) -> Result<SnapMap, Crash> {
    let mut m = BTreeMap::new();
    for e in snap.as_list()?.iter() {
        let path = e
            .field("path")?
            .as_list()?
            .iter()
            .map(|c| c.as_text().map(|s| s.to_string()))
            .collect::<Result<Vec<String>, Crash>>()?;
        m.insert(path, e.field("content")?);
    }
    Ok(m)
}

/// What a commit's glyph says about where it stands in a history
/// (specs/tree.md §Glyphs), beyond what its own files say. A `Commit` value
/// records none of it, so both its forms read it from the repository the
/// expression was given (`Loaded`).
struct Standing {
    focus: bool,
    /// an ancestor of the focus
    ancestor: bool,
    immutable: bool,
    /// no change against its parent
    empty: bool,
}

fn commit_line(
    interp: &Interp,
    c: &Value,
    pal: &Palette,
    standing: &Standing,
) -> Result<String, Crash> {
    let id = match c.field("id")? {
        Value::Id(i) => i.to_string(),
        _ => return Err(Crash::new("commit without id")),
    };
    let msg = c.field("message")?.as_text()?.to_string();
    let msg_first = msg.lines().next().unwrap_or("").to_string();
    let labels: Vec<String> = c
        .field("labels")?
        .as_list()?
        .iter()
        .map(|l| l.as_text().map(|s| s.to_string()))
        .collect::<Result<_, _>>()?;
    let glyph = node_glyph(c, &id, standing)?;
    let id_s = display_id(interp, &id, pal);
    let labels_s = if labels.is_empty() {
        String::new()
    } else {
        pal.green(&labels.join("  "))
    };
    Ok(if labels_s.is_empty() {
        format!("{} {}  {}", glyph, id_s, msg_first)
    } else {
        format!("{} {}  {}  {}", glyph, id_s, msg_first, labels_s)
    })
}

/// The glyph of a commit line, by `tree`'s precedence (`glyph_for`).
fn node_glyph(c: &Value, id: &str, standing: &Standing) -> Result<String, Crash> {
    let g = if id == ROOT_ID {
        "⌂"
    } else if has_conflict(c)? {
        "⊗"
    } else if standing.empty {
        "◌"
    } else if standing.immutable {
        "◆"
    } else if standing.focus {
        "◉"
    } else if standing.ancestor {
        "●"
    } else {
        "○"
    };
    Ok(g.to_string())
}

fn has_conflict(c: &Value) -> Result<bool, Crash> {
    let files = c.field("files")?;
    for e in files.as_list()?.iter() {
        if let Value::Blob(b) = e.field("content")? {
            if b.is_unresolved() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn display_block(
    interp: &mut Interp,
    v: &Value,
    pal: &Palette,
    indent: usize,
    out: &mut String,
    loaded: &mut Option<Loaded>,
) -> Result<(), Crash> {
    let pad = " ".repeat(indent);
    let v = &v.forced()?;
    match v {
        Value::Thunk(_) => unreachable!("forced never returns a thunk"),
        Value::Text(t) => {
            out.push_str(t);
            return Ok(());
        }
        Value::Blob(b) => {
            let content = String::from_utf8_lossy(&b.bytes()?).to_string();
            out.push_str(&indent_multiline(&content, indent));
            return Ok(());
        }
        Value::Int(_) | Value::Bool(_) | Value::Id(_) | Value::Shape(_) => {
            out.push_str(&pad);
            out.push_str(&display_line(interp, v, pal, loaded)?);
            out.push('\n');
            return Ok(());
        }
        // all of a lambda's source, of which the line form keeps the first
        // line (§5.1), each line under the block's indent
        Value::Fun(_) => {
            for l in crate::show::show(&Interp::dummy(), v)?.lines() {
                out.push_str(&pad);
                out.push_str(l);
                out.push('\n');
            }
            return Ok(());
        }
        Value::List(xs) => return display_list_block(interp, xs, pal, indent, out, loaded),
        Value::Record(_) => {}
    }
    if let Some(shape) = shape_of_record(interp, v) {
        match shape.as_str() {
            "Commit" => {
                let loaded = Loaded::cached(interp, loaded)?;
                return display_commit_block(interp, v, pal, indent, out, loaded);
            }
            "Entry" => {
                out.push_str(&pad);
                out.push_str(&entry_line(interp, v, pal)?);
                out.push('\n');
                return Ok(());
            }
            "Subtree" => {
                let opts = default_tree_options(interp)?;
                let repo = Value::record(&[
                    ("root", v.field("root")?),
                    ("children", v.field("children")?),
                    ("context", Value::list(vec![])),
                ]);
                let text = tree_render(interp, &opts, &repo, pal, false, tty_width())?;
                out.push_str(&indent_multiline(&text, indent));
                return Ok(());
            }
            "Repo" => {
                let tree_fn = interp
                    .globals
                    .lookup("tree")
                    .ok_or_else(|| Crash::new("`tree` is not defined"))?;
                let text = interp.apply(tree_fn, v.clone())?;
                let text = text.as_text()?;
                out.push_str(&indent_multiline(text, indent));
                return Ok(());
            }
            "Frame" => {
                out.push_str(&pad);
                out.push_str(&line_in(interp, &v.field("parent")?, pal, loaded, false)?);
                out.push('\n');
                return Ok(());
            }
            "Change" => {
                for (p, mark, _) in touched_paths(v)? {
                    out.push_str(&pad);
                    out.push_str(&format!("{} {}\n", colored_mark(mark, pal), p));
                }
                return Ok(());
            }
            _ => {}
        }
    }
    // generic record: one field per line
    let m = match v {
        Value::Record(m) => m,
        _ => unreachable!(),
    };
    let name_w = m.keys().map(|k| width(k)).max().unwrap_or(0);
    for (k, x) in m.iter() {
        out.push_str(&pad);
        out.push_str(&pal.dim(&pad_right(k, name_w)));
        out.push_str("  ");
        // lists print in block form under their key (§5.1)
        if matches!(x, Value::List(xs) if !xs.is_empty()) {
            out.push('\n');
            display_block(interp, x, pal, indent + name_w + 2, out, loaded)?;
        } else {
            let line = display_line(interp, x, pal, loaded)?;
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(())
}

fn indent_multiline(s: &str, indent: usize) -> String {
    if indent == 0 {
        return s.to_string();
    }
    let pad = " ".repeat(indent);
    s.lines()
        .map(|l| format!("{}{}", pad, l))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// A change mark (`+ ~ − ✖`), coloured by what it means.
fn colored_mark(mark: char, pal: &Palette) -> String {
    let ms = mark.to_string();
    match mark {
        '+' => pal.green(&ms),
        '−' | '✖' => pal.red(&ms),
        '~' => pal.yellow(&ms),
        _ => ms,
    }
}

/// The repository the expression was given (§1.2 step 4, after the
/// snapshot), or, where only the loaded one is set (the interpreter tests),
/// that one.
fn given_repo(interp: &Interp) -> Option<Value> {
    let given = interp.given_repo.borrow().clone();
    given.or_else(|| interp.old_repo.borrow().clone())
}

/// The repository the expression was given (`given_repo`), indexed for
/// displaying `Commit` values. A commit value records neither its parent nor
/// where it stands, so both its forms read them from the commit with the
/// same id there, as its block reads author and age (§5.1). Not the
/// repository as loaded: the snapshot has rebased every descendant of the
/// focus since, and the values displayed come from after it. Indexed once
/// per displayed value, not once per commit: a table of commits would
/// otherwise walk the history once for each row.
struct Loaded {
    /// the parent of every commit that has one
    parents: BTreeMap<String, Value>,
    focus: String,
    /// the focus's ancestors, the focus excluded
    ancestors: BTreeSet<String>,
    immutable: BTreeSet<String>,
}

impl Loaded {
    /// `cache`, indexed when a display first draws a commit
    fn cached<'a>(interp: &mut Interp, cache: &'a mut Option<Loaded>) -> Result<&'a Loaded, Crash> {
        if cache.is_none() {
            *cache = Some(Loaded::of(interp)?);
        }
        Ok(cache.as_ref().expect("just set"))
    }

    fn of(interp: &mut Interp) -> Result<Loaded, Crash> {
        let mut loaded = Loaded {
            parents: BTreeMap::new(),
            focus: String::new(),
            ancestors: BTreeSet::new(),
            immutable: BTreeSet::new(),
        };
        let Some(repo) = given_repo(interp) else {
            return Ok(loaded);
        };
        if let Value::Id(i) = repo.field("root")?.field("id")? {
            loaded.focus = i.to_string();
        }
        for frame in repo.field("context")?.as_list()?.iter() {
            loaded.ancestors.insert(id_of_frame_parent(frame)?);
        }
        // as `tree_render` does: a config whose `immutable` crashes draws no `◆`
        loaded.immutable = crate::repo::compute_immutable(interp, &repo).unwrap_or_default();
        // every commit's parent, walking down from the top of the history
        let top = crate::repo::by_id(&repo, &top_id(&repo)?)?.unwrap_or_else(|| repo.clone());
        let mut todo = vec![(top.field("root")?, top.field("children")?)];
        while let Some((parent, children)) = todo.pop() {
            for c in children.as_list()?.iter() {
                let root = c.field("root")?;
                if let Value::Id(i) = root.field("id")? {
                    loaded.parents.insert(i.to_string(), parent.clone());
                }
                todo.push((root, c.field("children")?));
            }
        }
        Ok(loaded)
    }

    /// Where the commit with id `id` stands here, given whether it changes
    /// nothing against its parent here.
    fn standing(&self, id: &str, empty: bool) -> Standing {
        Standing {
            focus: id == self.focus,
            ancestor: self.ancestors.contains(id),
            immutable: self.immutable.contains(id),
            empty,
        }
    }

    /// Where the commit value `c` stands here, for its line form: `◌` judged
    /// as the block judges it, by its files against its parent's here. While
    /// both still hold their stored trees the backend answers from tree ids,
    /// as for `tree` (`build_info`), so a table of commits reads no parent's
    /// files. A commit with no parent here claims no `◌`.
    fn line_standing(&self, interp: &Interp, c: &Value) -> Result<Standing, Crash> {
        let id = match c.field("id")? {
            Value::Id(i) => i.to_string(),
            _ => String::new(),
        };
        let empty = match self.parents.get(&id) {
            None => false,
            Some(parent) => {
                let parent_id = match parent.field("id")? {
                    Value::Id(i) => i.to_string(),
                    _ => String::new(),
                };
                let me = stored_origin(c).filter(|o| *o == id);
                let stored_parent = stored_origin(parent).filter(|o| *o == parent_id);
                let answer = match (me, stored_parent) {
                    (Some(me), Some(parent)) => interp.backend.is_empty(me, parent),
                    _ => None,
                };
                match answer {
                    Some(e) => e,
                    // in any order: a snapshot stands for a tree (§7.3)
                    None => crate::repo::snapshot_eq(&c.field("files")?, &parent.field("files")?)?,
                }
            }
        };
        Ok(self.standing(&id, empty))
    }
}

/// A commit's line form, drawn where it stands in the repository the
/// expression was given (§5.1); `as_focus` draws it as a `Repo`'s focus.
fn line_in(
    interp: &mut Interp,
    c: &Value,
    pal: &Palette,
    loaded: &mut Option<Loaded>,
    as_focus: bool,
) -> Result<String, Crash> {
    let loaded = Loaded::cached(interp, loaded)?;
    let mut standing = loaded.line_standing(interp, c)?;
    standing.focus |= as_focus;
    commit_line(interp, c, pal, &standing)
}

fn display_commit_block(
    interp: &mut Interp,
    c: &Value,
    pal: &Palette,
    indent: usize,
    out: &mut String,
    loaded: &Loaded,
) -> Result<(), Crash> {
    let pad = " ".repeat(indent);
    let id = match c.field("id")? {
        Value::Id(i) => i.to_string(),
        _ => String::new(),
    };
    let own = snapshot_map_of(&c.field("files")?)?;
    // the paths it changes against its parent in the given repository; with
    // no parent there — the root, or a commit the expression made, whose
    // parent the value does not record — what it changes is not known
    let touched = match loaded.parents.get(&id) {
        Some(parent) => Some(touched_in_maps(
            &snapshot_map_of(&parent.field("files")?)?,
            &own,
        )?),
        None => None,
    };
    // the line `tree` draws for it (§5.1: "a `tree` line without rails")
    let empty = touched.as_ref().is_some_and(|t| t.is_empty());
    out.push_str(&pad);
    out.push_str(&commit_line(interp, c, pal, &loaded.standing(&id, empty))?);
    out.push('\n');
    // author · age · n files — from metadata by id (§5.1)
    let n = touched.as_ref().map_or(own.len(), |t| t.len());
    if let Ok(meta) = interp.backend.meta(&id) {
        let age = render_age(meta.time);
        out.push_str(&pad);
        out.push_str(&format!("  {} · {} · {} files\n", meta.author, age, n));
    }
    if n > 0 {
        out.push('\n');
    }
    match &touched {
        Some(touched) => {
            for (path, mark, _) in touched {
                out.push_str(&pad);
                out.push_str(&format!(
                    "  {} {}\n",
                    colored_mark(*mark, pal),
                    path.join("/")
                ));
            }
        }
        // its files, unmarked but for `✖`
        None => {
            for (path, content) in &own {
                out.push_str(&pad);
                if matches!(content, Value::Blob(b) if b.is_unresolved()) {
                    out.push_str(&format!("  {} {}\n", pal.red("✖"), path.join("/")));
                } else {
                    out.push_str(&format!("    {}\n", path.join("/")));
                }
            }
        }
    }
    Ok(())
}

pub fn render_age(time: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let d = (now - time).max(0);
    if d < 60 {
        format!("{}s", d)
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86400 {
        format!("{}h", d / 3600)
    } else if d < 86400 * 7 {
        format!("{}d", d / 86400)
    } else if d < 86400 * 365 {
        format!("{}w", d / (86400 * 7))
    } else {
        format!("{}y", d / (86400 * 365))
    }
}

/// Format unix-seconds `time` as an absolute date `YYYY-MM-DD` (UTC), using
/// the civil-from-days algorithm (no external date crate).
pub fn render_date(time: i64) -> String {
    let days = time.div_euclid(86_400);
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// the repo value in context (the one the expression was given), for
/// resolving ids to commits
fn top_of(interp: &Interp, _x: &Value) -> Result<Value, Crash> {
    given_repo(interp).ok_or_else(|| Crash::new("display: no repository in context"))
}

fn display_list_block(
    interp: &mut Interp,
    xs: &[Value],
    pal: &Palette,
    indent: usize,
    out: &mut String,
    loaded: &mut Option<Loaded>,
) -> Result<(), Crash> {
    let pad = " ".repeat(indent);
    if xs.is_empty() {
        out.push_str(&pad);
        out.push_str(&pal.dim("none"));
        out.push('\n');
        return Ok(());
    }
    // list of Id: commit table (one commit line per id, in list order)
    if xs.iter().all(|x| matches!(x, Value::Id(_))) {
        // index the history once: a `by_id` walk per id made printing a
        // revset quadratic in the size of the history
        let repo = top_of(interp, &xs[0])?;
        let mut commits: BTreeMap<String, Value> = BTreeMap::new();
        for c in crate::repo::all_commits(&repo)? {
            if let Ok(Value::Id(i)) = c.field("id") {
                commits.insert(i.to_string(), c);
            }
        }
        let loaded = Loaded::cached(interp, loaded)?;
        for x in xs {
            let id = match x {
                Value::Id(i) => i.to_string(),
                _ => unreachable!(),
            };
            out.push_str(&pad);
            match commits.get(&id) {
                Some(c) => {
                    let c = c.clone();
                    // the glyph of its line form (§5.1)
                    let glyph = node_glyph(&c, &id, &loaded.line_standing(interp, &c)?)?;
                    let conflict = glyph == "⊗";
                    let glyph = if conflict { pal.red(&glyph) } else { glyph };
                    let msg = c.field("message")?.as_text()?.to_string();
                    let msg_first = msg.lines().next().unwrap_or("");
                    let labels: Vec<String> = c
                        .field("labels")?
                        .as_list()?
                        .iter()
                        .map(|l| l.as_text().map(|s| s.to_string()))
                        .collect::<Result<_, _>>()?;
                    let id_s = if conflict {
                        pal.red(&interp.unique_prefix(&id))
                    } else {
                        display_id(interp, &id, pal)
                    };
                    if labels.is_empty() {
                        out.push_str(&format!("{} {}  {}\n", glyph, id_s, msg_first));
                    } else {
                        out.push_str(&format!(
                            "{} {}  {}  {}\n",
                            glyph,
                            id_s,
                            msg_first,
                            pal.green(&labels.join("  "))
                        ));
                    }
                }
                None => {
                    out.push_str(&display_id(interp, &id, pal));
                    out.push('\n');
                }
            }
        }
        return Ok(());
    }
    // list of Text
    if xs.iter().all(|x| matches!(x, Value::Text(_))) {
        for x in xs {
            out.push_str(&pad);
            out.push_str(x.as_text()?);
            out.push('\n');
        }
        return Ok(());
    }
    // list of [Text]: paths
    let all_path = xs.iter().all(|x| match x {
        Value::List(p) => p.iter().all(|c| matches!(c, Value::Text(_))),
        _ => false,
    });
    if all_path {
        for x in xs {
            out.push_str(&pad);
            out.push_str(&path_string(x)?);
            out.push('\n');
        }
        return Ok(());
    }
    // list of records with identical field sets: Entry list gets entry lines,
    // other identical sets get a column table
    let all_record = xs.iter().all(|x| matches!(x, Value::Record(_)));
    if all_record {
        let first = field_set(&xs[0]).unwrap();
        let same = xs
            .iter()
            .all(|x| field_set(x).map(|s| s == first).unwrap_or(false));
        if same {
            if shape_of_record(interp, &xs[0]).as_deref() == Some("Entry") {
                let width_max = xs
                    .iter()
                    .map(|x| path_string(&x.field("path").unwrap()).map(|p| width(&p)))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .max()
                    .unwrap_or(0);
                for x in xs {
                    let path = path_string(&x.field("path")?)?;
                    let content = x.field("content")?;
                    let (size, unresolved) = match &content {
                        Value::Blob(b) => (human_size(b.size()), b.is_unresolved()),
                        _ => (String::new(), false),
                    };
                    out.push_str(&pad);
                    let mark = if unresolved { "✖" } else { " " };
                    let size_s = if unresolved {
                        pal.red(&size)
                    } else {
                        size
                    };
                    // pad by display width, not by character count: `{:n$}`
                    // pads to n *chars*, so a wide (East Asian) path pushes
                    // the size column out of line
                    out.push_str(&format!("{} {}  {}\n", mark, pad_right(&path, width_max), size_s));
                }
                return Ok(());
            }
            return display_table(interp, xs, pal, indent, out, loaded);
        }
    }
    // list of other scalars
    let all_scalar = xs
        .iter()
        .all(|x| matches!(x, Value::Int(_) | Value::Bool(_)));
    if all_scalar {
        out.push_str(&pad);
        out.push_str(&list_line(interp, xs, pal)?);
        out.push('\n');
        return Ok(());
    }
    // one block per item, separated by a blank line
    for (i, x) in xs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        display_block(interp, x, pal, indent, out, loaded)?;
    }
    Ok(())
}

fn display_table(
    interp: &mut Interp,
    xs: &[Value],
    pal: &Palette,
    indent: usize,
    out: &mut String,
    loaded: &mut Option<Loaded>,
) -> Result<(), Crash> {
    let pad = " ".repeat(indent);
    let fields: Vec<String> = field_set(&xs[0]).unwrap().into_iter().collect();
    // cells
    let mut rows: Vec<Vec<String>> = Vec::new();
    for x in xs {
        let mut row = Vec::new();
        for f in &fields {
            let cell = match x.field(f)? {
                Value::Bool(true) => "✓".to_string(),
                Value::Bool(false) => String::new(),
                other => display_line(interp, &other, pal, loaded)?,
            };
            row.push(cell);
        }
        rows.push(row);
    }
    // drop columns empty in every row
    let keep: Vec<usize> = (0..fields.len())
        .filter(|&i| rows.iter().any(|r| !r[i].is_empty()))
        .collect();
    let widths: Vec<usize> = keep
        .iter()
        .map(|&i| {
            rows.iter()
                .map(|r| width(&r[i]))
                .max()
                .unwrap_or(0)
                .max(width(&fields[i]))
        })
        .collect();
    // header
    out.push_str(&pad);
    let header: Vec<String> = keep
        .iter()
        .zip(&widths)
        .map(|(&i, w)| pad_right(&fields[i], *w))
        .collect();
    out.push_str(&pal.dim(&header.join("   ")));
    out.push('\n');
    for r in &rows {
        out.push_str(&pad);
        let cells: Vec<String> = keep
            .iter()
            .zip(&widths)
            .map(|(&i, w)| pad_right(&r[i], *w))
            .collect();
        out.push_str(&cells.join("   "));
        out.push('\n');
    }
    Ok(())
}

// ----------------------------------------------------------------------
// treeWith (§7.11)
// ----------------------------------------------------------------------

pub struct TreeOptions {
    pub detail: i64,
    pub margin: bool,
    pub elide: bool,
    pub icons: bool,
    pub color: String,
    pub lanes: i64,
    /// show the full author name column
    pub author: bool,
    /// show the absolute commit date column (YYYY-MM-DD)
    pub date: bool,
    /// show the number of files changed column
    pub files: bool,
}

fn default_tree_options(_interp: &Interp) -> Result<TreeOptions, Crash> {
    Ok(TreeOptions {
        detail: 1,
        margin: false,
        elide: true,
        icons: false,
        color: "auto".into(),
        lanes: 4,
        author: false,
        date: false,
        files: false,
    })
}

/// The terminal's width when stdout is one — what rows are cut to and the
/// legend is placed by (specs/tree.md Step 4, §Legend) — else `None`.
fn tty_width() -> Option<usize> {
    if crate::show::stdout_is_tty() {
        crate::show::terminal_width()
    } else {
        None
    }
}

pub fn tree_with(interp: &mut Interp, opts: &Value, repo: &Value) -> Result<Value, Crash> {
    tree_with_width(interp, opts, repo, tty_width())
}

/// `treeWith` as if stdout were a terminal `term_w` columns wide (`None`: not
/// a terminal). Public so the tests can drive truncation without a pty.
pub fn tree_with_width(
    interp: &mut Interp,
    opts: &Value,
    repo: &Value,
    term_w: Option<usize>,
) -> Result<Value, Crash> {
    let defaults = default_tree_options(interp)?;
    // a missing option falls back to its default, so an older config that
    // predates a newer option keeps working; a present option is type-checked
    let bool_opt = |name: &str, default: bool| -> Result<bool, Crash> {
        match opts.field(name) {
            Ok(Value::Bool(b)) => Ok(b),
            Ok(_) => Err(Crash::new(format!("treeWith: {} must be a Bool", name))),
            Err(_) => Ok(default),
        }
    };
    let int_opt = |name: &str, default: i64| -> Result<i64, Crash> {
        match opts.field(name) {
            Ok(Value::Int(n)) => Ok(n.to_string().parse::<i64>().unwrap_or(default)),
            Ok(_) => Err(Crash::new(format!("treeWith: {} must be an Int", name))),
            Err(_) => Ok(default),
        }
    };
    let detail = int_opt("detail", defaults.detail)?;
    let margin = bool_opt("margin", defaults.margin)?;
    let elide = bool_opt("elide", defaults.elide)?;
    let icons = bool_opt("icons", defaults.icons)?;
    let color = match opts.field("color") {
        Ok(v) => v.as_text()?.to_string(),
        Err(_) => defaults.color.clone(),
    };
    let lanes = int_opt("lanes", defaults.lanes)?;
    if lanes < 1 {
        return Err(Crash::new("treeWith: lanes must be at least 1"));
    }
    let author = bool_opt("author", defaults.author)?;
    let date = bool_opt("date", defaults.date)?;
    let files = bool_opt("files", defaults.files)?;
    let pal = Palette {
        on: color_enabled(&color),
    };
    let o = TreeOptions {
        detail,
        margin,
        elide,
        icons,
        color,
        lanes,
        author,
        date,
        files,
    };
    let text = tree_render(interp, &o, repo, &pal, true, term_w)?;
    Ok(Value::text(text))
}

struct CommitInfo {
    id: String,
    message: String,
    labels: Vec<String>,
    conflict: bool,
    empty: bool,
    immutable: bool,
    is_focus: bool,
    is_ancestor_of_focus: bool,
    is_child_of_ancestor: bool,
    meta: Option<crate::domain::MetaInfo>,
    size: Option<usize>, // lines added+removed against parent, up to SIZE_BAR_TOP
    nfiles: usize,       // number of files changed against parent (`files` column)
    detail_marks: Vec<(String, char)>,
    children: Vec<CommitInfo>,
}

impl CommitInfo {
    /// §Elision: a commit is *interesting* — never folded into a run — if it is
    /// the focus, labelled, conflicted, a leaf, or has more than one child; or
    /// if it is an ancestor of the focus (or a child of one) **off the trunk**.
    /// Trunk ancestors of the focus below the branch point are not interesting
    /// on that ground alone, so an uninteresting run of them folds (as in the
    /// worked example's `╎ 14`). The root is interesting only when it is not
    /// buried in a longer uninteresting run — a distant root folds into the run
    /// rather than being pinned at the top (§Option: far root). A run never
    /// straddles the trunk boundary.
    fn interesting(&self, trunk: &BTreeSet<String>) -> bool {
        self.is_focus
            || !self.labels.is_empty()
            || self.conflict
            || self.children.is_empty()
            || self.children.len() > 1
            || ((self.is_ancestor_of_focus || self.is_child_of_ancestor)
                && !trunk.contains(&self.id))
    }
}

/// A parent commit's snapshot as seen by its children: the files value,
/// loaded on first use — by the parent itself or by whichever child needs to
/// compare against it, since only the child knows that it does — plus its
/// path-keyed map built at most once and shared by every child's diff.
/// Keying a snapshot allocates a `Vec<String>` per entry, so rebuilding it for
/// each child dominated `treeFull`/`treeData` on a large history.
struct ParentSnap<'a> {
    commit: &'a Value,
    files: &'a std::cell::OnceCell<Value>,
    map: &'a std::cell::OnceCell<SnapMap>,
    /// the commit's id when its files are still the stored tree
    stored: Option<&'a str>,
}

impl ParentSnap<'_> {
    fn files(&self) -> Result<&Value, Crash> {
        if self.files.get().is_none() {
            let _ = self.files.set(self.commit.field("files")?);
        }
        Ok(self.files.get().expect("just set"))
    }

    fn map(&self) -> Result<&SnapMap, Crash> {
        if self.map.get().is_none() {
            let _ = self.map.set(snapshot_map_of(self.files()?)?);
        }
        Ok(self.map.get().expect("just set"))
    }
}

/// The id of the stored commit whose tree `commit`'s files are, read without
/// loading them: the tag on the backend's lazy list (`ThunkVal::origin`). An
/// edit that changes the files replaces that list, so this is `None` for them.
fn stored_origin(commit: &Value) -> Option<&str> {
    match commit {
        Value::Record(m) => match m.get("files") {
            Some(Value::Thunk(t)) => t.origin(),
            _ => None,
        },
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_info(
    interp: &mut Interp,
    opts: &TreeOptions,
    commit: &Value,
    children: &[Value],
    immutable: &BTreeSet<String>,
    focus_id: &str,
    anc: &BTreeSet<String>,
    parent_files: Option<&ParentSnap<'_>>,
    with_focus: bool,
    parent_is_ancestor: bool,
    parent_is_focus: bool,
) -> Result<CommitInfo, Crash> {
    let id = match commit.field("id")? {
        Value::Id(i) => i.to_string(),
        _ => String::new(),
    };
    let message = commit.field("message")?.as_text()?.to_string();
    let labels: Vec<String> = commit
        .field("labels")?
        .as_list()?
        .iter()
        .map(|l| l.as_text().map(|s| s.to_string()))
        .collect::<Result<_, _>>()?;
    // conflict and empty are O(1) backend queries when the backend can answer
    // them from tree ids (jj), without touching the file list at all. Those
    // answers are about the stored commits, so they are asked only while this
    // commit — and, for empty, its parent here — still holds its stored files:
    // a dry run (§1.2) renders edits that keep their ids but not their files.
    // Otherwise, as on the in-memory backend, fall back to walking the files.
    let stored = stored_origin(commit).filter(|o| *o == id);
    let conflict = match stored.and_then(|s| interp.backend.has_conflict(s)) {
        Some(c) => c,
        None => has_conflict(commit)?,
    };
    // `size` (a line count) and `detail_marks` are only rendered for detail ≥
    // 2 and for the focus and its neighbours, so skip the per-commit line
    // counting elsewhere — it dominates render time on large histories.
    let is_focus_pre = with_focus && id == focus_id;
    let child_is_focus = with_focus
        && children.iter().any(|c| {
            matches!(c.field("root").and_then(|r| r.field("id")), Ok(Value::Id(i)) if *i == focus_id)
        });
    let want_diff = opts.detail >= 2 || is_focus_pre || child_is_focus || parent_is_focus;
    let backend_empty = match (stored, parent_files.and_then(|p| p.stored)) {
        (Some(me), Some(parent)) => interp.backend.is_empty(me, parent),
        _ => None,
    };
    // this commit's own snapshot and its map, each built on first use — here
    // or by a child that compares against it — and shared with every child
    let my_files: std::cell::OnceCell<Value> = std::cell::OnceCell::new();
    let my_map: std::cell::OnceCell<SnapMap> = std::cell::OnceCell::new();
    let my_snap = ParentSnap {
        commit,
        files: &my_files,
        map: &my_map,
        stored,
    };
    // the files list is only materialized when something needs it: the diff
    // (detail/focus), the files data column, or the emptiness fallback
    let (empty, size, detail_marks, nfiles) = match parent_files {
        Some(parent) => {
            let empty = match backend_empty {
                Some(e) => e,
                // in any order: a snapshot stands for a tree (§7.3)
                None => crate::repo::snapshot_eq(my_snap.files()?, parent.files()?)?,
            };
            if want_diff {
                // both maps are built at most once per commit and reused by
                // this commit's children, so a diff costs one keying, not four
                let from_map = parent.map()?;
                let to_map = my_snap.map()?;
                let marks = touched_in_maps(from_map, to_map)?;
                // size: lines added plus removed against the parent (§7.11
                // column 5); a path changed without a changed line (an empty
                // file, a new file type) counts one, so only an empty commit
                // has no bar. The count stops where the bar does.
                let mut lines = 0usize;
                for (key, _, _) in &marks {
                    if lines >= SIZE_BAR_TOP {
                        break;
                    }
                    let cap = SIZE_BAR_TOP - lines;
                    lines += changed_lines(from_map.get(key), to_map.get(key), cap)?.max(1);
                }
                let marks2: Vec<(String, char)> =
                    marks.iter().map(|(p, m, _)| (p.join("/"), *m)).collect();
                (marks.is_empty(), Some(lines), marks2, marks.len())
            } else {
                // the files column is the number of paths changed against the
                // parent (specs/tree.md Step 4), so it diffs even without a bar
                let nfiles = if opts.files && !empty {
                    touched_in_maps(parent.map()?, my_snap.map()?)?.len()
                } else {
                    0
                };
                (empty, None, Vec::new(), nfiles)
            }
        }
        None => {
            // the top of the history: no parent to compare against, and so no
            // backend answer either — it is empty iff it has no files (the
            // root never counts as empty), and every file it has is added
            let nfiles = my_snap.files()?.as_list()?.len();
            (nfiles == 0 && id != ROOT_ID, None, Vec::new(), nfiles)
        }
    };
    let meta = interp.backend.meta(&id).ok();
    let is_focus = with_focus && id == focus_id;
    let is_ancestor_of_focus = anc.contains(&id);
    let mut infos = Vec::new();
    for c in children.iter() {
        infos.push(build_info(
            interp,
            opts,
            &c.field("root")?,
            c.field("children")?.as_list()?,
            immutable,
            focus_id,
            anc,
            Some(&my_snap),
            with_focus,
            is_ancestor_of_focus,
            is_focus_pre,
        )?);
    }    let immutable_flag = immutable.contains(&id);
    Ok(CommitInfo {
        id,
        message,
        labels,
        conflict,
        empty,
        immutable: immutable_flag,
        is_focus,
        is_ancestor_of_focus,
        is_child_of_ancestor: parent_is_ancestor,
        meta,
        size,
        nfiles,
        detail_marks,
        children: infos,
    })
}

fn tree_render(
    interp: &mut Interp,
    opts: &TreeOptions,
    repo: &Value,
    pal: &Palette,
    with_focus: bool,
    term_w: Option<usize>,
) -> Result<String, Crash> {
    // immutable set
    let immutable = crate::repo::compute_immutable(interp, repo).unwrap_or_default();
    let focus_id = match repo.field("root")?.field("id")? {
        Value::Id(i) => i.to_string(),
        _ => String::new(),
    };
    // ancestors of focus: the focus id plus every parent recorded in the
    // zipper's context frames — read directly rather than refocusing each
    // frame (an O(n) by_id per level would make this O(n²))
    let mut anc: BTreeSet<String> = BTreeSet::new();
    if let Ok(Value::Id(i)) = repo.field("root").and_then(|r| r.field("id")) {
        anc.insert(i.to_string());
    }
    if let Ok(ctx) = repo.field("context").and_then(|v| v.as_list().map(|x| x.to_vec())) {
        for frame in &ctx {
            if let Ok(pid) = id_of_frame_parent(frame) {
                anc.insert(pid);
            }
        }
    }
    // the whole history, from the top
    let top = crate::repo::by_id(repo, &top_id(repo)?)?.unwrap_or_else(|| repo.clone());
    let root_commit = top.field("root")?;
    let top_children = top.field("children")?;
    let root_info = build_info(
        interp,
        opts,
        &root_commit,
        top_children.as_list()?,
        &immutable,
        &focus_id,
        &anc,
        None,
        with_focus,
        false,
        false,
    )?;
    // trunk T (§Trunk)
    let trunk = compute_trunk(interp, repo)?;
    let lanes_n = opts.lanes.max(1) as usize;
    // Step 1 — display tree. The synthetic root is dropped when it is a pure
    // anchor (exactly one child): the first real commit then starts the tree
    // at the top, rather than pinning `⌂` above it. A root with several
    // children is a genuine branch point and is kept (§Option: drop root).
    let droot = if root_info.id == ROOT_ID && root_info.children.len() == 1 {
        make_display(&root_info.children[0], opts.elide, &trunk)
    } else {
        make_display(&root_info, opts.elide, &trunk)
    };
    // Step 2 — row order (topological, oldest first)
    let mut flat: Vec<&Display> = Vec::new();
    flatten_display(&droot, &mut flat);
    let preorder: BTreeMap<usize, usize> =
        flat.iter().enumerate().map(|(i, n)| (n.uid(), i)).collect();
    let mut ready: Vec<&Display> = vec![&droot];
    let mut rows: Vec<&Display> = Vec::new();
    while !ready.is_empty() {
        let best = ready
            .iter()
            .enumerate()
            .min_by_key(|(_, n)| (n.time(), preorder[&n.uid()]))
            .map(|(i, _)| i)
            .unwrap();
        let n = ready.remove(best);
        rows.push(n);
        for c in n.children() {
            ready.push(c);
        }
    }
    // Step 3 — lanes
    let placements = assign_lanes(&rows, &trunk, lanes_n);
    // id column: shortest unique prefix among the display tree's commits, min 4
    // (sorted once, so each row costs a lookup rather than a scan of them all)
    let mut ids: Vec<String> = rows
        .iter()
        .filter_map(|n| n.commit().map(|c| c.id.clone()))
        .collect();
    ids.sort();
    let prefix_len = |id: &str| -> usize {
        crate::eval::unique_prefix_in(&ids, id).len()
    };
    // Step 4 — draw
    draw_rows(&rows, &placements, opts, pal, lanes_n, with_focus, &prefix_len, term_w)
}

fn id_of_frame_parent(frame: &Value) -> Result<String, Crash> {
    match frame.field("parent")?.field("id")? {
        Value::Id(i) => Ok(i.to_string()),
        v => Err(Crash::new(format!("expected an Id, got a {}", v.kind_name()))),
    }
}

fn top_id(repo: &Value) -> Result<String, Crash> {
    // the topmost ancestor's id: the focus's own id when the context is empty,
    // otherwise the parent recorded in the *last* (outermost) zipper frame —
    // read directly, since refocusing each frame with by_id is O(n²) overall
    let ctx = repo.field("context").and_then(|v| v.as_list().map(|x| x.to_vec()))?;
    match ctx.last() {
        None => match repo.field("root")?.field("id")? {
            Value::Id(i) => Ok(i.to_string()),
            v => Err(Crash::new(format!("expected an Id, got a {}", v.kind_name()))),
        },
        Some(frame) => id_of_frame_parent(frame),
    }
}

/// The trunk (§Trunk): the config's `trunk` revset against the whole history.
/// Empty → the root alone; one commit → it and its ancestors; more → crash.
fn compute_trunk(interp: &mut Interp, repo: &Value) -> Result<BTreeSet<String>, Crash> {
    let mut set = BTreeSet::new();
    set.insert(top_id(repo)?);
    let v = interp.apply_cached_revset("trunk", repo)?;
    let mut named: Vec<String> = Vec::new();
    for idv in v.as_list()?.iter() {
        match idv {
            Value::Id(id) => named.push(id.to_string()),
            _ => return Err(Crash::new("treeWith: trunk returned a non-Id")),
        }
    }
    named.sort();
    named.dedup();
    match named.len() {
        0 => Ok(set),
        1 => {
            let pmap: BTreeMap<String, String> =
                crate::repo::parent_map(repo)?.into_iter().collect();
            let mut cur = named[0].clone();
            loop {
                set.insert(cur.clone());
                match pmap.get(&cur) {
                    Some(p) => cur = p.clone(),
                    None => break,
                }
            }
            Ok(set)
        }
        n => Err(Crash::new(format!("treeWith: trunk names {} revisions", n))),
    }
}

// ----------------------------------------------------------------------
// Step 1 — the display tree (§Elision)
// ----------------------------------------------------------------------

enum Display {
    Commit {
        uid: usize,
        info: CommitInfo,
        children: Vec<Display>,
    },
    Run {
        uid: usize,
        count: usize,
        time: Option<i64>,
        in_trunk: bool,
        child: Box<Display>,
    },
    Collapsed {
        uid: usize,
        info: CommitInfo,
        hidden: usize,
    },
}

impl Display {
    fn uid(&self) -> usize {
        match self {
            Display::Commit { uid, .. } => *uid,
            Display::Run { uid, .. } => *uid,
            Display::Collapsed { uid, .. } => *uid,
        }
    }

    /// The node's time for the row order (Step 1). A node with no stored
    /// counterpart (minted in this program) has time ∞: it sorts after every
    /// stored one, not first as a `None` would.
    fn time(&self) -> i64 {
        let t = match self {
            Display::Commit { info, .. } => info.meta.as_ref().map(|m| m.time),
            Display::Run { time, .. } => *time,
            Display::Collapsed { info, .. } => info.meta.as_ref().map(|m| m.time),
        };
        t.unwrap_or(i64::MAX)
    }

    fn children(&self) -> &[Display] {
        match self {
            Display::Commit { children, .. } => children,
            Display::Run { child, .. } => std::slice::from_ref(&**child),
            Display::Collapsed { .. } => &[],
        }
    }

    fn commit(&self) -> Option<&CommitInfo> {
        match self {
            Display::Commit { info, .. } => Some(info),
            Display::Collapsed { info, .. } => Some(info),
            Display::Run { .. } => None,
        }
    }

    fn in_trunk(&self, trunk: &BTreeSet<String>) -> bool {
        match self {
            Display::Run { in_trunk, .. } => *in_trunk,
            _ => self.commit().map(|c| trunk.contains(&c.id)).unwrap_or(false),
        }
    }
}

fn make_display(info: &CommitInfo, elide: bool, trunk: &BTreeSet<String>) -> Display {
    let mut uid = 0usize;
    make_display_rec(info, elide, trunk, true, &mut uid)
}

fn make_display_rec(
    info: &CommitInfo,
    elide: bool,
    trunk: &BTreeSet<String>,
    near: bool,
    uid: &mut usize,
) -> Display {
    if !elide {
        let my = *uid;
        *uid += 1;
        let children = info
            .children
            .iter()
            .map(|c| make_display_rec(c, elide, trunk, true, uid))
            .collect();
        return Display::Commit {
            uid: my,
            info: clone_info(info, Vec::new()),
            children,
        };
    }
    if !near && !info.is_ancestor_of_focus && !info.is_focus {
        let my = *uid;
        *uid += 1;
        return Display::Collapsed {
            uid: my,
            info: clone_info(info, Vec::new()),
            hidden: count_descendants(info),
        };
    }
    if info.interesting(trunk) {
        let my = *uid;
        *uid += 1;
        let children = info
            .children
            .iter()
            .map(|c| make_display_rec(c, elide, trunk, false, uid))
            .collect();
        return Display::Commit {
            uid: my,
            info: clone_info(info, Vec::new()),
            children,
        };
    }
    // uninteresting: gather the run down the single line of descent until the
    // next interesting commit (which always exists: leaves are interesting).
    // A run never straddles the trunk boundary: it stops before a commit whose
    // trunk membership differs from the run's first commit.
    let first_in_trunk = trunk.contains(&info.id);
    let mut chain: Vec<&CommitInfo> = Vec::new();
    let mut cur = info;
    while !cur.interesting(trunk) && trunk.contains(&cur.id) == first_in_trunk {
        chain.push(cur);
        cur = &cur.children[0];
    }
    // A distant root folds into the run, but a root that would form a run of
    // one — it is right next to an interesting commit, so it is not far — stays
    // visible as a normal commit at the top of the tree.
    if chain.len() == 1 && chain[0].id == ROOT_ID {
        let my = *uid;
        *uid += 1;
        let children = info
            .children
            .iter()
            .map(|c| make_display_rec(c, elide, trunk, near, uid))
            .collect();
        return Display::Commit {
            uid: my,
            info: clone_info(info, Vec::new()),
            children,
        };
    }
    let count = chain.len();
    let in_trunk = chain.iter().all(|c| trunk.contains(&c.id));
    let my = *uid;
    *uid += 1;
    Display::Run {
        uid: my,
        count,
        time: chain[0].meta.as_ref().map(|m| m.time),
        in_trunk,
        child: Box::new(make_display_rec(cur, elide, trunk, near, uid)),
    }
}

fn count_descendants(info: &CommitInfo) -> usize {
    info.children.iter().map(|c| 1 + count_descendants(c)).sum()
}

fn clone_info(info: &CommitInfo, children: Vec<CommitInfo>) -> CommitInfo {
    CommitInfo {
        id: info.id.clone(),
        message: info.message.clone(),
        labels: info.labels.clone(),
        conflict: info.conflict,
        empty: info.empty,
        immutable: info.immutable,
        is_focus: info.is_focus,
        is_ancestor_of_focus: info.is_ancestor_of_focus,
        is_child_of_ancestor: info.is_child_of_ancestor,
        meta: info.meta.clone(),
        size: info.size,
        nfiles: info.nfiles,
        detail_marks: info.detail_marks.clone(),
        children,
    }
}

fn flatten_display<'a>(n: &'a Display, out: &mut Vec<&'a Display>) {
    out.push(n);
    for c in n.children() {
        flatten_display(c, out);
    }
}

// ----------------------------------------------------------------------
// Step 3 — lanes
// ----------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Rail {
    Live,
    Reserved(usize),
}

#[derive(Clone, Debug)]
struct Placement {
    lane: Option<usize>,
    fork: Option<(usize, bool)>, // (source lane, source empties below)
    // on a flattened last child of the trunk head (or of the root of an empty
    // trunk): its parent's lane, which empties below this row though no fork
    // is drawn (Step 3, §Overflow)
    ends: Option<usize>,
    reservations: Vec<(usize, usize)>, // (lane, child uid)
    flattened: bool,
}

fn mark_subtree(n: &Display, set: &mut BTreeSet<usize>) {
    set.insert(n.uid());
    for c in n.children() {
        mark_subtree(c, set);
    }
}

fn assign_lanes(rows: &[&Display], trunk: &BTreeSet<String>, lanes_n: usize) -> Vec<Placement> {
    let mut rails: Vec<Option<Rail>> = vec![None; lanes_n];
    let mut out: Vec<Placement> = Vec::new();
    let mut flattened: BTreeSet<usize> = BTreeSet::new();
    // each node's row: "last child" and "after t" are in row order, which is
    // not sibling order once an edit appends a child (`new`, `rebase`)
    let row_of: BTreeMap<usize, usize> =
        rows.iter().enumerate().map(|(i, n)| (n.uid(), i)).collect();
    for (idx, n) in rows.iter().enumerate() {
        let in_trunk = n.in_trunk(trunk);
        let parent = rows[..idx]
            .iter()
            .copied()
            .find(|p| p.children().iter().any(|c| c.uid() == n.uid()));
        let parent_lane = parent.and_then(|p| out[rows[..idx].iter().position(|x| x.uid() == p.uid()).unwrap()].lane);
        let is_last_child = parent
            .map(|p| p.children().iter().all(|c| row_of[&c.uid()] <= idx))
            .unwrap_or(false);
        let was_flat = flattened.contains(&n.uid());
        let mut lane: Option<usize> = None;
        let mut fork: Option<(usize, bool)> = None;
        let mut ends: Option<usize> = None;
        if !was_flat {
            if idx == 0 {
                lane = Some(0);
            } else if in_trunk {
                lane = Some(0);
            } else if let Some(l) =
                rails.iter().position(|r| matches!(r, Some(Rail::Reserved(u)) if *u == n.uid()))
            {
                lane = Some(l);
            } else if let (Some(p), Some(pl)) = (parent, parent_lane) {
                if !p.in_trunk(trunk) && is_last_child {
                    // rule 4: inherit p's live rail
                    lane = Some(pl);
                } else {
                    // rule 5: fork to the leftmost empty lane right of p's lane
                    match (pl + 1..lanes_n).find(|&l| rails[l].is_none()) {
                        Some(l) => {
                            lane = Some(l);
                            if is_last_child {
                                fork = Some((pl, true));
                                rails[pl] = None;
                            } else {
                                fork = Some((pl, false));
                            }
                        }
                        None => {
                            // overflow is rule 5 too: when n is p's last
                            // child (p is then the trunk head or the root of
                            // an empty trunk, as any other trunk commit
                            // reserves lanes for its later side children),
                            // p's lane empties below this row (Step 3)
                            mark_subtree(n, &mut flattened);
                            if is_last_child {
                                rails[pl] = None;
                                ends = Some(pl);
                            }
                        }
                    }
                }
            } else {
                mark_subtree(n, &mut flattened);
            }
        }
        let now_flat = flattened.contains(&n.uid());
        if !now_flat {
            if let Some(l) = lane {
                rails[l] = if n.children().is_empty() {
                    None
                } else {
                    Some(Rail::Live)
                };
            }
        }
        // reservations: a trunk node with a trunk child reserves lanes for its
        // side children that come after the trunk child in row order
        let mut reservations: Vec<(usize, usize)> = Vec::new();
        if !now_flat && in_trunk {
            let cs = n.children();
            if let Some(t) = cs.iter().find(|c| c.in_trunk(trunk)) {
                let mut after: Vec<&Display> = cs
                    .iter()
                    .filter(|c| row_of[&c.uid()] > row_of[&t.uid()])
                    .collect();
                after.sort_by_key(|c| row_of[&c.uid()]);
                for c in after {
                    if flattened.contains(&c.uid()) {
                        continue;
                    }
                    match (1..lanes_n).find(|&l| rails[l].is_none()) {
                        Some(l) => {
                            rails[l] = Some(Rail::Reserved(c.uid()));
                            reservations.push((l, c.uid()));
                        }
                        None => mark_subtree(c, &mut flattened),
                    }
                }
            }
        }
        out.push(Placement {
            lane: if now_flat { None } else { lane },
            fork: if now_flat { None } else { fork },
            ends,
            reservations,
            flattened: now_flat,
        });
    }
    out
}

// ----------------------------------------------------------------------
// Step 4 — drawing
// ----------------------------------------------------------------------

struct RowText {
    gutter: String,
    id: String,
    bar: String,
    msg: String,
    labels: String,
    age: String,
    initials: String,
    initials_author: String,
    // extra right-side columns, already joined in display order (§Step 4):
    // date, files changed, full author name — only the enabled ones, plain
    meta_extra: Vec<String>,
}

fn build_row_text(
    n: &Display,
    rows: &[&Display],
    opts: &TreeOptions,
    pal: &Palette,
    with_focus: bool,
    prefix_len: &dyn Fn(&str) -> usize,
    id_w: usize,
) -> RowText {
    let c = n.commit();
    let is_focus = c.map(|x| x.is_focus).unwrap_or(false);
    let gutter = if is_focus && with_focus { "▶ " } else { "  " }.to_string();
    // id (blank on a run row, whose count `rail_row` draws: the part that
    // overflows the rails area runs on into the id column)
    let id = match n {
        Display::Run { .. } => " ".repeat(id_w),
        _ => match c {
            Some(info) => {
                let k = prefix_len(&info.id);
                let p = &info.id[..k.min(info.id.len())];
                // prefix with `@` so the id matches the id-literal syntax (@wqzt)
                let colored = if info.conflict {
                    pal.red(&format!("@{}", p))
                } else if info.immutable {
                    pal.blue(&format!("@{}", p))
                } else {
                    format!("@{}", p)
                };
                let plain = k + 1; // +1 for the @
                let padded = if plain < id_w {
                    format!("{}{}", colored, " ".repeat(id_w - plain))
                } else {
                    colored
                };
                padded
            }
            None => " ".repeat(id_w),
        },
    };
    // size bar
    let focus_idx = rows
        .iter()
        .position(|m| m.commit().map(|x| x.is_focus).unwrap_or(false));
    let is_focus_parent = focus_idx
        .map(|fi| {
            rows[..fi].iter().any(|p| {
                p.children()
                    .iter()
                    .any(|ch| ch.uid() == rows[fi].uid()) && p.uid() == n.uid()
            })
        })
        .unwrap_or(false);
    let is_focus_child = focus_idx
        .map(|fi| rows[fi].children().iter().any(|ch| ch.uid() == n.uid()))
        .unwrap_or(false);
    let show_bar = opts.detail >= 1
        && c.map(|x| opts.detail >= 2 || x.is_focus || is_focus_parent || is_focus_child)
            .unwrap_or(false);
    let bar = if show_bar {
        c.map(|x| size_bar(x.size, pal)).unwrap_or_default()
    } else {
        String::new()
    };
    // message, coloured by state (§Colour): focus bold, conflict red, empty
    // dim-italic; the collapsed marker is dim
    let msg_first = c.map(|x| x.message.lines().next().unwrap_or("")).unwrap_or("");
    let mut msg = match c {
        Some(info) if info.empty => pal.dim_italic(msg_first),
        Some(info) if info.is_focus => pal.bold(msg_first),
        Some(info) if info.conflict => pal.red(msg_first),
        _ => msg_first.to_string(),
    };
    if let Display::Collapsed { hidden, .. } = n {
        if *hidden > 0 {
            if !msg.is_empty() {
                msg.push_str("  ");
            }
            msg.push_str(&pal.dim(&format!("⋯ {}", hidden)));
        }
    }
    let labels = c.map(|x| x.labels.join("  ")).unwrap_or_default();
    let (age, init, author) = match c.and_then(|x| x.meta.as_ref()) {
        // the root's margin is not shown (§worked example)
        Some(m) if opts.margin && c.map(|x| x.id != ROOT_ID).unwrap_or(false) => {
            (render_age(m.time), initials(&m.author), m.author.clone())
        }
        _ => (String::new(), String::new(), String::new()),
    };
    // extra columns (§Step 4): date, files changed, full author name — one
    // entry per enabled column, blank without metadata (a minted commit), so
    // entry k is the same column on every row
    let mut meta_extra: Vec<String> = Vec::new();
    if let Some(info) = c {
        if info.id != ROOT_ID {
            if opts.date {
                meta_extra.push(info.meta.as_ref().map(|m| render_date(m.time)).unwrap_or_default());
            }
            if opts.files {
                meta_extra.push(format!("{} files", info.nfiles));
            }
            if opts.author {
                meta_extra.push(info.meta.as_ref().map(|m| m.author.clone()).unwrap_or_default());
            }
        }
    }
    RowText {
        gutter,
        id,
        bar,
        msg,
        labels,
        age,
        initials: init,
        initials_author: author,
        meta_extra,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_rows(
    rows: &[&Display],
    placements: &[Placement],
    opts: &TreeOptions,
    pal: &Palette,
    lanes_n: usize,
    with_focus: bool,
    prefix_len: &dyn Fn(&str) -> usize,
    term_w: Option<usize>,
) -> Result<String, Crash> {
    let rail_chars = 2 * lanes_n;
    // each row's rails, drawn once; a run row's go on past the rails area
    // when its count does not fit
    let rails: Vec<(Vec<char>, Vec<bool>)> = (0..rows.len())
        .map(|idx| rail_row(rows, placements, idx, lanes_n, opts))
        .collect();
    // id column width: 4-char min prefix plus the `@` literal prefix, and
    // what a run count writes into it past the rails area and the space
    // after it
    let mut id_w = 5usize;
    for (chars, _) in &rails {
        id_w = id_w.max(chars.len().saturating_sub(rail_chars + 1));
    }
    for n in rows {
        if let Some(c) = n.commit() {
            id_w = id_w.max(prefix_len(&c.id) + 1); // +1 for the `@`
        }
    }
    let label_col = rows
        .iter()
        .any(|n| n.commit().map(|c| !c.labels.is_empty()).unwrap_or(false));
    // build row texts
    let mut texts: Vec<RowText> = Vec::new();
    for n in rows {
        texts.push(build_row_text(n, rows, opts, pal, with_focus, prefix_len, id_w));
    }
    // Columns 4–8 start at the same offset on every row (§Step 4). The size
    // bar's column is there when any row draws a bar — ` ▅`, two columns —
    // and blank on the rows that draw none, so a bar never shifts a message.
    let bar_col = texts.iter().any(|t| !t.bar.is_empty());
    // each row up to its message: gutter, rails, id and bar
    let mut heads: Vec<String> = Vec::new();
    for (idx, n) in rows.iter().enumerate() {
        let pl = &placements[idx];
        let t = &texts[idx];
        let (chars, lane0) = &rails[idx];
        let mut line = String::new();
        line.push_str(&t.gutter);
        // the node glyph (on a commit row) is meaning-coloured; rails lane-coloured
        let glyph = match (n, pl.lane) {
            (Display::Commit { info, .. }, Some(l)) => Some((2 * l, glyph_ansi(info))),
            _ => None,
        };
        line.push_str(&color_rails(chars, lane0, pal, glyph));
        line.push(' ');
        line.push_str(&t.id);
        if bar_col {
            line.push(' ');
            line.push_str(if t.bar.is_empty() { " " } else { &t.bar });
        }
        line.push_str("  ");
        heads.push(line);
    }
    // the columns are placed by display width as drawn (an `icons` glyph is
    // two columns wide), from the message column on
    let msg_col = rows
        .iter()
        .zip(&heads)
        .filter(|(n, _)| n.commit().is_some())
        .map(|(_, h)| width(h))
        .max()
        .unwrap_or(0);
    let lab_w = texts.iter().map(|t| width(&t.labels)).max().unwrap_or(0);
    // the data columns, each as wide as its widest entry, so that the margin
    // after them starts at one offset too
    let mut extra_w: Vec<usize> = Vec::new();
    for t in &texts {
        for (k, e) in t.meta_extra.iter().enumerate() {
            if k == extra_w.len() {
                extra_w.push(0);
            }
            extra_w[k] = extra_w[k].max(width(e));
        }
    }
    // the margin's age and initials, each as wide as its widest entry, so
    // that a `13m` among `9m`s or a single initial moves nothing
    let margin_w = (
        texts.iter().map(|t| width(&t.age)).max().unwrap_or(0),
        texts.iter().map(|t| width(&t.initials)).max().unwrap_or(0),
    );
    let metas: Vec<String> = texts
        .iter()
        .map(|t| meta_block(t, &extra_w, margin_w, opts, pal))
        .collect();
    // message truncation to the terminal width (§Step 4): the widest part
    // right of the message column (labels, data, margin) sets the room left
    // for every message, and only messages are cut to it, so the label and
    // margin columns hold
    if let Some(w) = term_w {
        let right = texts
            .iter()
            .zip(&metas)
            .map(|(t, m)| {
                if !m.is_empty() {
                    (if label_col { 2 + lab_w } else { 0 }) + 2 + width(m)
                } else if !t.labels.is_empty() {
                    2 + width(&t.labels)
                } else {
                    0
                }
            })
            .max()
            .unwrap_or(0);
        let room = w.saturating_sub(msg_col + right);
        for t in texts.iter_mut() {
            t.msg = cut_to_width(&t.msg, room);
        }
    }
    let msg_w = texts.iter().map(|t| width(&t.msg)).max().unwrap_or(0);
    let lab_off = msg_col + msg_w + 2;
    let meta_off = if label_col { lab_off + lab_w + 2 } else { lab_off };
    let mut lines: Vec<String> = Vec::new();
    for (idx, n) in rows.iter().enumerate() {
        let t = &texts[idx];
        let mut line = std::mem::take(&mut heads[idx]);
        if n.commit().is_some() {
            pad_to(&mut line, msg_col);
        }
        line.push_str(&t.msg);
        if label_col && !t.labels.is_empty() {
            pad_to(&mut line, lab_off);
            line.push_str(&pal.green(&t.labels));
        }
        if !metas[idx].is_empty() {
            pad_to(&mut line, meta_off);
            line.push_str(&metas[idx]);
        }
        // the whole focus row is highlighted with a background band across the
        // full terminal width (§Colour; colour only — the ▶ gutter still marks
        // the focus when colour is off)
        let is_focus_row = n.commit().map(|c| c.is_focus).unwrap_or(false);
        if is_focus_row && pal.on {
            let padded = match term_w {
                Some(w) if width(&line) < w => {
                    format!("{}{}", line, " ".repeat(w - width(&line)))
                }
                _ => line.clone(),
            };
            lines.push(pal.bg_line("48;5;236", &padded));
        } else {
            lines.push(line);
        }
        // detail line (detail = 2, focus only)
        if opts.detail >= 2
            && with_focus
            && n.commit().map(|c| c.is_focus && !c.detail_marks.is_empty()).unwrap_or(false)
        {
            let c = n.commit().unwrap();
            let (dchars, dlane0) = detail_rail_row(rows, placements, idx, lanes_n);
            let mut dline = String::new();
            dline.push_str("  ");
            dline.push_str(&color_rails(&dchars, &dlane0, pal, None));
            dline.push(' ');
            // the marks start two columns into the id column (§Step 4)
            dline.push_str("  ");
            let marks: Vec<String> = c
                .detail_marks
                .iter()
                .map(|(p, m)| {
                    let ms = m.to_string();
                    let colored = match m {
                        '+' => pal.green(&ms),
                        '−' => pal.red(&ms),
                        '~' => pal.yellow(&ms),
                        '✖' => pal.red(&ms),
                        _ => ms,
                    };
                    format!("{} {}", colored, p)
                })
                .collect();
            dline.push_str(&marks.join("   "));
            lines.push(dline);
        }
    }
    // no trailing whitespace on any row
    for l in lines.iter_mut() {
        while l.ends_with(' ') {
            l.pop();
        }
    }
    // Legend (§Legend): explain the symbols that actually appear, faintly, on
    // the right of the tree if it fits the terminal width, else at the bottom.
    append_legend(&mut lines, rows, opts, pal, term_w);
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

/// Which symbols appear in the rendered tree, used to build the legend. Only
/// symbols that are actually shown are explained (§Legend option: only-used).
struct UsedSymbols {
    glyphs: Vec<&'static str>, // present node glyphs, in legend order
    run: bool,                 // a ╎ n run row
    collapsed: bool,           // a ⋯ n collapsed node
    focus_gutter: bool,        // ▶ present (a focus row exists)
    detail_marks: Vec<char>,   // + ~ − ✖ present in the focus's detail line
}

fn collect_used(rows: &[&Display], opts: &TreeOptions, icons: bool) -> UsedSymbols {
    let mut present: BTreeSet<&'static str> = BTreeSet::new();
    let mut run = false;
    let mut collapsed = false;
    let mut focus_gutter = false;
    let mut marks: BTreeSet<char> = BTreeSet::new();
    for n in rows {
        match n {
            Display::Run { .. } => run = true,
            Display::Commit { info, .. } | Display::Collapsed { info, .. } => {
                // a collapsed node only shows the ⋯ n marker when it hides
                // descendants; only then is the collapsed symbol used
                if let Display::Collapsed { hidden, .. } = n {
                    if *hidden > 0 {
                        collapsed = true;
                    }
                }
                present.insert(glyph_for(info, icons));
                if info.is_focus {
                    focus_gutter = true;
                    if opts.detail >= 2 {
                        for (_, m) in &info.detail_marks {
                            marks.insert(*m);
                        }
                    }
                }
            }
        }
    }
    // legend glyph order: focus, ancestor, other, immutable, empty, conflict, root
    const ORDER: [&str; 14] =
        ["◉", "🌸", "●", "🌿", "○", "🍃", "◆", "🪨", "◌", "🫙", "⊗", "🔥", "⌂", "🌱"];
    let glyphs = ORDER.iter().filter(|g| present.contains(**g)).copied().collect();
    UsedSymbols {
        glyphs,
        run,
        collapsed,
        focus_gutter,
        detail_marks: marks.into_iter().collect(),
    }
}

fn glyph_meaning(g: &str) -> &'static str {
    match g {
        "◉" | "🌸" => "focus",
        "●" | "🌿" => "ancestor of focus",
        "○" | "🍃" => "other commit",
        "◆" | "🪨" => "immutable",
        "◌" | "🫙" => "empty",
        "⊗" | "🔥" => "conflict",
        "⌂" | "🌱" => "root",
        _ => "",
    }
}

fn mark_meaning(m: char) -> &'static str {
    match m {
        '+' => "added",
        '~' => "modified",
        '−' => "deleted",
        '✖' => "unresolved",
        _ => "",
    }
}

/// Build the legend lines (plain, uncoloured text) for the used symbols.
fn legend_lines(used: &UsedSymbols) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for g in &used.glyphs {
        out.push(format!("{} {}", g, glyph_meaning(g)));
    }
    if used.focus_gutter {
        out.push("▶ current commit".to_string());
    }
    if used.run {
        out.push("╎ n run of n commits".to_string());
    }
    if used.collapsed {
        out.push("⋯ n collapsed, n hidden".to_string());
    }
    for m in &used.detail_marks {
        out.push(format!("{} {}", m, mark_meaning(*m)));
    }
    out
}

/// Place the legend. Try the right of the tree: each legend entry is appended,
/// faint, to the right of a tree row starting one row from the top, past the
/// widest tree line. If that would exceed the terminal width (or there is no
/// terminal), put the legend at the bottom instead.
fn append_legend(
    lines: &mut Vec<String>,
    rows: &[&Display],
    opts: &TreeOptions,
    pal: &Palette,
    tty_width: Option<usize>,
) {
    let used = collect_used(rows, opts, opts.icons);
    let entries = legend_lines(&used);
    if entries.is_empty() {
        return;
    }
    let tree_w = lines.iter().map(|l| width(l)).max().unwrap_or(0);
    let legend_w = entries.iter().map(|e| width(e)).max().unwrap_or(0);
    let gap = 4;
    let right_total = tree_w + gap + legend_w;
    let fits_right = tty_width.map_or(false, |w| right_total <= w);
    if fits_right {
        // pad every tree line to tree_w, then append the legend entries dim
        for l in lines.iter_mut() {
            let d = width(l);
            if d < tree_w {
                l.push_str(&" ".repeat(tree_w - d));
            }
        }
        for (k, e) in entries.iter().enumerate() {
            if k < lines.len() {
                lines[k].push_str(&" ".repeat(gap));
                lines[k].push_str(&pal.dim(e));
            } else {
                let mut l = " ".repeat(tree_w + gap);
                l.push_str(&pal.dim(e));
                lines.push(l);
            }
        }
    } else {
        lines.push(String::new());
        for e in &entries {
            lines.push(pal.dim(e));
        }
    }
}

/// rails state below row `upto` (exclusive): which lanes hold rails, and which
/// of those are lane-0
fn rail_state_below(
    rows: &[&Display],
    placements: &[Placement],
    upto: usize,
    lanes_n: usize,
) -> Vec<Option<bool>> {
    let mut state: Vec<Option<bool>> = vec![None; lanes_n]; // Some(lane==0)
    for (j, m) in rows.iter().enumerate().take(upto) {
        let p = &placements[j];
        if let Some((src, clears)) = p.fork {
            if clears {
                state[src] = None;
            }
        }
        if let Some(l) = p.ends {
            state[l] = None;
        }
        if let Some(l) = p.lane {
            state[l] = if m.children().is_empty() {
                None
            } else {
                Some(l == 0)
            };
        }
        for &(l, _) in &p.reservations {
            state[l] = Some(false);
        }
    }
    state
}

/// rails characters for the row of node `idx` (§Step 4, the character table).
/// Returns the characters and, per character, whether it belongs to lane 0.
/// On a run row they go on past the rails area when its count does not fit.
fn rail_row(
    rows: &[&Display],
    placements: &[Placement],
    idx: usize,
    lanes_n: usize,
    opts: &TreeOptions,
) -> (Vec<char>, Vec<bool>) {
    let n = rows[idx];
    let pl = &placements[idx];
    let mut chars = vec![' '; 2 * lanes_n];
    let mut lane0 = vec![false; 2 * lanes_n];
    let state = rail_state_below(rows, placements, idx, lanes_n);
    // horizontal segments: (start, end) in lane coordinates
    let mut segments: Vec<(usize, usize)> = Vec::new();
    if let Some((src, _)) = pl.fork {
        if let Some(tgt) = pl.lane {
            segments.push((src, tgt));
        }
    }
    if let Some(l) = pl.lane {
        if let Some(max_r) = pl.reservations.iter().map(|&(l, _)| l).max() {
            segments.push((l, max_r));
        }
    }
    let in_segment = |i: usize| segments.iter().any(|&(a, b)| a < i && i < b);
    for i in 0..lanes_n {
        let c = if Some(i) == pl.lane {
            match n {
                Display::Run { .. } => '╎',
                _ => glyph_for(n.commit().unwrap(), opts.icons)
                    .chars()
                    .next()
                    .unwrap(),
            }
        } else if pl.reservations.iter().any(|&(l, _)| l == i) {
            let right = pl.reservations.iter().any(|&(l, _)| l > i);
            if right {
                '┬'
            } else {
                '╮'
            }
        } else if pl.fork.map(|(src, _)| src == i).unwrap_or(false) {
            if pl.fork.unwrap().1 {
                '╰'
            } else {
                '├'
            }
        } else if in_segment(i) {
            if state[i].is_some() {
                '┼'
            } else {
                '─'
            }
        } else if state[i].is_some() {
            '│'
        } else if pl.flattened && i == lanes_n - 1 {
            '»'
        } else {
            ' '
        };
        chars[2 * i] = c;
        lane0[2 * i] = i == 0;
    }
    // on a run row the count follows ╎, or the rightmost rail drawn right of
    // it, so that it covers none; it is written into the rails area and, if
    // needed, on through the space after it into the blank id column. A run
    // reserves no lane and any fork on its row comes from its left, so the
    // rightmost character drawn so far is ╎ or such a rail.
    if let (Display::Run { count, .. }, Some(l)) = (n, pl.lane) {
        let from = chars.iter().rposition(|&c| c != ' ').unwrap_or(2 * l) + 1;
        for (k, d) in format!(" {}", count).chars().enumerate() {
            let pos = from + k;
            if pos == chars.len() {
                chars.push(d);
                lane0.push(l == 0);
            } else {
                chars[pos] = d;
                lane0[pos] = l == 0;
            }
        }
    }
    for i in 0..lanes_n {
        let inside = segments.iter().any(|&(a, b)| 2 * i + 1 > 2 * a && 2 * i + 1 < 2 * b);
        if inside {
            chars[2 * i + 1] = '─';
            lane0[2 * i + 1] = i == 0;
        }
    }
    (chars, lane0)
}

/// detail line (§Step 4): `│` in every lane that holds a rail below the focus
/// row, including the focus's own lane if it has children
fn detail_rail_row(
    rows: &[&Display],
    placements: &[Placement],
    idx: usize,
    lanes_n: usize,
) -> (Vec<char>, Vec<bool>) {
    let state = rail_state_below(rows, placements, idx + 1, lanes_n);
    let mut chars = vec![' '; 2 * lanes_n];
    let mut lane0 = vec![false; 2 * lanes_n];
    for i in 0..lanes_n {
        if let Some(is0) = state[i] {
            chars[2 * i] = '│';
            lane0[2 * i] = is0;
        }
    }
    (chars, lane0)
}

/// Colour the rails. Rail connectors are lane-coloured (lane 0 = immutable
/// blue, others dim); the node glyph at `glyph` (its lane position and ANSI
/// code) is meaning-coloured. A glyph two columns wide (`icons`) fills both
/// characters of its lane, so the one after it is not drawn (§Glyphs).
fn color_rails(
    chars: &[char],
    lane0: &[bool],
    pal: &Palette,
    glyph: Option<(usize, &str)>,
) -> String {
    let mut s = String::new();
    let mut covered = false;
    for (i, c) in chars.iter().enumerate() {
        if std::mem::take(&mut covered) {
            continue;
        }
        covered = UnicodeWidthChar::width(*c) == Some(2);
        let t = c.to_string();
        if *c == ' ' {
            s.push(*c);
        } else if let Some((pos, code)) = glyph {
            if i == pos {
                s.push_str(&pal.code(code, &t));
            } else if lane0[i] {
                s.push_str(&pal.blue(&t));
            } else {
                s.push_str(&pal.dim(&t));
            }
        } else if lane0[i] {
            s.push_str(&pal.blue(&t));
        } else {
            s.push_str(&pal.dim(&t));
        }
    }
    s
}

/// `s` cut to display width `w`, ending with `…` when anything is cut (and
/// there is a column for it): the message on a terminal (§Step 4). Colour
/// escapes are kept whole and not counted, and those past the cut still
/// follow it, so every style `s` opens is closed.
fn cut_to_width(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    let mut kept = String::new();
    // escapes not yet followed by a kept character
    let mut pending = String::new();
    let mut used = 0usize;
    let mut in_esc = false;
    let mut full = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_esc = true;
        }
        if in_esc {
            in_esc = c != 'm';
            pending.push(c);
            continue;
        }
        if full {
            continue;
        }
        // one column is left for the `…`
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw >= w {
            full = true;
            continue;
        }
        kept.push_str(&pending);
        pending.clear();
        kept.push(c);
        used += cw;
    }
    let ellipsis = if w == 0 { "" } else { "…" };
    format!("{}{}{}", kept.trim_end_matches(' '), ellipsis, pending)
}

/// Pad `line` with spaces to display column `col`.
fn pad_to(line: &mut String, col: usize) {
    let w = width(line);
    if w < col {
        line.push_str(&" ".repeat(col - w));
    }
}

/// A row's metadata block (§Step 4): the data columns (date / files /
/// author), each padded to its column's width in `extra_w`, then the margin
/// (age and initials), each right-aligned in its width in `margin_w`. Empty
/// when the row shows none of them.
fn meta_block(
    t: &RowText,
    extra_w: &[usize],
    margin_w: (usize, usize),
    opts: &TreeOptions,
    pal: &Palette,
) -> String {
    let mut meta: Vec<String> = Vec::new();
    for (e, w) in t.meta_extra.iter().zip(extra_w) {
        let mut s = if e.is_empty() { String::new() } else { pal.grey(4, e) };
        s.push_str(&" ".repeat(w - width(e)));
        meta.push(s);
    }
    if opts.margin && !t.age.is_empty() {
        let (age_w, init_w) = margin_w;
        let mut age = " ".repeat(age_w - width(&t.age));
        age.push_str(&pal.grey(4, &t.age));
        meta.push(age);
        let mut init = " ".repeat(init_w - width(&t.initials));
        init.push_str(&pal.author(&t.initials_author, &t.initials));
        meta.push(init);
    }
    meta.join("  ").trim_end().to_string()
}

fn glyph_for(info: &CommitInfo, icons: bool) -> &'static str {
    if !icons {
        if info.id == ROOT_ID {
            "⌂"
        } else if info.conflict {
            "⊗"
        } else if info.empty {
            "◌"
        } else if info.immutable {
            "◆"
        } else if info.is_focus {
            "◉"
        } else if info.is_ancestor_of_focus {
            "●"
        } else {
            "○"
        }
    } else if info.id == ROOT_ID {
        "🌱"
    } else if info.conflict {
        "🔥"
    } else if info.empty {
        "🫙"
    } else if info.immutable {
        "🪨"
    } else if info.is_focus {
        "🌸"
    } else if info.is_ancestor_of_focus {
        "🌿"
    } else {
        "🍃"
    }
}

/// The ANSI colour code for a node glyph, by meaning (§Colour: glyphs carry
/// meaning; colour only adds emphasis). Mirrors the precedence of `glyph_for`.
fn glyph_ansi(info: &CommitInfo) -> &'static str {
    if info.id == ROOT_ID {
        "1" // root: bold
    } else if info.conflict {
        "31" // conflict: red
    } else if info.empty {
        "2" // empty: dim
    } else if info.immutable {
        "34" // immutable: blue
    } else if info.is_focus {
        "1;36" // focus: bold cyan
    } else if info.is_ancestor_of_focus {
        "32" // ancestor of focus: green
    } else {
        "37" // other commit: light grey
    }
}

fn size_bar(size: Option<usize>, pal: &Palette) -> String {
    match size {
        None | Some(0) => String::new(),
        Some(n) => {
            // one glyph per threshold 1, 10, 50, 200, 1000 (§7.11 column 5)
            let (bar, level) = if n >= SIZE_BAR_TOP {
                ("▇", 4)
            } else if n >= 200 {
                ("▅", 3)
            } else if n >= 50 {
                ("▃", 2)
            } else if n >= 10 {
                ("▂", 1)
            } else {
                ("▁", 0)
            };
            pal.accent(level, bar)
        }
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::BlobVal;
    use std::collections::HashMap;

    /// Lines deleted plus inserted by a shortest diff of `a` to `b`, uncapped
    /// and found without anything `changed_lines` uses: both lengths less
    /// twice a longest common subsequence, which Hyyrö's bit-parallel LCS
    /// finds a machine word of `a` at a time.
    fn exact(a: &[String], b: &[String]) -> usize {
        let words = a.len().div_ceil(64);
        // per line of `a`, a bit at each place it occurs in `a`
        let mut masks: HashMap<&str, Vec<u64>> = HashMap::new();
        for (i, l) in a.iter().enumerate() {
            masks.entry(l.as_str()).or_insert_with(|| vec![0; words])[i / 64] |= 1 << (i % 64);
        }
        let absent = vec![0; words];
        // after each line of `b`, the zero bits of `v` up to a place in `a`
        // count a longest common subsequence of `a` up to there and `b` so
        // far; the bits past the end of `a` stay ones
        let mut v = vec![u64::MAX; words];
        for l in b {
            let m = masks.get(l.as_str()).unwrap_or(&absent);
            let mut carry = false;
            for (v, m) in v.iter_mut().zip(m) {
                let (sum, c1) = v.overflowing_add(*v & m);
                let (sum, c2) = sum.overflowing_add(carry as u64);
                carry = c1 || c2;
                *v = sum | (*v & !m);
            }
        }
        let common: usize = v.iter().map(|w| w.count_zeros() as usize).sum();
        a.len() + b.len() - 2 * common
    }

    /// `changed_lines` from `a` to `b` and back is the exact count, or the
    /// cap if that is less: under the size bar's cap, caps on either side of
    /// the first search's 64 rounds, `cap` if given, and the exact count plus
    /// one, which leaves no room: a bound or a search stopped short that
    /// overcounts at all returns the cap there.
    fn check(a: &[String], b: &[String], cap: Option<usize>) {
        let (va, vb) = (BlobVal::text_blob(&a.concat()), BlobVal::text_blob(&b.concat()));
        let want = exact(a, b);
        assert_eq!(want, exact(b, a));
        let mut caps = vec![1, 64, 65, want + 1, SIZE_BAR_TOP];
        caps.extend(cap);
        caps.retain(|c| *c <= SIZE_BAR_TOP);
        for cap in caps {
            for (from, to) in [(&va, &vb), (&vb, &va)] {
                let got = changed_lines(Some(from), Some(to), cap).unwrap();
                assert_eq!(got, want.min(cap), "cap {}, {} and {} lines", cap, a.len(), b.len());
            }
        }
    }

    /// xorshift64*: the same numbers on every run
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as usize % n
        }
    }

    #[test]
    fn changed_lines_is_exact_up_to_its_cap() {
        // the size bar is drawn from this count (§7.11 column 5), and no
        // shortcut may change it, whatever the machine or the content.
        // A few lines repeating, disjoint adjacent pairs of them swapped, or
        // one of each pair replaced by the other (two lines each): no line
        // occurs once on either side, and every diagonal the period divides
        // runs along equal lines from one change to the next. Replacing
        // changes how many times each line occurs; swapping does not
        for period in [2, 3] {
            let lines: Vec<String> = (0..3000).map(|i| format!("l{}\n", i % period)).collect();
            for changes in [1, 33, 499, 501] {
                let (mut swapped, mut replaced) = (lines.clone(), lines.clone());
                let step = 2990 / changes;
                for k in 0..changes {
                    let at = 3 + step * k;
                    swapped.swap(at, at + 1);
                    replaced[at] = lines[at + 1].clone();
                }
                check(&lines, &swapped, None);
                check(&lines, &replaced, None);
            }
        }
        // runs of one line swapped: a shortest diff keeps the longer run
        let runs = |x: &str, n: usize, y: &str, m: usize| -> Vec<String> {
            let run = |l: &str, n| std::iter::repeat_n(format!("{}\n", l), n);
            run(x, n).chain(run(y, m)).collect()
        };
        check(&runs("a", 499, "b", 501), &runs("b", 501, "a", 500), None);
        check(&runs("a", 500, "b", 501), &runs("b", 501, "a", 500), None);
        // distinct lines, blocks moved past as many others: two lines per line
        // moved, where only the lines that occur once on each side bound it
        let lines: Vec<String> = (0..4000).map(|i| format!("line {}\n", i)).collect();
        for lens in [&[90; 5][..], &[100, 100, 100, 100, 99], &[100; 5], &[250, 251], &[600]] {
            let mut moved = lines.clone();
            for (j, len) in lens.iter().enumerate() {
                let at = 100 + 700 * j;
                let block: Vec<String> = moved.drain(at..at + len).collect();
                moved.splice(at + len..at + len, block);
            }
            check(&lines, &moved, None);
        }
        // random edits of random files, of two lines up to mostly distinct
        // ones: lines inserted, removed, replaced, and blocks moved
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..40 {
            let distinct = [2, 5, 50, 1 << 20][rng.below(4)];
            let line = |rng: &mut Rng| format!("{}\n", rng.below(distinct));
            let a: Vec<String> = (0..rng.below(800)).map(|_| line(&mut rng)).collect();
            let mut b = a.clone();
            for _ in 0..rng.below(300) {
                let at = rng.below(b.len() + 1);
                match rng.below(4) {
                    0 => b.insert(at, line(&mut rng)),
                    1 if at < b.len() => {
                        b.remove(at);
                    }
                    2 if at < b.len() => b[at] = line(&mut rng),
                    _ => {
                        let end = (at + rng.below(40)).min(b.len());
                        let block: Vec<String> = b.drain(at..end).collect();
                        let to = rng.below(b.len() + 1);
                        b.splice(to..to, block);
                    }
                }
            }
            check(&a, &b, Some(1 + rng.below(SIZE_BAR_TOP)));
        }
    }
}

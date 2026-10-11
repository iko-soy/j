//! Read-only command discovery, built from the user's unevaluated config.

use j::ast::{Item, TypeExpr};
use j::config;
use j::lex::{lex, Tok};
use j::parse::parse_config;
use std::collections::BTreeMap;
use std::rc::Rc;

pub const OVERVIEW: &str = r#"j — a functional CLI for Jujutsu repositories

Usage:
  j EXPRESSION…                 Evaluate an expression
  printf '%s\n' 'tree' | j      Read an expression from stdin
  j help [NAME]                 Explain a command or config name
  j commands [QUERY]            Find everyday commands by task
  j commands all                Browse the complete vocabulary
  j help config                Set your identity or repair configuration

Start a repository:
  j setup 'Ada Lovelace' ada@example.org  Create config with your identity
  j init                        Create a repository in the current directory
  j clone URL [DIR]             Clone a repository

Inspect (default config):
  j tree                        Show the history and current focus
  j status                      Show the focused commit and changed paths
  j review                      Show changes (unified diff if difft is absent)
  j log                         List commits

Edit (default config):
  j save                        Record current file edits
  j 'describe "add a feature"'   Set the focused commit's message
  j new                         Start an empty child and move to it
  j prev                        Move to the parent commit
  j 'new . goto trunk'          Start a new commit on the main line
  j squash                      Fold the focus into its parent
  j 'rebase trunk'              Move the focused subtree onto the main line

Preview and recover:
  j 'preview squash'            Check and preview a squash without saving
  j undo                        Undo the last operation
  j redo                        Reapply an undone operation
  j ops                         Show the operation log

Sync:
  j remote URL                  Set the origin remote
  j fetch                       Fetch from origin
  j 'push (label "feature" here)'  Publish the focus as a remote bookmark
  j push relabel                Update existing remote bookmarks

Push sends recorded commits. After editing files, run `j save` to record them
before pushing; `j status` and other inspections do not save those edits.

Expressions compose right to left: `describe "wip" . new` runs new first.
A function result receives the current repository. A Repo result saves the
edit; other results print. Quote the whole expression with shell single
quotes to preserve text's double quotes and operators such as *, <, and >.

Your config.j defines the vocabulary: edits (Repo -> Repo), inspections,
revsets, filesets, types, and custom names. Search with `j commands rebase`
or `j commands Edit`; use `j help squash` or `j help commands` for details.
"#;

pub const CONFIG_HELP: &str = r#"Configuration and identity

For a new installation, create the complete config with your identity:

  j setup 'Ada Lovelace' ada@example.org

Use your own name and email. Then run `j init` or `j clone URL`.
To create the complete default and edit identity yourself, run `j setup`.
Setup never replaces an existing config.

If the file already exists, edit its existing `user` definition:

  user = { name = "Ada Lovelace", email = "ada@example.org" }

Keep the rest of the file: it defines your commands. The example above is
one definition, not a complete config. Set both fields before saving changes.
The shipped "Your Name" / "you@example.com" placeholders cannot author commits.
An existing configuration is never overwritten or upgraded automatically.

If configuration is broken, repair the reported line in this file. General
help and reserved-command help remain available without loading config.
`j commands` checks the vocabulary without evaluating it or changing a repo.

Configuration lives at $XDG_CONFIG_HOME/j/config.j when XDG_CONFIG_HOME is
set, otherwise at ~/.config/j/config.j. `j setup` creates an editable default;
help never creates or changes files.
"#;

struct Reserved {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    detail: &'static str,
}

const RESERVED: &[Reserved] = &[
    Reserved {
        name: "clone",
        usage: "j clone URL [DIR]",
        summary: "Clone a repository.",
        detail: "Clone URL into DIR, or into a directory named after the repository.\nFor first-time setup, run `j setup NAME EMAIL`. Shell-quote paths containing spaces.\nCloning does not change your shell's directory; enter the new folder afterward.\n\nExamples:\n  j clone https://example.com/team/project.git project\n  cd project\n  j status\n\n  j clone '/path/to/source repo' 'my project'\n  cd 'my project'\n",
    },
    Reserved {
        name: "commands",
        usage: "j commands [QUERY]",
        summary: "Find everyday commands by task, or browse the full vocabulary.",
        detail: "Without a query, show common tasks and a few custom definitions.\n`j commands all` lists every configured definition, builtin, operator, and type.\nSearch by task (save, record, commit, branch, checkout), name, signature, or\nsource comments. Search ignores case and puts direct matches and everyday\ncommands first. Use `j help NAME` for examples and types.\n\nThe config is parsed and validated without evaluating definitions or opening\na repository. No config file is created; when absent, defaults are shown.\n\nExamples:\n  j commands\n  j commands save\n  j commands rebase\n  j commands Edit\n  j commands all\n",
    },
    Reserved {
        name: "fetch",
        usage: "j fetch",
        summary: "Fetch commits and bookmarks from origin.",
        detail: "Fetch from the origin remote and import its commits and bookmarks.\nSet the remote with `j remote URL`.\n\nExample:\n  j fetch\n",
    },
    Reserved {
        name: "help",
        usage: "j help [NAME]",
        summary: "Show getting-started help or explain a name.",
        detail: "With no NAME, show common commands and the expression model. With NAME,\nshow CLI usage or a config name's signature, documentation, and source line.\nConfig help reads declarations without evaluating them or opening a repository.\n\nExamples:\n  j help\n  j help push\n  j help squash\n  j help Edit\n  j help '(.)'\n\nUse `j commands QUERY` to find names by purpose or signature.\n",
    },
    Reserved {
        name: "init",
        usage: "j init",
        summary: "Create a repository in the current directory.",
        detail: "Create a colocated .jj/.git repository and a working-copy commit. An\nexisting .git repository is adopted. On a new installation, first run\n`j setup NAME EMAIL`; use `j help config` to update an existing identity.\nThe shipped placeholder identity cannot author commits.\n\nExample:\n  j setup 'Ada Lovelace' ada@example.org\n  mkdir project\n  cd project\n  j init\n",
    },
    Reserved {
        name: "ops",
        usage: "j ops",
        summary: "Show the operation log.",
        detail: "Print the operation history, newest first, with the current operation\nmarked by *. Undo and redo operations are labelled. This reads repository\nhistory without loading config.j.\n\nExample:\n  j ops\n\nRelated: j help undo, j help redo\n",
    },
    Reserved {
        name: "push",
        usage: "j push EXPRESSION",
        summary: "Publish or delete bookmarks on origin.",
        detail: "Evaluate EXPRESSION to a list of Push and Drop records, then update origin.\nA function result receives the recorded repository; working-copy changes\nare not snapshotted by push. Unrecorded edits produce a warning before sending.\nRun `j id` to record them, then push again to include them; `j status` does not\nrecord edits. The default config provides label, unlabel, rename, and relabel.\n\nExamples (default config):\n  j 'push (label \"feature\" here)'\n  j push relabel\n  j 'push (unlabel \"feature\")'\n  j 'push (rename \"old\" \"new\")'\n\nThe unlabel example deletes the remote bookmark.\n",
    },
    Reserved {
        name: "redo",
        usage: "j redo",
        summary: "Reapply an undone operation.",
        detail: "Reapply the operation removed by undo. Refuses if there is nothing to redo\nor if restoring the working copy would overwrite unsnapshotted changes.\nRecording new edits with `j id` creates an operation and clears pending redos.\nInspect `j ops` before choosing how to preserve or discard those edits.\n\nExample:\n  j redo\n\nRelated: j help undo, j help ops\n",
    },
    Reserved {
        name: "remote",
        usage: "j remote URL",
        summary: "Set the origin remote URL.",
        detail: "Add origin or replace its URL. Fetch and push use this remote.\n\nExample:\n  j remote https://example.com/team/project.git\n  j fetch\n",
    },
    Reserved {
        name: "setup",
        usage: "j setup [NAME EMAIL]",
        summary: "Create your complete config before starting a repository.",
        detail: "With NAME and EMAIL, create the full default config with your identity.\nWithout arguments, create the default and print the exact definition to edit.\nAn existing config is never overwritten: edit its `user` definition directly.\nSetup does not create or open a repository.\n\nExamples:\n  j setup 'Ada Lovelace' ada@example.org\n  j init\n\n  j setup\n  j help config\n",
    },
    Reserved {
        name: "undo",
        usage: "j undo",
        summary: "Undo the last operation.",
        detail: "Restore the repository state before the last operation. Repeat to walk\nback through history. Refuses if restoring the working copy would overwrite\nunsnapshotted changes. `j id` preserves edits as a new operation: the next\nundo reverses that recording before reaching the earlier operation. Use\n`j ops` to inspect history; `j redo` can restore the undone recording.\nUndo does not reverse changes already sent to a remote.\n\nExample:\n  j undo\n\nRelated: j help redo, j help ops\n",
    },
];

pub fn reserved_help(name: &str) -> Option<String> {
    RESERVED
        .iter()
        .find(|cmd| cmd.name == name)
        .map(|cmd| format!("Usage: {}\n\n{}\n\n{}", cmd.usage, cmd.summary, cmd.detail))
}

#[derive(Clone, Copy)]
enum Kind {
    Definition,
    Builtin,
    Type,
}

struct Entry {
    name: String,
    kind: Kind,
    signature: Option<TypeExpr>,
    line: usize,
    docs: Vec<String>,
}

impl Entry {
    fn declaration(&self) -> String {
        let name = if config::is_reserved_builtin(&self.name)
            && !self.name.chars().next().unwrap_or('_').is_alphabetic()
        {
            format!("({})", self.name)
        } else {
            self.name.clone()
        };
        match (&self.signature, self.kind) {
            (Some(ty), Kind::Type) => format!("{} = {}", name, type_text(ty)),
            (Some(ty), _) => format!("{} : {}", name, type_text(ty)),
            (None, _) => format!("{} (definition; no signature)", name),
        }
    }
}

struct Task {
    group: &'static str,
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
}

const TASKS: &[Task] = &[
    Task { group: "Start", name: "setup", usage: "j setup NAME EMAIL", summary: "Set your name and email." },
    Task { group: "Start", name: "init", usage: "j init", summary: "Create a repository here." },
    Task { group: "Start", name: "clone", usage: "j clone URL [DIR]", summary: "Clone a repository, then cd into it." },
    Task { group: "Inspect", name: "status", usage: "j status", summary: "See your current change and files." },
    Task { group: "Inspect", name: "tree", usage: "j tree", summary: "See history and your current commit." },
    Task { group: "Inspect", name: "review", usage: "j review", summary: "Review changed file contents." },
    Task { group: "Work", name: "save", usage: "j save", summary: "Record current file edits." },
    Task { group: "Work", name: "id", usage: "j id", summary: "Record current file edits." },
    Task { group: "Work", name: "describe", usage: "j 'describe \"message\"'", summary: "Name the current commit." },
    Task { group: "Work", name: "new", usage: "j new", summary: "Start the next commit before editing." },
    Task { group: "Work", name: "preview", usage: "j 'preview squash'", summary: "Check an edit without saving it." },
    Task { group: "Work", name: "prev", usage: "j prev", summary: "Move to the parent commit." },
    Task { group: "Share", name: "remote", usage: "j remote URL", summary: "Set your remote repository." },
    Task { group: "Share", name: "fetch", usage: "j fetch", summary: "Get remote commits and bookmarks." },
    Task { group: "Share", name: "push", usage: "j help push", summary: "Learn how to publish a bookmark." },
    Task { group: "Recover", name: "undo", usage: "j undo", summary: "Undo the last operation." },
    Task { group: "Recover", name: "redo", usage: "j redo", summary: "Reapply an undone operation." },
    Task { group: "Recover", name: "ops", usage: "j ops", summary: "Inspect the operation history." },
];

fn everyday_list(entries: &[Entry]) -> String {
    let mut out = String::from("Everyday commands:\n");
    let reference = config::load_config(include_str!("../config.j")).expect("embedded config is valid");
    let mut group = "";
    for task in TASKS {
        let entry = entries.iter().find(|entry| entry.name == task.name);
        let reserved = RESERVED.iter().any(|command| command.name == task.name);
        if !reserved && entry.is_none() {
            continue;
        }
        if task.name == "id" && entries.iter().any(|entry| entry.name == "save") {
            continue;
        }
        if task.group != group {
            out.push_str(&format!("\n{}:\n", task.group));
            group = task.group;
        }
        let custom_summary;
        let summary = match entry {
            Some(entry) if !entry.docs.is_empty() => entry.docs[0].as_str(),
            Some(entry) if entry.signature.is_none() && entry.docs.is_empty() => {
                custom_summary = format!("Custom definition; see `j help {}`.", entry.name);
                custom_summary.as_str()
            }
            _ => task.summary,
        };
        let changed_signature = entry.is_some_and(|entry| entry.signature.as_ref() != reference.sigs.get(task.name));
        let usage = if changed_signature || (task.name == "preview" && !entries.iter().any(|entry| entry.name == "squash")) {
            format!("j help {}", task.name)
        } else {
            task.usage.to_string()
        };
        out.push_str(&format!("  {:<29} {}\n", usage, truncate(summary, 47)));
    }
    // Surface a few additions without letting a large custom vocabulary turn
    // the starting guide into the complete catalog again.
    let custom: Vec<_> = entries.iter().filter(|entry| {
        matches!(entry.kind, Kind::Definition)
            && !reference.defs.iter().any(|(name, _)| name == &entry.name)
    }).collect();
    if !custom.is_empty() {
        out.push_str("\nYour custom definitions:\n");
        for entry in custom.iter().take(6) {
            let docs = entry.docs.first().map(String::as_str).unwrap_or("Use j help for details.");
            out.push_str(&format!("  {:<29} {}\n", entry.name, truncate(docs, 55)));
        }
        if custom.len() > 6 {
            out.push_str("  More custom definitions are listed by `j commands all`.\n");
        }
    }
    out.push_str("\nUse `j help NAME` for examples. Search by task: `j commands save`.\nUse `j commands all` for every definition, builtin, operator, and type.\n");
    out
}

fn intent_names(query: &str) -> &'static [&'static str] {
    match query {
        "save" | "record" | "snapshot" | "save changes" | "record changes" => &["save", "id"],
        "commit" | "git commit" => &["describe", "new", "save", "id"],
        "branch" | "git branch" => &["new", "label", "push", "goto"],
        "checkout" | "switch" | "switch branch" | "git checkout" => &["goto", "prev", "next"],
        "diff" | "changes" | "git diff" => &["review", "status", "changed", "diff"],
        _ => &[],
    }
}

pub fn command_list(src: &str, query: &str) -> Result<String, String> {
    let entries = entries(src)?;
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(everyday_list(&entries));
    }
    let all = query == "all";
    let intents = intent_names(&query);
    let matches = |text: &str| all || text.to_lowercase().contains(&query);
    let mut results = Vec::new();
    for cmd in RESERVED {
        let intent = intents.iter().position(|name| *name == cmd.name);
        if matches(&format!("{} {} {}", cmd.usage, cmd.summary, cmd.detail)) || intent.is_some() {
            let rank = if all || query == cmd.name { 0 } else { intent.map(|i| i + 1).unwrap_or(40) };
            results.push((rank, cmd.name.to_string(), format!("  {:<24} {}\n", cmd.usage, cmd.summary)));
        }
    }
    for entry in &entries {
        let declaration = entry.declaration();
        let intent = intents.iter().position(|name| *name == entry.name);
        if !matches(&format!("{} {}", declaration, entry.docs.join(" "))) && intent.is_none() {
            continue;
        }
        let everyday = TASKS.iter().any(|task| task.name == entry.name);
        let exact = query == entry.name.to_lowercase() && (!matches!(entry.kind, Kind::Type) || intents.is_empty());
        let rank = if all { 1 } else if exact { 0 }
            else { intent.map(|i| i + 1).unwrap_or(if everyday { 20 } else { 30 }) };
        let mut description = format!("  {}\n", declaration);
        if let Some(doc) = entry.docs.first() {
            description.push_str(&format!("    {}\n", truncate(doc, 110)));
        }
        results.push((rank, entry.name.clone(), description));
    }
    results.sort_by(|a, b| (&a.0, a.1.to_lowercase(), &a.1).cmp(&(&b.0, b.1.to_lowercase(), &b.1)));
    let mut out = String::new();
    if matches!(query.as_str(), "stash" | "git stash") && !entries.iter().any(|entry| entry.name == "stash") {
        out.push_str("There is no direct stash command in this config.\n");
        let save = if entries.iter().any(|entry| entry.name == "save") { Some("save") }
            else if entries.iter().any(|entry| entry.name == "id") { Some("id") } else { None };
        if let Some(save) = save {
            out.push_str(&format!("`j {}` records file edits in the current commit.\n", save));
        }
        if entries.iter().any(|entry| entry.name == "new") {
            out.push_str("`j new` starts a child commit; the recorded work remains in its parent.\n");
        }
        out.push_str("\n");
    }
    if results.is_empty() {
        out.push_str("No matching commands or config names. Try `j commands` for common tasks or `j commands all` for every name.\n");
    } else {
        out.push_str(if all { "All commands and config vocabulary:\n" } else { "Matching commands and config names:\n" });
        for (_, _, description) in results {
            out.push_str(&description);
        }
        out.push_str("\nUse `j help NAME` for examples and details; `j commands QUERY` to search.\n");
    }
    Ok(out)
}

pub fn command_help(src: &str, name: &str) -> Result<Option<String>, String> {
    let name = name.trim();
    let name = name
        .strip_prefix('(')
        .and_then(|n| n.strip_suffix(')'))
        .unwrap_or(name)
        .trim();
    let entry = entries(src)?.into_iter().find(|entry| entry.name == name);
    Ok(entry.map(|entry| {
        let kind = match entry.kind {
            Kind::Definition => "Config definition",
            Kind::Builtin => "Builtin declared in config",
            Kind::Type => "Type declared in config",
        };
        let mut out = String::new();
        if !entry.docs.is_empty() {
            for doc in entry.docs.iter().take(24) {
                out.push_str(doc);
                out.push('\n');
            }
            if entry.docs.len() > 24 {
                out.push_str("… See the source for the remaining comments.\n");
            }
            out.push('\n');
        }
        out.push_str(&format!("{}\n\n{}\nSource: line {}\n", entry.declaration(), kind, entry.line));
        out
    }))
}

fn entries(src: &str) -> Result<Vec<Entry>, String> {
    let cfg = config::load_config(src).map_err(|err| err.message())?;
    let items = parse_config(src, Rc::new(config::reserved_set()))
        .map_err(|err| format!("line {}: {}", err.line, err.msg))?;
    let tokens = lex(src).map_err(|err| format!("line {}: {}", err.line, err.msg))?;
    let starts: Vec<_> = tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.col == 1 && !matches!(token.tok, Tok::Newline | Tok::Eof))
        .map(|(index, _)| index)
        .collect();
    let comments = comments(src, &tokens);
    let mut entries = BTreeMap::<String, Entry>::new();
    for (index, item) in items.into_iter().enumerate() {
        let first = &tokens[starts[index]];
        let end = starts.get(index + 1).copied().unwrap_or(tokens.len());
        let last = tokens[starts[index]..end]
            .iter()
            .rev()
            .find(|token| !matches!(token.tok, Tok::Newline | Tok::Eof))
            .unwrap();
        let next_start = starts
            .get(index + 1)
            .map(|&i| tokens[i].start)
            .unwrap_or(src.len());
        let docs = item_docs(src, first.line, last.end, next_start, &comments);
        let (name, signature, kind) = match item {
            Item::TypeDecl(name, ty) => (name, Some(ty), Kind::Type),
            Item::Signature(name, ty, _) => {
                let kind = if cfg.builtin_names.contains(&name) {
                    Kind::Builtin
                } else {
                    Kind::Definition
                };
                (name, Some(ty), kind)
            }
            Item::Definition(name, _, _) => (name, None, Kind::Definition),
        };
        let entry = entries.entry(name.clone()).or_insert(Entry {
            name,
            signature,
            kind,
            line: first.line,
            docs: Vec::new(),
        });
        for doc in docs {
            if !entry.docs.contains(&doc) {
                entry.docs.push(doc);
            }
        }
    }
    let mut entries: Vec<_> = entries.into_values().collect();
    entries.sort_by_cached_key(|entry| (entry.name.to_lowercase(), entry.name.clone()));
    Ok(entries)
}

fn type_text(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Con(name) => name.clone(),
        TypeExpr::List(item) => format!("[{}]", type_text(item)),
        TypeExpr::Record(fields) => format!(
            "{{ {} }}",
            fields
                .iter()
                .map(|(name, ty)| format!("{} : {}", name, type_text(ty)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeExpr::Fun(arg, result) => {
            let arg_text = type_text(arg);
            let arg_text = if matches!(arg.as_ref(), TypeExpr::Fun(_, _)) {
                format!("({})", arg_text)
            } else {
                arg_text
            };
            format!("{} -> {}", arg_text, type_text(result))
        }
    }
}

fn truncate(text: &str, length: usize) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(length).collect();
    if chars.next().is_some() {
        format!("{}…", prefix)
    } else {
        prefix
    }
}

struct Comment {
    start: usize,
    first_line: usize,
    last_line: usize,
    standalone: bool,
    continuation: bool,
    text: Vec<String>,
}

/// Comments occupy gaps between lexer tokens, so text such as "-- not a doc"
/// inside string literals cannot be mistaken for documentation.
fn comments(src: &str, tokens: &[j::lex::SpTok]) -> Vec<Comment> {
    let mut comments = Vec::<Comment>::new();
    let mut cursor = 0;
    for token in tokens
        .iter()
        .filter(|token| !matches!(token.tok, Tok::Newline))
    {
        let mut rest = &src[cursor..token.start];
        while !rest.trim_start().is_empty() {
            rest = rest.trim_start();
            let start = token.start - rest.len();
            let (raw, consumed) = if let Some(body) = rest.strip_prefix("--") {
                let length = body.find('\n').unwrap_or(body.len());
                (&body[..length], length + 2)
            } else if rest.starts_with("{-") {
                let mut depth = 1;
                let mut offset = 2;
                while depth > 0 {
                    if rest[offset..].starts_with("{-") {
                        depth += 1;
                        offset += 2;
                    } else if rest[offset..].starts_with("-}") {
                        depth -= 1;
                        offset += 2;
                    } else {
                        offset += rest[offset..].chars().next().unwrap().len_utf8();
                    }
                }
                (&rest[2..offset - 2], offset)
            } else {
                break;
            };
            let first_line = src[..start].bytes().filter(|&b| b == b'\n').count() + 1;
            let last_line = first_line + rest[..consumed].bytes().filter(|&b| b == b'\n').count();
            let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
            let standalone = src[line_start..start].trim().is_empty();
            let continuation = standalone
                && start > line_start
                && comments.last().is_some_and(|previous| {
                    previous.last_line + 1 == first_line
                        && (!previous.standalone || previous.continuation)
                });
            comments.push(Comment {
                start,
                first_line,
                last_line,
                standalone,
                continuation,
                text: raw.lines().map(|line| line.trim().to_string()).collect(),
            });
            rest = &rest[consumed..];
        }
        cursor = token.end;
    }
    comments
}

fn item_docs(
    src: &str,
    first_line: usize,
    end: usize,
    next_start: usize,
    comments: &[Comment],
) -> Vec<String> {
    let mut preceding = Vec::new();
    let mut line = first_line.saturating_sub(1);
    while let Some(comment) = comments
        .iter()
        .rev()
        .find(|comment| comment.last_line == line && comment.standalone && !comment.continuation)
    {
        if comment
            .text
            .iter()
            .all(|text| text.chars().all(|c| "-=_ ".contains(c)))
        {
            break;
        }
        preceding.push(comment);
        line = comment.first_line.saturating_sub(1);
    }
    preceding.reverse();
    let last_line = src[..end].bytes().filter(|&b| b == b'\n').count() + 1;
    let inline = comments.iter().filter(|comment| {
        comment.first_line >= first_line
            && comment.start < next_start
            && (comment.first_line <= last_line || comment.continuation)
    });
    preceding
        .into_iter()
        .chain(inline)
        .flat_map(|comment| comment.text.iter())
        .filter(|text| !text.is_empty() && !text.chars().all(|c| "-=_ ".contains(c)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "user = {}\nimmutable = []\ntree = 0\nlabelled = 0\n";

    #[test]
    fn lists_custom_names_types_and_docs_without_evaluation() {
        let src = format!("{BASE}-- A custom greeting.\nhello = crash \"not evaluated\"\nAlias = Text -- a useful type\n");
        let list = command_list(&src, "CUSTOM GREETING").unwrap();
        assert!(list.contains("hello (definition; no signature)"));
        assert!(!list.contains("immutable"));
        let help = command_help(&src, "Alias").unwrap().unwrap();
        assert!(help.contains("Alias = Text"));
        assert!(help.contains("a useful type"));
        assert!(help.contains("Source: line 7"));
    }

    #[test]
    fn signatures_keep_function_argument_parentheses() {
        let src = format!("{BASE}(.) : (b -> c) -> (a -> b) -> a -> c -- compose\n");
        let help = command_help(&src, "(.)").unwrap().unwrap();
        assert!(help.contains("(.) : (b -> c) -> (a -> b) -> a -> c"));
        assert!(help.contains("Builtin declared in config"));
        assert!(help.contains("compose"));
    }

    #[test]
    fn comments_handle_strings_blocks_and_multiline_types() {
        let src = format!("{BASE}message = \"-- not a doc\"\n\n{{- Useful\n   block docs. -}}\nShape = {{ value : Text\n        , count : Int }} -- shape details\n                         -- continued details\n\n-- Different docs.\nother = 1\n");
        let message = command_help(&src, "message").unwrap().unwrap();
        assert!(!message.contains("not a doc"));
        let shape = command_help(&src, "Shape").unwrap().unwrap();
        assert!(shape.contains("Useful\nblock docs."));
        assert!(shape.contains("shape details\ncontinued details"));
        assert!(!shape.contains("Different docs"));
        assert!(shape.contains("Source: line 9"));
    }

    #[test]
    fn inline_continuations_belong_to_the_preceding_declaration() {
        let src = format!("{BASE}first = 1 -- first comment\n          -- first continuation\nsecond = 2\n\n  -- third comment\nthird = 3\n");
        let first = command_help(&src, "first").unwrap().unwrap();
        assert!(first.contains("first comment\nfirst continuation"));
        let second = command_help(&src, "second").unwrap().unwrap();
        assert!(!second.contains("first continuation"));
        assert!(!second.contains("third comment"));
        assert!(command_help(&src, "third")
            .unwrap()
            .unwrap()
            .contains("third comment"));
    }

    #[test]
    fn validates_config_and_does_not_resolve_ids() {
        assert!(command_list("broken =", "").is_err());
        assert!(command_help("x = 1", "x")
            .unwrap_err()
            .contains("must define"));
        let src = format!("{BASE}revision = @zzzz\n");
        assert!(command_help(&src, "revision").unwrap().is_some());
        assert!(command_help(&src, "missing").unwrap().is_none());
        assert!(command_list(&src, "no-such-query")
            .unwrap()
            .contains("No matching"));
    }

    #[test]
    fn default_config_documents_real_names() {
        let src = include_str!("../config.j");
        let help = command_help(src, "squash").unwrap().unwrap();
        assert!(help.contains("squash : Edit"));
        assert!(help.contains("Fold the focus entirely into its parent"));
        let list = command_list(src, "edit").unwrap();
        assert!(list.contains("squash : Edit"));
        assert!(list.contains("describe : Text -> Edit"));
        assert!(reserved_help("push")
            .unwrap()
            .contains("j 'push (label \"feature\" here)'"));
    }
}

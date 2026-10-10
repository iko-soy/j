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
  j commands [QUERY]            Search names, signatures, and documentation

Start a repository:
  j init                        Create a repository in the current directory
  j clone URL [DIR]             Clone a repository

Inspect (default config):
  j tree                        Show the history and current focus
  j status                      Show the focused commit and changed paths
  j review                      Show changes with difftastic (difft on PATH)
  j log                         List commits

Edit (default config):
  j 'describe "add a feature"'   Set the focused commit's message
  j new                         Start an empty child and move to it
  j prev                        Move to the parent commit
  j 'new . goto trunk'          Start a new commit on the main line
  j squash                      Fold the focus into its parent
  j 'rebase trunk'              Move the focused subtree onto the main line

Preview and recover:
  j 'tree . squash'             Preview a squash without saving the edit
  j undo                        Undo the last operation
  j redo                        Reapply an undone operation
  j ops                         Show the operation log

Sync:
  j remote URL                  Set the origin remote
  j fetch                       Fetch from origin
  j 'push (label "feature" here)'  Publish the focus as a remote bookmark
  j push relabel                Update existing remote bookmarks

Expressions compose right to left: `describe "wip" . new` runs new first.
A function result receives the current repository. A Repo result saves the
edit; other results print. Quote the whole expression with shell single
quotes to preserve text's double quotes and operators such as *, <, and >.

Your config.j defines the vocabulary: edits (Repo -> Repo), inspections,
revsets, filesets, types, and custom names. Search with `j commands rebase`
or `j commands Edit`; use `j help squash` or `j help commands` for details.
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
        detail: "Clone URL into DIR, or into a directory named after the repository.\n\nExample:\n  j clone https://example.com/team/project.git project\n",
    },
    Reserved {
        name: "commands",
        usage: "j commands [QUERY]",
        summary: "List and search the commands and config vocabulary.",
        detail: "List CLI commands and the names declared or defined in config.j, including\ncustom definitions, builtins, operators, and types. QUERY matches names,\nsignatures, and source comments, ignoring case.\n\nThe config is parsed and validated without evaluating definitions or opening\na repository. No config file is created; when absent, defaults are shown.\n\nExamples:\n  j commands\n  j commands rebase\n  j commands Edit\n  j commands 'remote bookmark'\n  j help squash\n",
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
        detail: "Create a colocated .jj/.git repository and a working-copy commit. An\nexisting .git repository is adopted. Set your identity in config.j's `user`\ndefinition before creating commits.\n\nExample:\n  mkdir project\n  cd project\n  j init\n",
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
        detail: "Evaluate EXPRESSION to a list of Push and Drop records, then update origin.\nA function result receives the recorded repository; working-copy changes\nare not snapshotted by push. The default config provides label, unlabel,\nrename, and relabel helpers.\n\nExamples (default config):\n  j 'push (label \"feature\" here)'\n  j push relabel\n  j 'push (unlabel \"feature\")'\n  j 'push (rename \"old\" \"new\")'\n\nThe unlabel example deletes the remote bookmark.\n",
    },
    Reserved {
        name: "redo",
        usage: "j redo",
        summary: "Reapply an undone operation.",
        detail: "Reapply the operation removed by undo. Refuses if there is nothing to redo\nor if restoring the working copy would overwrite unsnapshotted changes.\n\nExample:\n  j redo\n\nRelated: j help undo, j help ops\n",
    },
    Reserved {
        name: "remote",
        usage: "j remote URL",
        summary: "Set the origin remote URL.",
        detail: "Add origin or replace its URL. Fetch and push use this remote.\n\nExample:\n  j remote https://example.com/team/project.git\n  j fetch\n",
    },
    Reserved {
        name: "undo",
        usage: "j undo",
        summary: "Undo the last operation.",
        detail: "Restore the repository state before the last operation. Repeat to walk\nback through history. Refuses if restoring the working copy would overwrite\nunsnapshotted changes. Undo does not reverse changes already sent to a remote.\n\nExample:\n  j undo\n\nRelated: j help redo, j help ops\n",
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

pub fn command_list(src: &str, query: &str) -> Result<String, String> {
    let entries = entries(src)?;
    let query = query.trim().to_lowercase();
    let matches = |text: &str| text.to_lowercase().contains(&query);
    let mut out = String::new();
    let mut count = 0;
    for cmd in RESERVED {
        if matches(&format!("{} {} {}", cmd.usage, cmd.summary, cmd.detail)) {
            if count == 0 {
                out.push_str("CLI commands:\n");
            }
            out.push_str(&format!("  {:<24} {}\n", cmd.usage, cmd.summary));
            count += 1;
        }
    }
    let mut config_count = 0;
    for entry in entries {
        let declaration = entry.declaration();
        if !matches(&format!("{} {}", declaration, entry.docs.join(" "))) {
            continue;
        }
        if config_count == 0 {
            if count > 0 {
                out.push('\n');
            }
            out.push_str("Config vocabulary (definitions, builtins, operators, and types):\n");
        }
        out.push_str(&format!("  {}\n", declaration));
        if let Some(doc) = entry.docs.first() {
            out.push_str(&format!("    {}\n", truncate(doc, 110)));
        }
        config_count += 1;
    }
    if count + config_count == 0 {
        out.push_str("No matching commands or config names. Try `j commands` to list all names.\n");
    } else {
        out.push_str("\nUse `j help NAME` for details; `j commands QUERY` to narrow the list.\n");
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
        let mut out = format!(
            "{}\n\n{}\nSource: line {}\n",
            entry.declaration(),
            kind,
            entry.line
        );
        if !entry.docs.is_empty() {
            out.push('\n');
            for doc in entry.docs.iter().take(24) {
                out.push_str(doc);
                out.push('\n');
            }
            if entry.docs.len() > 24 {
                out.push_str("… See the source for the remaining comments.\n");
            }
        }
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

//! Explicit first-run configuration, independent of repository discovery.

use j::ast::Item;
use j::config;
use j::eval::Interp;
use j::lex::{lex, Tok};
use j::parse::parse_config;
use j::show::show;
use j::value::Value;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;

const IDENTITY_EXAMPLE: &str = "user = { name = \"Ada Lovelace\", email = \"ada@example.org\" }";

/// `args` contains only the arguments after `setup`. Existing files, including
/// invalid configs and symlinks, are never replaced. A new file is published
/// atomically so concurrent setup commands cannot overwrite one another.
pub fn run(path: &Path, default: &str, args: &[&str]) -> Result<String, (u8, String)> {
    if !matches!(args.len(), 0 | 2) {
        return Err((2, "usage: j setup [NAME EMAIL]; for example: j setup 'Ada Lovelace' ada@example.org; see `j help setup`".into()));
    }
    if path.symlink_metadata().is_ok() {
        return existing(path, args);
    }
    let source = match args {
        [name, email] => with_identity(default, name, email)?,
        [] => default.to_string(),
        _ => unreachable!(),
    };
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| (3, format!("cannot create config directory {}: {}", parent.display(), error)))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| (3, format!("cannot prepare config at {}: {}", path.display(), error)))?;
    temporary.write_all(source.as_bytes())
        .map_err(|error| (3, format!("cannot write config at {}: {}", path.display(), error)))?;
    match temporary.persist_noclobber(path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => return existing(path, args),
        Err(error) => return Err((3, format!("cannot create config at {}: {}", path.display(), error.error))),
    }
    if args.is_empty() {
        Ok(format!(
            "Created the complete default config at {}.\n\nEdit the existing `user` definition in this file, replacing both example values with your identity:\n  {}\nKeep the rest of the file: it defines your commands.\n\nThen run `j init` in your project directory, or `j clone URL`.\n",
            path.display(), IDENTITY_EXAMPLE,
        ))
    } else {
        Ok(format!(
            "Configuration ready at {}.\nIdentity: {}\n\nNext: run `j init` in your project directory, or `j clone URL`.\n",
            path.display(), identity_line(&source)?,
        ))
    }
}

fn existing(path: &Path, args: &[&str]) -> Result<String, (u8, String)> {
    if !args.is_empty() {
        return Err((2, format!(
            "config already exists at {}; it was not changed. Edit its existing `user` definition to change your identity; see `j help config`.",
            path.display(),
        )));
    }
    let problem = match std::fs::read_to_string(path) {
        Ok(source) => match identity_line(&source) {
            Ok(identity) => return Ok(format!(
                "Configuration already exists at {} and is ready.\nIdentity: {}\n\nNext: run `j init` in your project directory, or `j clone URL`.\n",
                path.display(), identity,
            )),
            Err((_, message)) => message,
        },
        Err(error) => format!("cannot read this config: {}", error),
    };
    Ok(format!(
        "Configuration already exists at {}; it was not changed.\nNeeds attention: {}\n\nTo set your identity, edit the existing `user` definition:\n  {}\nKeep the rest of the file and repair the problem above.\nSee `j help config`; then retry `j setup`.\n",
        path.display(), problem, IDENTITY_EXAMPLE,
    ))
}

fn identity_line(source: &str) -> Result<String, (u8, String)> {
    let cfg = config::load_config(source).map_err(|error| (3, error.message()))?;
    j::jj::configured_identity(&cfg)
}

fn with_identity(default: &str, name: &str, email: &str) -> Result<String, (u8, String)> {
    let items = parse_config(default, Rc::new(config::reserved_set()))
        .map_err(|error| (3, format!("cannot prepare default config: {}", error.msg)))?;
    let line = items.iter().find_map(|item| match item {
        Item::Definition(name, _, line) if name == "user" => Some(*line),
        _ => None,
    }).ok_or_else(|| (3, "default config has no `user` definition".into()))?;
    let tokens = lex(default).map_err(|error| (3, error.msg))?;
    let start = tokens.iter().position(|token| token.line == line && token.col == 1)
        .ok_or_else(|| (3, "cannot locate the default `user` definition".into()))?;
    let mut depth = 0usize;
    let mut end = tokens[start].end;
    for token in &tokens[start..] {
        if matches!(token.tok, Tok::Eof)
            || (token.line > line && token.col == 1 && depth == 0 && !matches!(token.tok, Tok::Newline))
        {
            break;
        }
        match token.tok {
            Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
            Tok::RParen | Tok::RBracket | Tok::RBrace => depth = depth.saturating_sub(1),
            Tok::Newline => continue,
            _ => {}
        }
        end = token.end;
    }
    let interp = Interp::dummy();
    let name = show(&interp, &Value::text(name)).map_err(|error| (3, error.msg))?;
    let email = show(&interp, &Value::text(email)).map_err(|error| (3, error.msg))?;
    let definition = format!("user = {{ name = {}, email = {} }}", name, email);
    let mut source = default.to_string();
    source.replace_range(tokens[start].start..end, &definition);
    // Reuse the same identity requirements as init/clone before writing a file.
    identity_line(&source)?;
    Ok(source)
}

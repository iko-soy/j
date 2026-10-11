/// `eprintln!` for `j`'s own messages (§1.4). A stderr that cannot take the
/// line (its reader has exited, as in `j … 2>&1 | head`, or the device is
/// full) loses it; `eprintln!` would panic instead, and the run would end
/// with the runtime's status 101 rather than the one it reports.
#[macro_export]
macro_rules! report {
    ($($arg:tt)*) => {{
        $crate::terminal::report(format_args!($($arg)*));
    }};
}

pub mod ast;
pub mod builtins;
pub mod config;
pub mod diagnostics;
pub mod domain;
pub mod eval;
pub mod lex;
mod lookup;
pub mod parse;
pub mod render;
pub mod repo;
pub mod shape;
pub mod show;
pub mod terminal;
pub mod value;

pub mod jj;

//! Interactive presentation. Rendered values stay free of cursor controls;
//! motion belongs only to the CLI's final output and temporary status line.

use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

static ACTIVE_PROGRESS: Mutex<Option<Weak<ProgressWorker>>> = Mutex::new(None);
static EXTERNAL_STDERR: AtomicBool = AtomicBool::new(false);

fn stop_progress() {
    let worker = ACTIVE_PROGRESS.lock().unwrap().as_ref().and_then(Weak::upgrade);
    if let Some(worker) = worker {
        worker.finish();
    }
}

/// A subprocess may write an unterminated diagnostic. Clear our status before
/// it starts, and leave stderr alone for the remainder of this run.
pub fn external_stderr() {
    EXTERNAL_STDERR.store(true, Ordering::Relaxed);
    stop_progress();
}

fn nonempty_env(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn capable_terminal() -> bool {
    !nonempty_env("NO_COLOR") && std::env::var("TERM").as_deref() != Ok("dumb")
}

fn interactive() -> bool {
    io::stdout().is_terminal()
        && io::stderr().is_terminal()
        && !nonempty_env("CI")
}

fn motion_enabled() -> bool {
    interactive()
        && capable_terminal()
        && !matches!(
            std::env::var("J_ANIMATIONS")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str(),
            "never" | "off" | "false" | "0"
        )
}

/// Write a diagnostic without turning a closed stderr into a panic (§1.4).
pub fn report(args: fmt::Arguments<'_>) {
    report_with_color(args, "1;96");
}

pub fn report_error(args: fmt::Arguments<'_>) {
    report_with_color(args, "1;91");
}

fn report_with_color(args: fmt::Arguments<'_>, code: &str) {
    stop_progress();
    let mut out = io::stderr().lock();
    if out.is_terminal() && capable_terminal() {
        let message = args.to_string();
        if let Some(rest) = message.strip_prefix("j: ") {
            let _ = writeln!(out, "\x1b[{code}mj:\x1b[0m {rest}");
        } else {
            let _ = writeln!(out, "\x1b[2m{message}\x1b[0m");
        }
    } else {
        let _ = writeln!(out, "{args}");
    }
}

/// Short confirmation for commands that otherwise have no terminal output.
/// Pipes and unattended runs keep their existing, quiet behavior.
pub fn success(message: &str) {
    stop_progress();
    if interactive() {
        let mut out = io::stderr().lock();
        if capable_terminal() {
            let _ = writeln!(out, "\x1b[1;92m✓\x1b[0m \x1b[1;96mj:\x1b[0m {message}");
        } else {
            let _ = writeln!(out, "j: {message}");
        }
    }
}

/// A delayed spinner, activity bar, and elapsed time. The bar is deliberately
/// indeterminate: elapsed time cannot tell us how much work remains.
/// Stopping wakes the worker immediately, so short operations have no extra
/// delay. Never hide or move the cursor
/// between rows: even an abrupt signal leaves the terminal usable.
struct Progress {
    worker: Option<Arc<ProgressWorker>>,
}

struct ProgressWorker {
    stop: Sender<()>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ProgressWorker {
    fn finish(&self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.thread.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl Progress {
    fn start(label: &'static str) -> Self {
        if !motion_enabled() || EXTERNAL_STDERR.load(Ordering::Relaxed) {
            return Self { worker: None };
        }
        stop_progress();
        let (send, receive) = mpsc::channel();
        let started = Instant::now();
        let worker = thread::Builder::new().name("j-progress".into()).spawn(move || {
            if receive.recv_timeout(Duration::from_millis(140)) != Err(mpsc::RecvTimeoutError::Timeout) {
                return;
            }
            let colors = ["95", "96", "93", "92", "94"];
            let mut frame = 0;
            loop {
                // Fit each new frame to the current terminal width, including
                // after a resize. Leave one column spare at the edge.
                let width = stderr_width().saturating_sub(1);
                let line = progress_line(label, frame, started.elapsed(), width);
                {
                    let mut out = io::stderr().lock();
                    if write!(
                        out,
                        "\r\x1b[2K\x1b[1;{}m{}\x1b[0m",
                        colors[frame % colors.len()],
                        line
                    )
                    .and_then(|()| out.flush())
                    .is_err()
                    {
                        break;
                    }
                }
                frame += 1;
                if receive.recv_timeout(Duration::from_millis(75)) != Err(mpsc::RecvTimeoutError::Timeout) {
                    break;
                }
            }
            let mut out = io::stderr().lock();
            let _ = write!(out, "\r\x1b[2K").and_then(|()| out.flush());
        });
        let worker = worker.ok().map(|worker| Arc::new(ProgressWorker {
            stop: send,
            thread: Mutex::new(Some(worker)),
        }));
        *ACTIVE_PROGRESS.lock().unwrap() = worker.as_ref().map(Arc::downgrade);
        Self { worker }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.finish();
        }
    }
}

/// Finish the temporary line before the caller reports success or failure.
pub fn with_progress<T>(label: &'static str, operation: impl FnOnce() -> T) -> T {
    let _progress = Progress::start(label);
    operation()
}

fn progress_line(label: &str, frame: usize, elapsed: Duration, width: usize) -> String {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let mut line = format!("{} j · {}", FRAMES[frame % FRAMES.len()], label);
    let seconds = elapsed.as_secs();
    let time = if seconds < 60 {
        format!("{}.{}s", seconds, elapsed.subsec_millis() / 100)
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    };
    // Bounce a three-cell pulse across a ten-cell track. It never fills up
    // or suggests a completion percentage for work with no known total.
    let step = frame % 14;
    let start = step.min(14 - step);
    let bar: String = (0..10)
        .map(|cell| if (start..start + 3).contains(&cell) { '━' } else { '·' })
        .collect();
    let details = format!(" [{bar}] {time}");
    if line.width() + details.width() <= width {
        line.push_str(&details);
    } else if line.width() + 1 + time.width() <= width {
        // On small terminals, keep the operation's name ahead of decoration.
        line.push(' ');
        line.push_str(&time);
    }
    let mut used = 0;
    line.chars()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= width
        })
        .collect()
}

fn stderr_width() -> usize {
    // The two streams can be attached to different terminals. Measure the
    // one that receives the progress line, rather than the tree's stdout.
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut size) == 0
            && size.ws_col > 0
        {
            size.ws_col as usize
        } else {
            80
        }
    }
}

/// Reveal colored, multiline terminal output in a few quick batches. Never
/// split a line (and therefore an ANSI style), alter its bytes, or animate
/// plain text. The total pacing is at most 96 ms, even for very large trees.
pub fn write_output(out: &mut impl Write, text: &str) -> io::Result<()> {
    if !motion_enabled() || !text.contains("\x1b[") {
        return out.write_all(text.as_bytes()).and_then(|()| out.flush());
    }
    let lines = text.bytes().filter(|&byte| byte == b'\n').count()
        + usize::from(!text.ends_with('\n'));
    let batch = lines.div_ceil(7).max(1);
    let mut start = 0;
    let mut end = 0;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        end += line.len();
        if (index + 1) % batch == 0 && index + 1 < lines {
            out.write_all(text[start..end].as_bytes())?;
            out.flush()?;
            start = end;
            thread::sleep(Duration::from_millis(16));
        }
    }
    out.write_all(text[start..].as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_fits_terminal_cells_and_keeps_the_operation_visible() {
        let label = "Reading 仓库";
        for width in 0..80 {
            for frame in 0..20 {
                let line = progress_line(label, frame, Duration::from_millis(1234), width);
                assert!(line.width() <= width, "{width} columns: {line:?}");
                if width >= 18 {
                    assert!(line.contains(label), "operation name lost: {line:?}");
                }
            }
        }
        let line = progress_line("Reading repository", 0, Duration::from_millis(1234), 80);
        assert!(line.contains("[━━━·······] 1.2s"), "{line:?}");
        assert!(progress_line("Reading repository", 0, Duration::from_secs(65), 80)
            .ends_with("1m 05s"));
    }
}

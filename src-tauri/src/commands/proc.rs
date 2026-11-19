//! Suppress the console window Windows allocates for each helper subprocess.
//!
//! modkit's binary is a GUI-subsystem executable, so it has no console of its own.
//! When it spawns a *console-subsystem* helper — `reg`, `powershell`, `wad_simulator`,
//! the crack tool — Windows allocates a fresh console for that child and flashes it on
//! screen: one window per process, gone the instant the child exits. License detection
//! alone fires ~a dozen `reg query` processes ([`super::license::detect_license`]), so a
//! single Setup pass strobed a dozen console windows — alarming even to a trusting user.
//!
//! `CREATE_NO_WINDOW` tells the loader not to give the child a console at all, which
//! also suppresses the window. It does not change how the command runs; stdout/stderr are
//! still captured via `output()`. On non-Windows platforms this is a no-op.
//!
//! Apply [`NoWindow::no_window`] to every *helper* subprocess. Do **not** apply it to a
//! process the user is meant to see (e.g. toolchain.rs opening an interactive shell, which
//! deliberately uses `CREATE_NEW_CONSOLE`) or to a child that already sets creation flags
//! (`creation_flags` overwrites, not OR-combines — `DETACHED_PROCESS` already has no console).

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;

/// `CREATE_NO_WINDOW` (winbase.h): run the child without allocating a console, so no
/// console window appears for console-subsystem helpers.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Spawn a helper subprocess without flashing a console window.
pub trait NoWindow {
    /// Suppress the child's console window on Windows; a no-op on other platforms.
    /// Chainable with the other `Command` builder methods.
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for Command {
    #[cfg(target_os = "windows")]
    fn no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(target_os = "windows"))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}

/// Everything a [`run_streaming`] child wrote, and how it ended: what `Command::output()` returns,
/// for a child whose lines were also handed out while it ran.
#[derive(Debug)]
pub struct StreamedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run `cmd` to completion like `Command::output()`, and hand each line it writes to `on_line` as
/// it arrives.
///
/// stdout and stderr are drained concurrently, each on its own thread, so a child that fills one
/// pipe while the other is being read cannot deadlock. Lines from one stream reach `on_line` in the
/// order that stream wrote them; how lines of the two streams interleave is the order they were
/// read. A line is handed over without its `\n` / `\r\n`, decoded lossily; the returned buffers
/// hold the bytes exactly as written, a last line without a newline included.
pub fn run_streaming(
    cmd: &mut Command,
    on_line: &mut dyn FnMut(&str),
) -> std::io::Result<StreamedOutput> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let (tx, rx) = mpsc::channel::<String>();
    let out_reader = spawn_reader(stdout, tx.clone());
    let err_reader = spawn_reader(stderr, tx);
    // Ends once both readers have dropped their senders, at the end of both streams.
    for line in rx {
        on_line(&line);
    }
    // Both readers are joined and the child reaped before a read error is returned.
    let stdout = join_reader(out_reader);
    let stderr = join_reader(err_reader);
    let status = child.wait()?;
    Ok(StreamedOutput {
        status,
        stdout: stdout?,
        stderr: stderr?,
    })
}

/// Read `pipe` to its end on a new thread, sending each line and returning every byte read.
fn spawn_reader<R: Read + Send + 'static>(
    pipe: R,
    tx: mpsc::Sender<String>,
) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut all = Vec::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                return Ok(all);
            }
            all.extend_from_slice(&line);
            let text = line.strip_suffix(b"\n").unwrap_or(&line);
            let text = text.strip_suffix(b"\r").unwrap_or(text);
            // The receiver lives until both readers finish, so a send cannot fail.
            let _ = tx.send(String::from_utf8_lossy(text).into_owned());
        }
    })
}

fn join_reader(handle: thread::JoinHandle<std::io::Result<Vec<u8>>>) -> std::io::Result<Vec<u8>> {
    handle
        .join()
        .map_err(|_| std::io::Error::other("a pipe reader thread panicked"))?
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Lines arrive in the order each stream wrote them, a last line without a newline included,
    /// and the full output and exit status come back as `output()` would return them.
    #[test]
    fn lines_arrive_in_order_with_full_output_and_status() {
        let mut lines = Vec::new();
        let out = run_streaming(
            Command::new("sh").args([
                "-c",
                "echo a; echo b 1>&2; echo c; printf 'd\\r\\n'; printf e; exit 3",
            ]),
            &mut |l| lines.push(l.to_string()),
        )
        .unwrap();
        let from_stdout: Vec<&str> = lines
            .iter()
            .map(String::as_str)
            .filter(|l| *l != "b")
            .collect();
        assert_eq!(from_stdout, ["a", "c", "d", "e"]);
        assert_eq!(lines.iter().filter(|l| *l == "b").count(), 1);
        assert_eq!(lines.len(), 5);
        assert_eq!(out.stdout, b"a\nc\nd\r\ne");
        assert_eq!(out.stderr, b"b\n");
        assert_eq!(out.status.code(), Some(3));
    }

    /// Far more than a pipe buffer on both streams at once: draining one stream while the other
    /// is left full would hang here.
    #[test]
    fn both_streams_full_at_once_do_not_deadlock() {
        let mut count = 0usize;
        let out = run_streaming(
            Command::new("sh").args([
                "-c",
                "i=0; while [ $i -lt 20000 ]; do echo out$i; echo err$i 1>&2; i=$((i+1)); done",
            ]),
            &mut |_| count += 1,
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(count, 40_000);
        assert!(out.stdout.len() > 64 * 1024 && out.stderr.len() > 64 * 1024);
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(stdout.starts_with("out0\nout1\n") && stdout.ends_with("out19999\n"));
    }
}

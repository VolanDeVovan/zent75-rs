//! Control socket and script runner. Both turn lines into [`Request`]s for the real-time loop.
//!
//! Socket protocol: one command per line; the reply is the command's output (zero or more
//! lines) followed by a line `ok`, or a single line `err <message>`. Blank lines and `#`
//! comments get no reply.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use crate::command::{self, Command};
use crate::sim::Request;

/// Bind the control socket, replacing a stale socket file but never a live simulator.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("{} is in use by another simulator", path.display())));
        }
        std::fs::remove_file(path)?;
    }
    UnixListener::bind(path)
}

/// Accept connections forever on a background thread.
pub fn spawn_server(listener: UnixListener, tx: Sender<Request>) {
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            thread::spawn(move || {
                let _ = serve_conn(stream, tx);
            });
        }
    });
}

/// Run one command line through the loop; `None` for blank lines/comments.
/// Returns `(reply text, quit?)`.
pub fn execute(line: &str, tx: &Sender<Request>) -> Option<(Result<String, String>, bool)> {
    let cmd = match command::parse(line) {
        Ok(None) => return None,
        Ok(Some(c)) => c,
        Err(e) => return Some((Err(e), false)),
    };
    if let Command::Sleep(ms) = cmd {
        thread::sleep(Duration::from_millis(ms as u64));
        return Some((Ok(String::new()), false));
    }
    let quit = cmd == Command::Quit;
    let (rtx, rrx) = mpsc::channel();
    if tx.send(Request { cmd, reply: rtx }).is_err() {
        return Some((Err("simulator is shutting down".into()), true));
    }
    let r = rrx.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| Err("no reply from the simulator loop".into()));
    Some((r, quit))
}

fn format_reply(r: &Result<String, String>) -> String {
    match r {
        Ok(body) if body.is_empty() => "ok\n".to_string(),
        Ok(body) => format!("{body}\nok\n"),
        Err(e) => format!("err {}\n", e.replace('\n', " ")),
    }
}

fn serve_conn(stream: UnixStream, tx: Sender<Request>) -> io::Result<()> {
    let mut w = stream.try_clone()?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if let Some((r, quit)) = execute(&line, &tx) {
            w.write_all(format_reply(&r).as_bytes())?;
            if quit {
                break;
            }
        }
    }
    Ok(())
}

/// Run a script: echo each command and its reply to `out`. Stops at the first error or `quit`.
/// Returns `Ok(true)` if the script ended with `quit`.
pub fn run_script(text: &str, tx: &Sender<Request>, out: &mut impl Write) -> Result<bool, String> {
    for (no, line) in text.lines().enumerate() {
        let Some((r, quit)) = execute(line, tx) else { continue };
        let _ = writeln!(out, "> {}", line.trim());
        let _ = out.write_all(format_reply(&r).as_bytes());
        if let Err(e) = r {
            return Err(format!("line {}: {e}", no + 1));
        }
        if quit {
            return Ok(true);
        }
    }
    Ok(false)
}

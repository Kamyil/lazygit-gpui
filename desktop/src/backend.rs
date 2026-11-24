//! Runs lazygit in desktop mode as a subprocess and exchanges protocol
//! messages with it on background threads.

use crate::protocol::{Incoming, Outgoing};
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use std::{
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{self as sync, Receiver, Sender},
    thread,
};

pub enum BackendEvent {
    Message(Incoming),
    Closed,
    Error(String),
}

pub struct Backend {
    input: Sender<Outgoing>,
}

impl Backend {
    pub fn send(&self, message: Outgoing) {
        // If lazygit is gone, the event stream reports it.
        let _ = self.input.send(message);
    }
}

pub fn launch(bin: PathBuf, repo: &Path) -> Result<(Backend, UnboundedReceiver<BackendEvent>), String> {
    let result = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(repo)
        .output()
        .map_err(|e| format!("Run git: {e}"))?;
    if !result.status.success() || result.stdout != b"true\n" {
        return Err(format!("Not a Git worktree: {}", repo.display()));
    }

    let mut child = Command::new(bin)
        .current_dir(repo)
        .env("LAZYGIT_GPUI", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Start LazyGit: {e}"))?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let (input, input_rx) = sync::channel();
    let (events_tx, events) = mpsc::unbounded();

    thread::spawn(move || write_input(stdin, input_rx));
    thread::spawn(move || read_output(child, stdout, events_tx));
    Ok((Backend { input }, events))
}

fn write_input(stdin: ChildStdin, messages: Receiver<Outgoing>) -> io::Result<()> {
    let mut stdin = BufWriter::new(stdin);
    for message in messages {
        serde_json::to_writer(&mut stdin, &message)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
    }
    Ok(())
}

fn read_output(mut child: Child, stdout: ChildStdout, events: UnboundedSender<BackendEvent>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let failure = loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break None,
            Ok(_) => match serde_json::from_str::<Incoming>(&line) {
                Ok(message) => {
                    if events.unbounded_send(BackendEvent::Message(message)).is_err() {
                        // The window is gone; stop lazygit with it.
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                }
                Err(err) => break Some(format!("Bad message from LazyGit: {err}")),
            },
            Err(err) => break Some(format!("Read from LazyGit: {err}")),
        }
    };
    if failure.is_some() {
        let _ = child.kill();
    }
    let event = match (failure, child.wait()) {
        (Some(error), _) => BackendEvent::Error(error),
        (None, Ok(status)) if status.success() => BackendEvent::Closed,
        (None, result) => BackendEvent::Error(format!("LazyGit exited: {result:?}")),
    };
    let _ = events.unbounded_send(event);
}

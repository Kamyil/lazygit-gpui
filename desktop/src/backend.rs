use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use gpui::SharedString;
use serde::Deserialize;
use std::{
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{self as sync, Receiver, Sender},
    thread,
};

#[derive(Deserialize)]
pub struct Pane {
    pub title: SharedString,
    pub lines: Vec<SharedString>,
    pub selected: usize,
    pub origin: usize,
}

#[derive(Deserialize)]
pub struct Snapshot {
    pub focus: String,
    pub files: Pane,
    pub main: Pane,
    pub secondary: Option<Pane>,
    pub other: Option<Pane>,
}

pub enum BackendEvent {
    Frame(Snapshot),
    Closed,
    Error(String),
}

pub fn launch(
    bin: PathBuf,
    repo: &Path,
) -> Result<(Sender<String>, UnboundedReceiver<BackendEvent>), String> {
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
    let input = child.stdin.take().expect("piped stdin");
    let output = child.stdout.take().expect("piped stdout");
    let (keys, key_rx) = sync::channel();
    let (event_tx, events) = mpsc::unbounded();
    thread::spawn(move || {
        if let Err(err) = exchange(input, output, key_rx, &event_tx) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = event_tx.unbounded_send(BackendEvent::Error(format!("LazyGit bridge: {err}")));
            return;
        }
        match child.wait() {
            Ok(status) if status.success() => {
                let _ = event_tx.unbounded_send(BackendEvent::Closed);
            }
            result => {
                let _ = event_tx
                    .unbounded_send(BackendEvent::Error(format!("LazyGit exited: {result:?}")));
            }
        }
    });
    Ok((keys, events))
}

fn exchange(
    mut input: ChildStdin,
    output: ChildStdout,
    keys: Receiver<String>,
    events: &UnboundedSender<BackendEvent>,
) -> io::Result<()> {
    let mut reader = BufReader::new(output);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let snapshot = serde_json::from_str(&line)?;
        if events
            .unbounded_send(BackendEvent::Frame(snapshot))
            .is_err()
        {
            return Ok(());
        }
        let Ok(key) = keys.recv() else {
            return Ok(());
        };
        serde_json::to_writer(&mut input, &key)?;
        input.write_all(b"\n")?;
    }
}

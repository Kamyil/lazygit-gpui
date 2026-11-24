"""Drives lazygit in desktop mode (LAZYGIT_GPUI=1) over its stdin/stdout
protocol, the way the GPUI frontend does, and keeps the same copy of its views
that the frontend keeps. Used by workflows.py."""

import json
import os
import queue
import subprocess
import threading
import time


class Driver:
    def __init__(self, lazygit, repo, config_dir, env=None):
        # Keep lazygit and Git away from the user's own configuration and
        # state: everything they read or write lives under config_dir.
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("LG_", "XDG_", "GIT_")) and key != "CONFIG_DIR"
        }
        environment.update(
            {
                "LAZYGIT_GPUI": "1",
                "CONFIG_DIR": config_dir,
                "XDG_CONFIG_HOME": os.path.join(config_dir, "xdg-config"),
                "XDG_STATE_HOME": os.path.join(config_dir, "xdg-state"),
                "XDG_CACHE_HOME": os.path.join(config_dir, "xdg-cache"),
                "XDG_CONFIG_DIRS": os.path.join(config_dir, "xdg-config-dirs"),
                "GIT_CONFIG_GLOBAL": os.path.join(config_dir, "gitconfig"),
                "GIT_CONFIG_NOSYSTEM": "1",
            }
        )
        environment.update(env or {})
        self.process = subprocess.Popen(
            [lazygit],
            cwd=repo,
            env=environment,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=open(os.path.join(config_dir, "stderr.log"), "w"),
            text=True,
            bufsize=1,
        )
        self.messages = queue.Queue()
        self.frame = None
        self.lines = {}
        self.externals = []
        self.frames_seen = 0
        self.next_sync = 1
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.process.stdout:
            self.messages.put(json.loads(line))
        self.messages.put(None)

    def _apply(self, message):
        if message is None:
            raise RuntimeError("lazygit exited")
        if message["type"] == "external":
            self.externals.append(message)
            return
        self.frames_seen += 1
        for name, lines in (message.get("lines") or {}).items():
            self.lines[name] = lines
        self.lines = {k: v for k, v in self.lines.items() if k in message["views"]}
        self.frame = message

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def key(self, label):
        self.send({"type": "key", "key": label})
        return self

    def type(self, text):
        self.send({"type": "text", "text": text})
        return self

    def paste(self, text):
        self.send({"type": "paste", "text": text})
        return self

    def click(self, view, line, double=False, extend=False):
        self.send(
            {"type": "click", "view": view, "x": 0, "y": line, "double": double, "extend": extend}
        )
        return self

    def sync(self, timeout=20):
        """Waits until lazygit has handled everything sent so far."""
        sync_id = self.next_sync
        self.next_sync += 1
        self.send({"type": "sync", "id": sync_id})
        deadline = time.time() + timeout
        while True:
            message = self.messages.get(timeout=max(0.1, deadline - time.time()))
            self._apply(message)
            if message.get("type") == "frame" and message.get("sync") == sync_id:
                return self

    def drain(self, seconds):
        """Applies whatever lazygit sends on its own for a while."""
        deadline = time.time() + seconds
        while time.time() < deadline:
            try:
                self._apply(self.messages.get(timeout=0.05))
            except queue.Empty:
                pass
        return self

    def wait_for(self, predicate, timeout=15, what="condition"):
        deadline = time.time() + timeout
        while not predicate():
            if time.time() > deadline:
                raise AssertionError(f"timed out waiting for {what}")
            try:
                self._apply(self.messages.get(timeout=0.1))
            except queue.Empty:
                pass
        return self

    def text(self, view):
        return ["".join(run["t"] for run in line) for line in self.lines.get(view, [])]

    def view(self, name):
        return self.frame["views"].get(name)

    def selected(self, view):
        meta = self.view(view)
        return self.text(view)[meta["cursor"]]

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()

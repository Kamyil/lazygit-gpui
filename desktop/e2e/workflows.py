#!/usr/bin/env python3
"""End-to-end checks of lazygit's desktop protocol against synthetic Git
repositories in a temporary directory. Each check drives real lazygit through
real workflows, exactly as the GPUI frontend would, and verifies the result
both in what lazygit reports and in the repository.

Usage: desktop/e2e/workflows.py /path/to/lazygit [check-name ...]

Nothing outside the temporary directory is touched: lazygit's config and state
(CONFIG_DIR) and Git's global config (GIT_CONFIG_GLOBAL) point into it, and
commands that need a terminal run through a fake terminal launcher
(LAZYGIT_GPUI_TERMINAL) that runs them without one."""

import os
import shutil
import subprocess
import sys
import tempfile
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver import Driver  # noqa: E402

CONFIG = """
gui:
  showRandomTip: false
  showCommandLog: true
git:
  autoFetch: false
refresher:
  refreshInterval: 1
promptToReturnFromSubprocess: false
os:
  edit: "printf ' edited' >> {{filename}}"
  editInTerminal: true
keybinding:
  files:
    commitChanges: K
customCommands:
  - key: X
    context: files
    command: touch custom-marker
  - key: Y
    context: files
    command: printf terminal > terminal-marker
    output: terminal
  - key: Z
    context: files
    command: sleep 30
    output: terminal
"""

FAKE_TERMINAL = """#!/bin/sh
# Stands in for a terminal window: runs the script it is given, detached, with
# no terminal attached.
sh "$1" < /dev/null > "$(dirname "$0")/terminal.log" 2>&1 &
"""


class Env:
    def __init__(self, lazygit):
        self.lazygit = lazygit
        self.root = tempfile.mkdtemp(prefix="lazygit-gpui-e2e-")
        self.config = os.path.join(self.root, "config")
        self.repo = os.path.join(self.root, "repo")
        os.makedirs(self.config)
        os.makedirs(self.repo)
        with open(os.path.join(self.config, "config.yml"), "w") as f:
            f.write(CONFIG)
        self.terminal = os.path.join(self.root, "fake-terminal.sh")
        with open(self.terminal, "w") as f:
            f.write(FAKE_TERMINAL)
        self.git_env = dict(
            {k: v for k, v in os.environ.items() if not k.startswith(("GIT_", "XDG_"))},
            GIT_CONFIG_GLOBAL=os.path.join(self.config, "gitconfig"),
            GIT_CONFIG_NOSYSTEM="1",
            GIT_AUTHOR_DATE="2026-01-01T12:00:00",
            GIT_COMMITTER_DATE="2026-01-01T12:00:00",
        )
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.email", "test@example.com")
        self.git("config", "user.name", "Test User")
        self.write("a.txt", "".join(f"line {i}\n" for i in range(1, 21)))
        self.git("add", ".")
        self.git("commit", "-qm", "First commit")
        self.write("b.txt", "bee\n")
        self.git("add", ".")
        self.git("commit", "-qm", "Second commit")
        self.write("c.txt", "sea\n")
        self.git("add", ".")
        self.git("commit", "-qm", "Third commit")
        self.git("tag", "v1.0")
        self.git("branch", "feature")
        self.write(
            "a.txt",
            "".join((f"changed {i}\n" if i in (3, 15) else f"line {i}\n") for i in range(1, 21)),
        )
        self.write("new.txt", "fresh\n")

    def git(self, *args):
        return subprocess.run(
            ["git", *args], cwd=self.repo, env=self.git_env, check=True, capture_output=True, text=True
        ).stdout

    def write(self, name, content):
        with open(os.path.join(self.repo, name), "w") as f:
            f.write(content)

    def read(self, name):
        with open(os.path.join(self.repo, name)) as f:
            return f.read()

    def start(self):
        return Driver(
            self.lazygit,
            self.repo,
            self.config,
            env={"LAZYGIT_GPUI_TERMINAL": f"/bin/sh {self.terminal}"},
        ).sync()

    def cleanup(self):
        shutil.rmtree(self.root, ignore_errors=True)


def expect(condition, message):
    if not condition:
        raise AssertionError(message)


def select_file(d, name):
    for _ in range(len(d.text("files")) + 1):
        if d.selected("files").strip().endswith(name):
            return d
        d.key("<down>").sync()
    raise AssertionError(f"file {name} not found in {d.text('files')}")


def check_layout(env, d):
    """Side panels, tabs and the main view come from lazygit's view tree."""
    expect(d.frame["focus"] == "files", f"focus {d.frame['focus']}")
    windows = {w["name"]: w for w in d.frame["sideWindows"]}
    expect(list(windows) == ["status", "files", "branches", "commits", "stash"], list(windows))
    expect(windows["branches"]["tabTitles"] == ["Local branches", "Remotes", "Tags"], windows["branches"])
    expect(windows["commits"]["tabViews"] == ["commits", "reflogCommits"], windows["commits"])
    expect([w["view"] for w in d.frame["mainWindows"]] == ["main"], d.frame["mainWindows"])
    expect(any("a.txt" in line for line in d.text("files")), d.text("files"))
    expect(len(d.text("commits")) == 3 and "Third commit" in d.text("commits")[0], d.text("commits"))
    expect(any("feature" in line for line in d.text("localBranches")), d.text("localBranches"))
    expect(d.text("status")[0].endswith("main"), d.text("status"))
    expect(any("Stage" in line for line in d.text("options")), d.text("options"))
    expect(d.view("extras") is not None, "command log shown")
    files = d.view("files")
    expect(files["focused"] and files["highlight"] and not files.get("inactive"), files)
    # Content carries lazygit's colors: the file status is colored.
    expect(any(run.get("f") for line in d.lines["files"] for run in line), d.lines["files"])


def check_navigation_and_diff(env, d):
    select_file(d, "a.txt")
    main = d.text("main")
    expect(d.view("main")["title"].endswith("Unstaged changes"), d.view("main"))
    expect("+changed 3" in main and "-line 3" in main, main)
    select_file(d, "new.txt")
    expect("+fresh" in d.text("main"), d.text("main"))
    d.key("<up>").sync()
    expect(d.selected("files").strip().endswith("a.txt"), d.selected("files"))


def check_line_staging(env, d):
    select_file(d, "a.txt")
    d.key("<enter>").sync()
    main = d.view("main")
    expect(d.frame["focus"] == "main" and main.get("highlight"), (d.frame["focus"], main))
    selected = d.text("main")[main["rangeStart"] : main["rangeEnd"] + 1]
    expect(selected == ["-line 3", "+changed 3"], selected)
    # Stage only the added line: line-by-line selection, then <space>.
    d.type("a").sync()
    expect(d.view("main")["rangeStart"] == d.view("main")["rangeEnd"], d.view("main"))
    while d.selected("main") != "+changed 3":
        d.key("<down>").sync()
    d.type(" ").sync()
    staged = env.git("diff", "--cached")
    expect("+changed 3" in staged and "-line 3" not in staged and "changed 15" not in staged, staged)
    expect([w["view"] for w in d.frame["mainWindows"]] == ["main", "secondary"], d.frame["mainWindows"])
    expect("+changed 3" in d.text("secondary"), d.text("secondary"))
    # Stage the second hunk as a whole.
    d.type("a").sync()
    while "changed 15" not in "".join(
        d.text("main")[d.view("main")["rangeStart"] : d.view("main")["rangeEnd"] + 1]
    ):
        d.key("<down>").sync()
    d.type(" ").sync()
    staged = env.git("diff", "--cached")
    expect("+changed 15" in staged, staged)
    # <esc> leaves hunk mode first, then the diff.
    d.key("<esc>").sync().key("<esc>").sync()
    expect(d.frame["focus"] == "files", d.frame["focus"])


def check_commit_with_body(env, d):
    select_file(d, "new.txt")
    d.type(" ").sync()
    # The commit binding is customized to K in the config; c does nothing.
    d.type("c").sync()
    expect(d.frame["popups"] == [], d.frame["popups"])
    d.type("K").sync()
    expect(d.frame["popups"] == ["commitMessage", "commitDescription"], d.frame["popups"])
    expect(d.frame["focus"] == "commitMessage", d.frame["focus"])
    d.type("Add new file ✓").sync()
    summary = d.view("commitMessage")
    expect(summary["input"]["cursorByte"] == len("Add new file ✓".encode()), summary["input"])
    d.key("<tab>").sync()
    expect(d.frame["focus"] == "commitDescription", d.frame["focus"])
    d.paste("Body line one\nBody line two").sync()
    expect(d.text("commitDescription") == ["Body line one", "Body line two"], d.text("commitDescription"))
    d.key("<m-enter>").sync()
    message = env.git("log", "-1", "--format=%B")
    expect(message == "Add new file ✓\n\nBody line one\nBody line two\n\n", repr(message))
    expect(d.frame["popups"] == [] and "Add new file" in d.text("commits")[0], d.text("commits"))


def check_menu_and_filter(env, d):
    d.type("?").sync()
    expect("menu" in d.frame["popups"] and d.view("menu")["title"] == "Keybindings", d.frame["popups"])
    d.type("discard").sync()
    expect(d.text("menuFilter") == ["discard"], d.text("menuFilter"))
    expect(all("iscard" in line or "───" in line for line in d.text("menu")), d.text("menu"))
    d.key("<esc>").sync().key("<esc>").sync()
    expect(d.frame["popups"] == [], d.frame["popups"])


def check_stash_prompt_and_confirmation(env, d):
    d.type("s").sync()
    expect(d.frame["popups"] == ["prompt"] and d.view("prompt")["editable"], d.frame["popups"])
    d.type("wip").key("<backspace>").type("p work").key("<enter>").sync()
    expect("wip work" in env.git("stash", "list"), env.git("stash", "list"))
    d.type("5").sync()
    expect(d.frame["focus"] == "stash" and "wip work" in d.selected("stash"), d.text("stash"))
    d.type("g").sync()
    expect(d.frame["popups"] == ["confirmation"], d.frame["popups"])
    expect("pop" in d.text("confirmation")[0], d.text("confirmation"))
    d.key("<enter>").sync()
    expect(env.git("stash", "list") == "", env.git("stash", "list"))
    expect("M a.txt" in env.git("status", "--short"), env.git("status", "--short"))


def check_branches_and_tabs(env, d):
    d.type("3").sync()
    expect(d.frame["focus"] == "localBranches", (d.frame["focus"], d.frame["popups"], d.text("appStatus")))
    d.type("n").sync()
    d.type("topic").key("<enter>").sync()
    expect(env.git("branch", "--show-current").strip() == "topic", env.git("branch"))
    expect(d.text("status")[0].endswith("topic"), d.text("status"))
    # A click on a tab goes through lazygit's own tab click handler.
    d.send({"type": "tab", "window": "branches", "index": 2})
    d.sync()
    expect(d.frame["focus"] == "tags" and "v1.0" in d.text("tags")[0], d.text("tags"))
    branches = next(w for w in d.frame["sideWindows"] if w["name"] == "branches")
    expect(branches["view"] == "tags", branches)
    d.type("[").sync()  # previous tab
    expect(d.frame["focus"] == "remotes", d.frame["focus"])


def check_mouse(env, d):
    d.click("commits", 2).sync()
    expect(d.frame["focus"] == "commits" and d.view("commits")["cursor"] == 2, d.view("commits"))
    expect("First commit" in d.selected("commits"), d.selected("commits"))
    d.click("commits", 0, extend=True).sync()
    commits = d.view("commits")
    expect((commits["rangeStart"], commits["rangeEnd"]) == (0, 2), commits)
    d.key("<esc>").sync()
    d.click("commits", 1).sync()
    d.click("commits", 1, double=True).sync()
    commits_window = next(w for w in d.frame["sideWindows"] if w["name"] == "commits")
    expect(d.frame["focus"] == "commitFiles" and commits_window["view"] == "commitFiles", d.frame["focus"])
    expect(any("b.txt" in line for line in d.text("commitFiles")), d.text("commitFiles"))
    d.key("<esc>").sync()
    expect(d.frame["focus"] == "commits", d.frame["focus"])
    # Clicking a diff line focuses the diff with that line selected.
    d.click("files", 1).sync()
    select_file(d, "a.txt")
    line = d.text("main").index("+changed 3")
    d.click("main", line).sync()
    main = d.view("main")
    expect(d.frame["focus"] == "main" and main["rangeStart"] <= line <= main["rangeEnd"], main)
    d.key("<esc>").sync()


def check_search(env, d):
    d.type("4").sync()
    d.type("/").sync()
    expect(d.view("search") and d.view("search")["editable"], "search field shown")
    expect(any("Search" in line for line in d.text("searchPrefix")), d.text("searchPrefix"))
    d.type("Second").key("<enter>").sync()
    expect("Second commit" in d.selected("commits"), d.selected("commits"))
    expect(d.view("commits").get("search") == [0, 1], d.view("commits"))
    d.key("<esc>").sync()


def check_custom_commands_and_external_terminal(env, d):
    d.type("2").sync()
    d.type("X").sync()
    expect(os.path.exists(os.path.join(env.repo, "custom-marker")), "custom command ran")
    d.wait_for(lambda: any("custom-marker" in line for line in d.text("files")), what="refresh")
    # output: terminal needs a terminal, so it runs in the (fake) external one.
    d.type("Y").sync()
    expect(env.read("terminal-marker") == "terminal", "terminal command ran")
    expect([e["running"] for e in d.externals[-2:]] == [True, False], d.externals)
    expect(d.externals[-1].get("error") is None, d.externals[-1])
    # The external editor (editInTerminal) goes the same way.
    select_file(d, "b.txt" if any("b.txt" in l for l in d.text("files")) else "a.txt")
    name = d.selected("files").split()[-1]
    before = env.read(name)
    d.type("e").sync()
    expect(env.read(name) == before + " edited", repr(env.read(name)))
    # The user can stop waiting for a command that doesn't finish.
    d.type("Z")
    d.wait_for(lambda: d.externals and d.externals[-1]["running"], what="external command")
    d.send({"type": "cancelExternal"})
    d.wait_for(lambda: not d.externals[-1]["running"], what="cancel")
    expect("stopped waiting" in d.externals[-1]["error"], d.externals[-1])
    d.sync()
    d.key("<esc>").sync()  # the error popup
    expect(d.frame["popups"] == [], d.frame["popups"])


def check_background_refresh(env, d):
    d.type("2").sync()
    env.write("outside.txt", "made elsewhere\n")
    # No input: lazygit's background refresh alone must produce the frame.
    d.wait_for(lambda: any("outside.txt" in line for line in d.text("files")), timeout=10, what="background refresh")
    # Regaining focus refreshes too, like a terminal's focus-in.
    d.send({"type": "focus", "focused": False})
    env.git("add", "outside.txt")
    d.send({"type": "focus", "focused": True})
    d.wait_for(lambda: any("A  outside.txt" in line for line in d.text("files")), timeout=10, what="focus refresh")


def check_lazy_loading(env, d):
    env.write("big.txt", "".join(f"row {i}\n" for i in range(20000)))
    d.type("r").sync()
    select_file(d, "big.txt")
    d.sync()
    main = d.view("main")
    expect(main["loading"] and main["lineCount"] < 20000, main)
    d.send({"type": "load", "view": "main", "lines": 30000})
    d.wait_for(lambda: d.view("main")["lineCount"] >= 20000, what="all lines")
    expect(d.text("main")[-1] == "+row 19999", d.text("main")[-1])


def check_suspend_is_refused(env, d):
    d.key("<c-z>")
    d.wait_for(
        lambda: any("not supported in the desktop frontend" in line for line in d.text("appStatus")),
        what="the reason suspending is refused",
    )
    d.sync()
    expect(d.process.poll() is None, "lazygit still running")


def check_quit(env, d):
    d.type("q")
    d.process.wait(timeout=10)
    expect(d.process.returncode == 0, d.process.returncode)


CHECKS = [
    check_layout,
    check_navigation_and_diff,
    check_line_staging,
    check_commit_with_body,
    check_menu_and_filter,
    check_stash_prompt_and_confirmation,
    check_branches_and_tabs,
    check_mouse,
    check_search,
    check_custom_commands_and_external_terminal,
    check_background_refresh,
    check_lazy_loading,
    check_suspend_is_refused,
    check_quit,
]


def main():
    lazygit = os.path.abspath(sys.argv[1])
    wanted = sys.argv[2:]
    failures = 0
    for check in CHECKS:
        if wanted and check.__name__ not in wanted:
            continue
        env = Env(lazygit)
        d = None
        started = time.time()
        try:
            d = env.start()
            check(env, d)
            print(f"ok   {check.__name__} ({time.time() - started:.1f}s)")
        except Exception:
            failures += 1
            print(f"FAIL {check.__name__}")
            traceback.print_exc()
            if d is not None and d.frame is not None:
                print("  focus:", d.frame["focus"], "popups:", d.frame["popups"])
                for name in d.frame["popups"]:
                    print("  ", name, d.text(name)[:10])
            log = os.path.join(env.config, "stderr.log")
            if os.path.exists(log):
                print(open(log).read()[-3000:])
        finally:
            if d is not None:
                d.close()
            env.cleanup()
    print("all checks passed" if failures == 0 else f"{failures} check(s) failed")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()

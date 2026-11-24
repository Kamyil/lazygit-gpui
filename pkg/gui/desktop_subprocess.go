package gui

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"

	"github.com/jesseduffield/lazygit/pkg/commands/oscommands"
)

// In desktop mode lazygit has no terminal to hand to a subprocess that needs
// one (a terminal editor, a custom command with output: terminal, an
// interactive git command): its stdin and stdout are the pipes to the
// frontend. So such a subprocess runs in a terminal window of its own, and
// lazygit waits for it to finish, just as it waits for it in the terminal.
//
// The terminal is opened by the command in LAZYGIT_GPUI_TERMINAL, with the
// path of a shell script to run appended (shell-quoted); on macOS it defaults
// to opening the script in Terminal.app. The frontend is told about the
// running command, and can stop waiting for it ("cancelExternal").
const desktopTerminalEnvVar = "LAZYGIT_GPUI_TERMINAL"

type desktopExternalRun struct {
	cancel chan struct{}
}

func (self *desktopBridge) externalRunning() bool {
	self.mutex.Lock()
	defer self.mutex.Unlock()
	return self.external != nil
}

func (self *desktopBridge) cancelExternal() {
	self.mutex.Lock()
	defer self.mutex.Unlock()
	if self.external != nil {
		select {
		case <-self.external.cancel:
		default:
			close(self.external.cancel)
		}
	}
}

func desktopTerminalLauncher() (string, error) {
	if launcher := os.Getenv(desktopTerminalEnvVar); launcher != "" {
		return launcher, nil
	}
	if runtime.GOOS == "darwin" {
		return "open -a Terminal", nil
	}
	return "", fmt.Errorf("this command needs a terminal; set %s to a command that opens one", desktopTerminalEnvVar)
}

// runSubprocessInTerminal runs cmdObj in an external terminal and waits until
// it exits, returning an error if it fails.
func (self *desktopBridge) runSubprocessInTerminal(cmdObj *oscommands.CmdObj, promptToReturn bool) error {
	launcher, err := desktopTerminalLauncher()
	if err != nil {
		return err
	}

	dir, err := os.MkdirTemp("", "lazygit-gpui-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(dir)

	cmd := cmdObj.GetCmd()
	statusPath := filepath.Join(dir, "status")
	scriptPath := filepath.Join(dir, "run.command")
	if err := os.WriteFile(scriptPath, []byte(desktopTerminalScript(cmd, statusPath, promptToReturn, self.gui.Tr.PressEnterToReturn)), 0o700); err != nil {
		return err
	}

	run := &desktopExternalRun{cancel: make(chan struct{})}
	self.mutex.Lock()
	self.external = run
	self.mutex.Unlock()
	defer func() {
		self.mutex.Lock()
		self.external = nil
		self.mutex.Unlock()
	}()

	commandLine := strings.Join(cmd.Args, " ")
	self.queueNotice(desktopExternal{Type: "external", Command: commandLine, Running: true})
	err = self.waitForTerminal(launcher, scriptPath, statusPath, run)
	notice := desktopExternal{Type: "external", Command: commandLine}
	if err != nil {
		notice.Error = err.Error()
	}
	self.queueNotice(notice)
	return err
}

func (self *desktopBridge) waitForTerminal(launcher, scriptPath, statusPath string, run *desktopExternalRun) error {
	launch := exec.Command("sh", "-c", launcher+" "+shellQuote(scriptPath))
	launch.Stdin, launch.Stdout, launch.Stderr = nil, nil, nil
	if output, err := launch.CombinedOutput(); err != nil {
		return fmt.Errorf("open a terminal with %q: %w\n%s", launcher, err, output)
	}

	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()
	for {
		select {
		case <-run.cancel:
			return errors.New("stopped waiting for the command in the external terminal; it may still be running")
		case <-ticker.C:
			content, err := os.ReadFile(statusPath)
			if err != nil {
				continue
			}
			status, err := strconv.Atoi(strings.TrimSpace(string(content)))
			if err != nil || status != 0 {
				return fmt.Errorf("exit status %s", strings.TrimSpace(string(content)))
			}
			return nil
		}
	}
}

// desktopTerminalScript returns a script that runs cmd as exec would (same
// arguments, directory and environment), then records its exit status in
// statusPath.
func desktopTerminalScript(cmd *exec.Cmd, statusPath string, promptToReturn bool, pressEnterToReturn string) string {
	env := cmd.Env
	if env == nil {
		env = os.Environ()
	}
	dir := cmd.Dir
	if dir == "" {
		dir, _ = os.Getwd()
	}
	status := shellQuote(statusPath)

	var script strings.Builder
	script.WriteString("#!/bin/sh\n")
	fmt.Fprintf(&script, "trap 'printf 129 > %s.tmp; mv %s.tmp %s; exit 129' HUP INT TERM\n", status, status, status)
	fmt.Fprintf(&script, "cd %s || { printf 1 > %s.tmp; mv %s.tmp %s; exit 1; }\n", shellQuote(dir), status, status, status)
	fmt.Fprintf(&script, "printf '%%s\\n\\n' %s\n", shellQuote("+ "+strings.Join(cmd.Args, " ")))
	script.WriteString("env -i")
	for _, variable := range env {
		script.WriteString(" " + shellQuote(variable))
	}
	// Path is the program that Args[0] resolved to.
	script.WriteString(" " + shellQuote(cmd.Path))
	for _, arg := range cmd.Args[1:] {
		script.WriteString(" " + shellQuote(arg))
	}
	script.WriteString("\ncode=$?\n")
	if promptToReturn {
		fmt.Fprintf(&script, "printf '\\n%%s' %s\nread _\n", shellQuote(pressEnterToReturn))
	} else {
		fmt.Fprintf(&script, "[ $code -eq 0 ] || { printf '\\n%%s' %s; read _; }\n", shellQuote(pressEnterToReturn))
	}
	fmt.Fprintf(&script, "printf '%%s' \"$code\" > %s.tmp && mv %s.tmp %s\n", status, status, status)
	return script.String()
}

func shellQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}

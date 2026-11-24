package gui

import (
	"encoding/json"
	"io"
	"os"

	"github.com/gdamore/tcell/v3"
	"github.com/jesseduffield/lazygit/pkg/config"
	"github.com/jesseduffield/lazygit/pkg/gocui"
)

func desktopMode() bool { return os.Getenv("LAZYGIT_GPUI") == "1" }

type desktopPane struct {
	Title    string   `json:"title"`
	Lines    []string `json:"lines"`
	Selected int      `json:"selected"`
	Origin   int      `json:"origin"`
}

type desktopSnapshot struct {
	Focus     string       `json:"focus"`
	Files     desktopPane  `json:"files"`
	Main      desktopPane  `json:"main"`
	Secondary *desktopPane `json:"secondary"`
	Other     *desktopPane `json:"other"`
}

func desktopView(view *gocui.View) desktopPane {
	origin, _ := view.BufferLineForViewLine(view.OriginY())
	selected, _ := view.BufferLineForViewLine(view.SelectedLineIdx())
	return desktopPane{view.Title, view.BufferLines(), max(0, selected), max(0, origin)}
}

// Read the view buffers on LazyGit's UI thread, never from the terminal screen.
func (gui *Gui) desktopSnapshot() desktopSnapshot {
	views := gui.c.Views()
	snapshot := desktopSnapshot{Files: desktopView(views.Files), Main: desktopView(views.Main)}
	if views.Secondary.Visible {
		pane := desktopView(views.Secondary)
		snapshot.Secondary = &pane
	}
	if view := gui.g.CurrentView(); view != nil {
		snapshot.Focus = view.Name()
		if view != views.Files && view != views.Main && view != views.Secondary {
			pane := desktopView(view)
			snapshot.Other = &pane
		}
	}
	return snapshot
}

// Send one initial snapshot, then one for each JSON key label received on stdin.
// Replaying through gocui uses the same input dispatch as LazyGit's TUI.
func (gui *Gui) serveDesktop(input io.Reader, output io.Writer) {
	g := gui.g
	decoder, encoder := json.NewDecoder(input), json.NewEncoder(output)
	defer g.Update(func(*gocui.Gui) error { return gocui.ErrQuit })

	// Cross the UI queue before checking idle so the initial layout has run.
	if g.OnUIThreadAndWait(func() {}) != nil {
		return
	}
	for {
		g.WaitUntilIdle()
		// The TUI only loads a few screenfuls ahead. GPUI scrolls independently.
		if g.OnUIThreadAndWait(func() {
			views := gui.c.Views()
			for _, view := range []*gocui.View{views.Main, views.Secondary} {
				if view.Visible {
					if manager := gui.getViewBufferManagerForView(view); manager != nil && manager.IsLoading() {
						manager.ReadLinesAndWait(-1)
					}
				}
			}
		}) != nil {
			return
		}
		g.WaitUntilIdle()
		var snapshot desktopSnapshot
		if g.OnUIThreadAndWait(func() { snapshot = gui.desktopSnapshot() }) != nil || encoder.Encode(snapshot) != nil {
			return
		}
		var label string
		if decoder.Decode(&label) != nil {
			return
		}
		if key, ok := config.KeyFromLabel(label); ok {
			g.ReplayKeyEvent(gocui.NewTcellKeyEventWrapper(
				tcell.NewEventKey(tcell.Key(key.KeyName()), key.Str(), tcell.ModMask(key.Mod())),
			))
		}
	}
}

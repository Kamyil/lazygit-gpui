package gui

import (
	"github.com/jesseduffield/lazygit/pkg/gui/context"
	"github.com/jesseduffield/lazygit/pkg/gui/types"
)

func (gui *Gui) contextTree() *context.ContextTree {
	contextCommon := &context.ContextCommon{
		IGuiCommon: gui.c.IGuiCommon,
		Common:     gui.c.Common,
	}
	contextTree := context.NewContextTree(contextCommon)
	if desktopMode() {
		// The desktop frontend scrolls lists itself, so it needs all of
		// their lines, not just those in the headless screen's viewport.
		for _, c := range contextTree.Flatten() {
			if list, ok := c.(interface{ RenderAllLines() }); ok {
				list.RenderAllLines()
			}
		}
	}
	return contextTree
}

func (gui *Gui) defaultSideContext() types.Context {
	if gui.State.Modes.Filtering.Active() {
		return gui.State.Contexts.LocalCommits
	}

	return gui.State.Contexts.Files
}

package gui

import (
	"bytes"
	"encoding/json"
	"io"
	"os"
	"slices"
	"sync"

	"github.com/gdamore/tcell/v3"
	"github.com/jesseduffield/lazygit/pkg/config"
	"github.com/jesseduffield/lazygit/pkg/gocui"
	"github.com/jesseduffield/lazygit/pkg/gui/types"
	"github.com/rivo/uniseg"
	"github.com/samber/lo"
)

// The desktop frontend (desktop/ in this repository) runs lazygit as a
// subprocess with LAZYGIT_GPUI=1 and draws lazygit's views natively. lazygit
// runs headless: its event loop, contexts, controllers and keybindings work
// exactly as in the terminal, but instead of drawing to a terminal it describes
// its views, as structured data, on stdout. The frontend sends input on stdin.
// Both directions carry one JSON object per line.
//
// lazygit sends a "frame" whenever drawing changed what its views show. A frame
// lists every visible view with its title, selection, cursor and so on; the
// content of a view (styled lines) is included only when it changed since the
// last frame, and otherwise the frontend keeps what it has.
func desktopMode() bool { return os.Getenv("LAZYGIT_GPUI") == "1" }

// The size of the headless screen in desktop mode. The frontend scrolls views
// independently of this, but lazygit's layout (e.g. how many columns a list
// shows, or how far a page scrolls) is computed for it.
const (
	desktopScreenWidth  = 200
	desktopScreenHeight = 60
)

// A run of equally styled text of a line of a view.
type desktopRun struct {
	Text string `json:"t"`
	// See desktopColor.
	Fg uint32 `json:"f,omitempty"`
	Bg uint32 `json:"b,omitempty"`
	// See desktopAttributes.
	Attributes uint8  `json:"a,omitempty"`
	Hyperlink  string `json:"h,omitempty"`
}

// The state of an editable view's text area.
type desktopInput struct {
	// The buffer line and the byte offset within it that the cursor is at.
	CursorLine int `json:"cursorLine"`
	CursorByte int `json:"cursorByte"`
	// If not empty, the view shows this in place of every character.
	Mask string `json:"mask,omitempty"`
}

type desktopView struct {
	Title    string   `json:"title,omitempty"`
	Subtitle string   `json:"subtitle,omitempty"`
	Footer   string   `json:"footer,omitempty"`
	Tabs     []string `json:"tabs,omitempty"`
	TabIndex int      `json:"tabIndex"`
	// The window that the view is shown in, and the key and kind of its
	// context (empty for views without one, like the options bar).
	Window  string `json:"window"`
	Context string `json:"context,omitempty"`
	Kind    string `json:"kind,omitempty"`
	// Whether the view, or the panel it is part of, has the keyboard focus.
	Focused bool `json:"focused"`
	// How many lines of content the view has; the lines themselves are only
	// sent when they changed.
	LineCount int `json:"lineCount"`
	// Whether more content is still being read into the view; see the "load"
	// input.
	Loading bool `json:"loading,omitempty"`
	// The buffer line shown at the top of the view.
	Origin int `json:"origin"`
	// The selection, in buffer lines. Shown only if Highlight is true; it is
	// shown in the inactive colors if Inactive is true. If SelectionWidth is
	// greater than zero, only that many columns at the edges of a selected
	// line are colored, because the line's own colors carry meaning.
	Cursor         int  `json:"cursor"`
	RangeStart     int  `json:"rangeStart"`
	RangeEnd       int  `json:"rangeEnd"`
	Highlight      bool `json:"highlight,omitempty"`
	Inactive       bool `json:"inactive,omitempty"`
	SelectionWidth int  `json:"selectionWidth,omitempty"`
	// The buffer lines the inclusion gutter marks, and its marker.
	Gutter       []int         `json:"gutter,omitempty"`
	GutterMarker string        `json:"gutterMarker,omitempty"`
	Editable     bool          `json:"editable,omitempty"`
	Input        *desktopInput `json:"input,omitempty"`
	// The view's own text color, for text without a color of its own.
	Fg uint32 `json:"fg,omitempty"`
	// The index of the current search match and the number of matches, while
	// the view is being searched.
	Search []int `json:"search,omitempty"`
}

type desktopWindow struct {
	Name string `json:"name"`
	// The view the window shows.
	View string `json:"view"`
	// The views of the window's tabs and their titles, if it has tabs.
	TabViews  []string `json:"tabViews,omitempty"`
	TabTitles []string `json:"tabTitles,omitempty"`
}

type desktopFrame struct {
	Type string `json:"type"`
	Seq  uint64 `json:"seq"`
	// Set on the frame that answers a "sync" input.
	Sync uint64 `json:"sync,omitempty"`

	Repo string `json:"repo"`
	// The view with the keyboard focus, and the key and kind of its context.
	Focus       string `json:"focus"`
	Context     string `json:"context"`
	ContextKind string `json:"contextKind"`

	SideWindows []desktopWindow `json:"sideWindows"`
	MainWindows []desktopWindow `json:"mainWindows"`
	// The visible popup views, bottom to top.
	Popups []string `json:"popups"`
	// Every visible view.
	Views map[string]*desktopView `json:"views"`
	// The content of the views whose content changed since the last frame.
	Lines map[string][][]desktopRun `json:"lines,omitempty"`
}

// An external command running in a terminal outside lazygit; see
// runSubprocessInTerminal.
type desktopExternal struct {
	Type    string `json:"type"`
	Command string `json:"command"`
	Running bool   `json:"running"`
	Error   string `json:"error,omitempty"`
}

type desktopInputMessage struct {
	Type string `json:"type"`
	// "key": a key label in the format of lazygit's keybinding config.
	Key string `json:"key"`
	// "text", "paste": text typed or pasted.
	Text string `json:"text"`
	// "click": a click on line Y (a buffer line) and column X of view View.
	// Double marks a double click, Extend a click that extends the selection
	// (as dragging to there would).
	View   string `json:"view"`
	X      int    `json:"x"`
	Y      int    `json:"y"`
	Double bool   `json:"double"`
	Extend bool   `json:"extend"`
	// "tab": a click on tab Index of window Window.
	Window string `json:"window"`
	Index  int    `json:"index"`
	// "load": the frontend wants at least Lines lines of View's content.
	Lines int `json:"lines"`
	// "focus": the frontend's window gained or lost the focus.
	Focused bool `json:"focused"`
	// "sync": answer with a frame carrying this ID once lazygit is idle.
	ID uint64 `json:"id"`
}

type desktopBridge struct {
	gui    *Gui
	output io.Writer

	// Owned by the UI thread.
	seq         uint64
	syncID      uint64
	lastMeta    []byte
	sentDigests map[string]uint64

	// The frame (or external-command notice) waiting to be written, and a
	// signal for the writer that there is one. A frame that comes before the
	// previous one is written is merged into it, so that a slow reader never
	// holds up lazygit.
	mutex          sync.Mutex
	pendingFrame   *desktopFrame
	pendingNotices [][]byte
	wake           chan struct{}

	// Set while an external command runs; the UI thread is blocked then, so
	// input is dropped instead of queueing up behind it.
	external *desktopExternalRun
}

func newDesktopBridge(gui *Gui, output io.Writer) *desktopBridge {
	return &desktopBridge{
		gui:         gui,
		output:      output,
		sentDigests: map[string]uint64{},
		wake:        make(chan struct{}, 1),
	}
}

// startDesktopBridge connects the gui to the desktop frontend over stdin and
// stdout. stdout is reserved for the protocol from here on: anything else that
// would write to it, like a subprocess, writes to stderr instead.
func (gui *Gui) startDesktopBridge() {
	output := os.Stdout
	os.Stdout = os.Stderr

	bridge := newDesktopBridge(gui, output)
	gui.desktop = bridge
	gui.g.AfterFlush = bridge.capture
	go bridge.writeLoop()
	go bridge.readLoop(os.Stdin)
}

// capture runs on the UI thread after every flush, and queues a frame if what
// the views show changed.
func (self *desktopBridge) capture() {
	frame := self.buildFrame()

	// Compare without the content, which is only there if it changed anyway.
	lines := frame.Lines
	frame.Lines = nil
	meta, err := json.Marshal(frame)
	frame.Lines = lines
	if err != nil {
		self.gui.c.Log.Error(err)
		return
	}
	sync := self.syncID
	self.syncID = 0
	if len(frame.Lines) == 0 && sync == 0 && bytes.Equal(meta, self.lastMeta) {
		return
	}
	self.lastMeta = meta

	frame.Type = "frame"
	self.seq++
	frame.Seq = self.seq
	frame.Sync = sync
	self.queueFrame(frame)
}

func (self *desktopBridge) buildFrame() *desktopFrame {
	gui := self.gui
	frame := &desktopFrame{
		Repo:  gui.git.RepoPaths.WorktreePath(),
		Views: map[string]*desktopView{},
	}

	contextByView := map[string]types.Context{}
	for _, context := range gui.State.Contexts.Flatten() {
		if _, ok := contextByView[context.GetViewName()]; !ok {
			contextByView[context.GetViewName()] = context
		}
	}

	current := gui.c.Context().Current()
	frame.Context = string(current.GetKey())
	frame.ContextKind = desktopContextKind(current.GetKind())
	currentView := gui.g.CurrentView()
	if currentView != nil {
		frame.Focus = currentView.Name()
	}

	lines := map[string][][]desktopRun{}
	visible := map[string]bool{}
	for _, view := range gui.g.Views() {
		if !view.Visible || view.Name() == "statusSpacer1" || view.Name() == "statusSpacer2" {
			continue
		}
		visible[view.Name()] = true
		state := self.viewState(view, contextByView[view.Name()], currentView)
		frame.Views[view.Name()] = state

		digest := view.ContentDigest()
		if sent, ok := self.sentDigests[view.Name()]; !ok || sent != digest {
			lines[view.Name()] = desktopLines(view.StyledLines())
			self.sentDigests[view.Name()] = digest
		}
	}
	// A view that becomes visible again has to be sent in full.
	for name := range self.sentDigests {
		if !visible[name] {
			delete(self.sentDigests, name)
		}
	}
	if len(lines) > 0 {
		frame.Lines = lines
	}

	tabs := gui.viewTabMap()
	window := func(name string) (desktopWindow, bool) {
		viewName := gui.helpers.Window.GetViewNameForWindow(name)
		if !visible[viewName] {
			return desktopWindow{}, false
		}
		result := desktopWindow{Name: name, View: viewName}
		for _, tab := range tabs[name] {
			result.TabViews = append(result.TabViews, tab.ViewName)
			result.TabTitles = append(result.TabTitles, tab.Tab)
		}
		return result, true
	}
	frame.SideWindows = []desktopWindow{}
	for _, name := range gui.helpers.Window.SideWindows() {
		if result, ok := window(name); ok {
			frame.SideWindows = append(frame.SideWindows, result)
		}
	}
	frame.MainWindows = []desktopWindow{}
	for _, name := range []string{"main", "secondary"} {
		if result, ok := window(name); ok {
			frame.MainWindows = append(frame.MainWindows, result)
		}
	}

	frame.Popups = []string{}
	for _, name := range gui.popupLayerViewNames() {
		if visible[name] {
			frame.Popups = append(frame.Popups, name)
		}
	}

	return frame
}

// popupLayerViewNames returns the names of the views that are drawn over the
// rest of the layout, bottom to top.
func (gui *Gui) popupLayerViewNames() []string {
	mappings := gui.orderedViewNameMappings()
	firstPopup := lo.IndexOf(lo.Map(mappings, func(m viewNameMapping, _ int) string { return m.name }), "statusSpacer2") + 1
	return lo.Map(mappings[firstPopup:], func(m viewNameMapping, _ int) string { return m.name })
}

func (self *desktopBridge) viewState(view *gocui.View, context types.Context, currentView *gocui.View) *desktopView {
	gui := self.gui
	state := &desktopView{
		Title:          view.TitlePrefix + view.Title,
		Subtitle:       view.Subtitle,
		Footer:         view.Footer,
		Tabs:           view.Tabs,
		TabIndex:       view.TabIndex,
		Window:         view.Name(),
		LineCount:      view.LinesHeight(),
		Highlight:      view.Highlight,
		SelectionWidth: view.SelectedLineColorWidth,
		Editable:       view.Editable,
		Fg:             desktopColor(view.FgColor),
		Cursor:         -1,
		RangeStart:     -1,
		RangeEnd:       -1,
	}
	if context != nil {
		state.Window = context.GetWindowName()
		state.Context = string(context.GetKey())
		state.Kind = desktopContextKind(context.GetKind())
	}
	if currentView != nil {
		state.Focused = view == currentView ||
			view.ParentView == currentView || currentView.ParentView == view ||
			viewsBelongToSamePopupPanel(view.Name(), currentView.Name())
	}
	state.Inactive = view.HighlightInactive || !state.Focused

	if manager := gui.getViewBufferManagerForView(view); manager != nil {
		state.Loading = manager.IsLoading()
	}
	if origin, ok := view.BufferLineForViewLine(view.OriginY()); ok {
		state.Origin = origin
	}
	if cursor, ok := view.BufferLineForViewLine(view.SelectedLineIdx()); ok {
		state.Cursor = cursor
		state.RangeStart, state.RangeEnd = cursor, cursor
	}
	if first, last, ok := view.SelectedBufferLineRange(); ok {
		state.RangeStart, state.RangeEnd = first, last
	}
	if marks := view.InclusionGutterMarks(); marks != nil {
		state.GutterMarker = view.InclusionGutterMarker
		state.Gutter = []int{}
		for i, marked := range marks {
			if marked {
				state.Gutter = append(state.Gutter, i)
			}
		}
	}
	if view.Editable && view.TextArea != nil {
		state.Input = desktopTextAreaInput(view)
	}
	if view.IsSearching() {
		current, total := view.GetSearchStatus()
		state.Search = []int{current, total}
	}
	return state
}

func desktopTextAreaInput(view *gocui.View) *desktopInput {
	cursorX, cursorY := view.TextArea.GetCursorXY()
	lines := splitLines(view.TextArea.GetContent())
	input := &desktopInput{CursorLine: cursorY, Mask: view.Mask}
	if cursorY < len(lines) {
		line := lines[cursorY]
		width, offset := 0, 0
		state := -1
		for offset < len(line) && width < cursorX {
			var cluster string
			var clusterWidth int
			cluster, _, clusterWidth, state = uniseg.FirstGraphemeClusterInString(line[offset:], state)
			offset += len(cluster)
			width += clusterWidth
		}
		input.CursorByte = offset
	}
	return input
}

func splitLines(s string) []string {
	return slices.Collect(func(yield func(string) bool) {
		for line := range bytes.Lines([]byte(s)) {
			if !yield(string(bytes.TrimSuffix(line, []byte("\n")))) {
				return
			}
		}
	})
}

func desktopContextKind(kind types.ContextKind) string {
	switch kind {
	case types.SIDE_CONTEXT:
		return "side"
	case types.MAIN_CONTEXT:
		return "main"
	case types.PERSISTENT_POPUP:
		return "persistentPopup"
	case types.TEMPORARY_POPUP:
		return "temporaryPopup"
	case types.EXTRAS_CONTEXT:
		return "extras"
	case types.GLOBAL_CONTEXT:
		return "global"
	case types.DISPLAY_CONTEXT:
		return "display"
	}
	return ""
}

func desktopLines(styled [][]gocui.StyledRun) [][]desktopRun {
	result := make([][]desktopRun, len(styled))
	for i, line := range styled {
		runs := make([]desktopRun, len(line))
		for j, run := range line {
			runs[j] = desktopRun{
				Text:       run.Text,
				Fg:         desktopColor(run.Fg),
				Bg:         desktopColor(run.Bg),
				Attributes: desktopAttributes(run.Fg | run.Bg),
				Hyperlink:  run.Hyperlink,
			}
		}
		result[i] = runs
	}
	return result
}

// desktopColor encodes a color for the frontend: 0 is the default color,
// 1 to 256 are the terminal palette colors 0 to 255, and 0x1000000 plus a
// 24-bit RGB value is that RGB color.
func desktopColor(attribute gocui.Attribute) uint32 {
	color := attribute & gocui.AttrColorBits
	if color&gocui.AttrIsValidColor == 0 {
		return 0
	}
	if color&gocui.AttrIsRGBColor != 0 {
		return 1<<24 | uint32(color&0xffffff)
	}
	return uint32(color&0xff) + 1
}

// desktopAttributes encodes text attributes for the frontend as bit flags:
// 1 bold, 2 underline, 4 reverse, 8 dim, 16 italic, 32 strikethrough.
func desktopAttributes(attribute gocui.Attribute) uint8 {
	var result uint8
	for i, flag := range []gocui.Attribute{
		gocui.AttrBold, gocui.AttrUnderline, gocui.AttrReverse,
		gocui.AttrDim, gocui.AttrItalic, gocui.AttrStrikeThrough,
	} {
		if attribute&flag != 0 {
			result |= 1 << i
		}
	}
	return result
}

func (self *desktopBridge) queueFrame(frame *desktopFrame) {
	self.mutex.Lock()
	if pending := self.pendingFrame; pending != nil {
		for name, lines := range pending.Lines {
			if _, ok := frame.Lines[name]; !ok && frame.Views[name] != nil {
				if frame.Lines == nil {
					frame.Lines = map[string][][]desktopRun{}
				}
				frame.Lines[name] = lines
			}
		}
		if frame.Sync == 0 {
			frame.Sync = pending.Sync
		}
	}
	self.pendingFrame = frame
	self.mutex.Unlock()
	self.signal()
}

func (self *desktopBridge) queueNotice(notice any) {
	encoded, err := json.Marshal(notice)
	if err != nil {
		return
	}
	self.mutex.Lock()
	self.pendingNotices = append(self.pendingNotices, encoded)
	self.mutex.Unlock()
	self.signal()
}

func (self *desktopBridge) signal() {
	select {
	case self.wake <- struct{}{}:
	default:
	}
}

func (self *desktopBridge) writeLoop() {
	for range self.wake {
		self.mutex.Lock()
		frame, notices := self.pendingFrame, self.pendingNotices
		self.pendingFrame, self.pendingNotices = nil, nil
		self.mutex.Unlock()

		var buffer bytes.Buffer
		if frame != nil {
			encoded, err := json.Marshal(frame)
			if err != nil {
				self.gui.c.Log.Error(err)
				continue
			}
			buffer.Write(encoded)
			buffer.WriteByte('\n')
		}
		for _, notice := range notices {
			buffer.Write(notice)
			buffer.WriteByte('\n')
		}
		if _, err := self.output.Write(buffer.Bytes()); err != nil {
			// The frontend is gone.
			self.quit()
			return
		}
	}
}

func (self *desktopBridge) quit() {
	self.gui.g.Update(func(*gocui.Gui) error { return gocui.ErrQuit })
}

func (self *desktopBridge) readLoop(input io.Reader) {
	defer self.quit()

	decoder := json.NewDecoder(input)
	for {
		var message desktopInputMessage
		if err := decoder.Decode(&message); err != nil {
			return
		}
		self.handleInput(message)
	}
}

func (self *desktopBridge) handleInput(message desktopInputMessage) {
	g := self.gui.g
	if message.Type == "cancelExternal" {
		self.cancelExternal()
		return
	}
	if self.externalRunning() {
		// The UI thread is blocked until the command finishes, so this
		// would wait until then; the terminal has the user's attention.
		if message.Type == "sync" {
			go self.sync(message.ID)
		}
		return
	}

	switch message.Type {
	case "key":
		if key, ok := config.KeyFromLabel(message.Key); ok && key.IsSet() {
			g.ReplayKeyEvent(gocui.NewTcellKeyEventWrapper(
				tcell.NewEventKey(tcell.Key(key.KeyName()), key.Str(), tcell.ModMask(key.Mod())),
			))
		}
	case "text":
		self.replayText(message.Text)
	case "paste":
		g.ReplayPasteEvent(gocui.NewTcellPasteEventWrapper(tcell.NewEventPaste(true)))
		self.replayText(message.Text)
		g.ReplayPasteEvent(gocui.NewTcellPasteEventWrapper(tcell.NewEventPaste(false)))
	case "click":
		modifier := gocui.ModNone
		if message.Extend {
			modifier = gocui.ModMotion
		}
		g.ReplayViewMouseEvent(message.View, message.X, message.Y, gocui.MouseLeft, modifier, message.Double)
		g.ReplayViewMouseEvent(message.View, message.X, message.Y, gocui.MouseRelease, gocui.ModNone, false)
	case "tab":
		g.Update(func(*gocui.Gui) error {
			return self.gui.onViewTabClick(message.Window, message.Index)
		})
	case "load":
		g.Update(func(*gocui.Gui) error {
			if view, err := g.View(message.View); err == nil {
				if manager := self.gui.getViewBufferManagerForView(view); manager != nil {
					manager.ReadLines(message.Lines)
				}
			}
			return nil
		})
	case "focus":
		g.ReplayFocusEvent(gocui.NewTcellFocusEventWrapper(tcell.NewEventFocus(message.Focused)))
	case "sync":
		go self.sync(message.ID)
	}
}

func (self *desktopBridge) replayText(text string) {
	g := self.gui.g
	state := -1
	for text != "" {
		var cluster string
		cluster, text, _, state = uniseg.FirstGraphemeClusterInString(text, state)
		var event *tcell.EventKey
		switch cluster {
		case "\n", "\r\n", "\r":
			event = tcell.NewEventKey(tcell.KeyEnter, "", tcell.ModNone)
		case "\t":
			event = tcell.NewEventKey(tcell.KeyTab, "", tcell.ModNone)
		default:
			event = tcell.NewEventKey(tcell.KeyRune, cluster, tcell.ModNone)
		}
		g.ReplayKeyEvent(gocui.NewTcellKeyEventWrapper(event))
	}
}

// sync waits until lazygit is idle, then makes it send a frame tagged with
// id, even if nothing changed. A frontend (or a test) uses it to know that
// what it sees reflects all the input it sent before.
func (self *desktopBridge) sync(id uint64) {
	g := self.gui.g
	g.WaitUntilIdle()
	g.Update(func(*gocui.Gui) error {
		self.syncID = id
		return nil
	})
}

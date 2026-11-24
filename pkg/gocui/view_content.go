package gocui

// StyledRun is a stretch of one line of a view's content whose cells all have
// the same colors and attributes. It is for frontends that draw a view's
// content themselves rather than through the terminal screen.
type StyledRun struct {
	Text string
	// The colors the content was written with, attributes such as AttrBold
	// included. ColorDefault means the view's own color.
	Fg, Bg Attribute
	// The target of the hyperlink the run is part of, if any.
	Hyperlink string
}

// StyledLines returns the view's content, one entry per unwrapped buffer line
// (the same lines as BufferLines), each split into runs of equally styled text.
// The view's selection, search matches and inclusion gutter are not part of
// the content and so not part of the result.
func (v *View) StyledLines() [][]StyledRun {
	v.writeMutex.Lock()
	defer v.writeMutex.Unlock()

	result := make([][]StyledRun, len(v.buf.lines))
	for i := range v.buf.lines {
		result[i] = styledRuns(v.buf.lines[i].cells)
	}
	return result
}

func styledRuns(line cells) []StyledRun {
	var runs []StyledRun
	start := 0
	flush := func(end int) {
		if end == start {
			return
		}
		first := &line[start]
		runs = append(runs, StyledRun{
			Text:      line[start:end].String(),
			Fg:        first.fgColor,
			Bg:        first.bgColor,
			Hyperlink: first.hyperlink(),
		})
		start = end
	}
	for i := range line {
		first := &line[start]
		if line[i].fgColor != first.fgColor || line[i].bgColor != first.bgColor ||
			line[i].hyperlink() != first.hyperlink() {
			flush(i)
		}
	}
	flush(len(line))
	return runs
}

// ContentDigest returns a hash of the view's content, colors included. A
// frontend that mirrors the content can compare digests to find out cheaply
// whether it has to copy the content again.
func (v *View) ContentDigest() uint64 {
	v.writeMutex.Lock()
	defer v.writeMutex.Unlock()

	// FNV-1a, inlined so that hashing a long buffer doesn't allocate.
	const prime = 1099511628211
	hash := uint64(14695981039346656037)
	writeNumber := func(n uint64) {
		for range 8 {
			hash = (hash ^ (n & 0xff)) * prime
			n >>= 8
		}
	}
	writeNumber(uint64(len(v.buf.lines)))
	for i := range v.buf.lines {
		line := v.buf.lines[i].cells
		writeNumber(uint64(len(line)))
		for j := range line {
			for k := 0; k < len(line[j].chr); k++ {
				hash = (hash ^ uint64(line[j].chr[k])) * prime
			}
			writeNumber(uint64(line[j].fgColor))
			writeNumber(uint64(line[j].bgColor))
		}
	}
	return hash
}

// InclusionGutterMarks returns which lines of the content the inclusion gutter
// marks (see SetInclusionGutter), or nil while the gutter is hidden.
func (v *View) InclusionGutterMarks() []bool {
	v.writeMutex.Lock()
	defer v.writeMutex.Unlock()

	if !v.showInclusionGutter {
		return nil
	}
	return append([]bool(nil), v.inclusionGutterMarks...)
}

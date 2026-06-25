package progress

import (
	"fmt"
	"io"
	"os"
	"strings"
	"sync"
)

const barWidth = 20

// Bar reports progress for a local, countable operation (hashing files into the
// cache during add/import/setup). Network transfers are not covered here: rclone
// renders its own --progress bar for push and pull.
//
// On a terminal the bar redraws in place with a carriage return. When the writer
// is not a terminal (a pipe, a file, CI logs), per-step redraws are suppressed
// and a single summary line is written on Close, so logs are not flooded.
type Bar struct {
	w       io.Writer
	label   string
	total   int
	enabled bool
	isTTY   bool

	mu     sync.Mutex
	done   int
	closed bool
}

func New(w io.Writer, label string, total int, off bool) *Bar {
	b := &Bar{w: w, label: label, total: total}
	b.enabled = !off && w != nil && total > 0
	b.isTTY = isTerminal(w)
	return b
}

// Step advances the bar by one unit. It is safe to call concurrently from
// multiple worker goroutines.
func (b *Bar) Step() {
	if b == nil || !b.enabled {
		return
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	b.done++
	if b.isTTY {
		fmt.Fprint(b.w, "\r"+barLine(b.label, b.done, b.total))
	}
}

// Close finishes the bar: it terminates the in-place line on a terminal, or
// writes a single summary line otherwise. Calling Close more than once is safe.
func (b *Bar) Close() {
	if b == nil || !b.enabled {
		return
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.closed || b.done == 0 {
		return
	}
	b.closed = true
	if b.isTTY {
		fmt.Fprintln(b.w)
		return
	}
	fmt.Fprintf(b.w, "%s %d/%d\n", b.label, b.done, b.total)
}

// barLine renders the bar body without the carriage return or trailing newline,
// e.g. "add [##########----------] 12/24".
func barLine(label string, done, total int) string {
	filled := done * barWidth / total
	if filled > barWidth {
		filled = barWidth
	}
	bar := strings.Repeat("#", filled) + strings.Repeat("-", barWidth-filled)
	return fmt.Sprintf("%s [%s] %d/%d", label, bar, done, total)
}

// isTerminal reports whether w is a character device (a terminal). It avoids a
// third-party dependency by inspecting the file mode directly.
func isTerminal(w io.Writer) bool {
	f, ok := w.(*os.File)
	if !ok {
		return false
	}
	info, err := f.Stat()
	if err != nil {
		return false
	}
	return info.Mode()&os.ModeCharDevice != 0
}

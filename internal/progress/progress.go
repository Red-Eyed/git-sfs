package progress

import (
	"fmt"
	"io"
	"os"
	"strings"
	"sync"
	"time"
)

const barWidth = 20

// Bar reports progress for a local, countable operation. It has two modes:
//
//   - count mode (New): the unit is "items", e.g. files protected during setup.
//   - byte mode (NewBytes): the unit is bytes, e.g. bytes hashed during add and
//     import. Byte mode gives smooth within-file progress for large files, where
//     a file count would jump straight from 0 to 100%.
//
// Network transfers are not covered here: rclone renders its own --progress bar
// for push and pull.
//
// On a terminal the bar redraws in place with a carriage return on every update,
// so progress is live. When the writer is not a terminal (a pipe, a file, CI
// logs) a carriage return is useless, so instead a fresh progress line is
// written each time the whole-percent figure advances. This keeps a long-running
// job visibly live in logs while bounding output to ~100 lines, rather than
// dumping everything at the end.
type Bar struct {
	w        io.Writer
	label    string
	total    int64
	humanize bool
	enabled  bool
	isTTY    bool

	mu          sync.Mutex
	done        int64
	lastPercent int
	closed      bool
}

// New creates a count-mode bar advanced one item at a time with Step.
func New(w io.Writer, label string, total int, off bool) *Bar {
	return newBar(w, label, int64(total), false, off)
}

// NewBytes creates a byte-mode bar advanced by byte counts with Add. total is
// the number of bytes the operation will process.
func NewBytes(w io.Writer, label string, total int64, off bool) *Bar {
	return newBar(w, label, total, true, off)
}

func newBar(w io.Writer, label string, total int64, humanize, off bool) *Bar {
	b := &Bar{w: w, label: label, total: total, humanize: humanize}
	b.enabled = !off && w != nil && total > 0
	b.isTTY = isTerminal(w)
	return b
}

// Step advances a count-mode bar by one item.
func (b *Bar) Step() { b.Add(1) }

// Add advances the bar by n units (items or bytes). It is safe to call
// concurrently from multiple worker goroutines.
func (b *Bar) Add(n int) {
	if b == nil || !b.enabled || n <= 0 {
		return
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	b.done += int64(n)
	if b.done > b.total {
		// A byte total is a snapshot; if a file grew between sizing and hashing,
		// clamp so the bar never exceeds 100%.
		b.done = b.total
	}
	if b.isTTY {
		fmt.Fprint(b.w, "\r"+renderBar(b.label, b.done, b.total, b.humanize))
		return
	}
	// Non-terminal: emit a line each time the percentage advances (1%..99%), so
	// a long job stays visibly live in logs. The final 100% line is left to Close
	// to avoid a duplicate.
	if percent := b.percent(); percent > b.lastPercent && percent < 100 {
		b.lastPercent = percent
		fmt.Fprintln(b.w, b.statusLine(percent))
	}
}

// Close finishes the bar: it terminates the in-place line on a terminal, or
// writes a final status line otherwise. Calling Close more than once is safe.
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
	fmt.Fprintln(b.w, b.statusLine(b.percent()))
}

func (b *Bar) percent() int { return int(b.done * 100 / b.total) }

// statusLine is the non-terminal log form, e.g. "add 42% 3.0 MiB/7.1 MiB".
func (b *Bar) statusLine(percent int) string {
	return fmt.Sprintf("%s %d%% %s", b.label, percent, amounts(b.done, b.total, b.humanize))
}

// renderBar produces the bar body without the carriage return or trailing
// newline, e.g. "add [##########----------] 12.0 MB/24.0 MB".
func renderBar(label string, done, total int64, humanize bool) string {
	filled := int(done * barWidth / total)
	if filled > barWidth {
		filled = barWidth
	}
	bar := strings.Repeat("#", filled) + strings.Repeat("-", barWidth-filled)
	return fmt.Sprintf("%s [%s] %s", label, bar, amounts(done, total, humanize))
}

func amounts(done, total int64, humanize bool) string {
	if humanize {
		return HumanizeBytes(done) + "/" + HumanizeBytes(total)
	}
	return fmt.Sprintf("%d/%d", done, total)
}

// HumanizeBytes formats a byte count with a binary (1024-based) unit suffix.
func HumanizeBytes(n int64) string {
	const unit = 1024
	if n < unit {
		return fmt.Sprintf("%d B", n)
	}
	div, exp := int64(unit), 0
	for x := n / unit; x >= unit; x /= unit {
		div *= unit
		exp++
	}
	return fmt.Sprintf("%.1f %ciB", float64(n)/float64(div), "KMGTPE"[exp])
}

// Spinner is an indeterminate progress indicator for operations where the
// total work is unknown — typically a single remote metadata call. It prints
// elapsed seconds so the terminal does not look frozen.
//
// On a TTY it redraws in place with a carriage return every second. On a
// non-TTY it emits a log line every 10 seconds so long-running CI jobs stay
// visibly alive. Fast operations (< 1 s) produce no output at all.
type Spinner struct {
	enabled bool
	isTTY   bool
	label   string
	w       io.Writer
	done    chan struct{}
	wg      sync.WaitGroup
}

// NewSpinner starts and returns a running spinner. Call Stop when the
// operation finishes.
func NewSpinner(w io.Writer, label string, quiet bool) *Spinner {
	s := &Spinner{label: label, w: w, done: make(chan struct{})}
	s.enabled = !quiet && w != nil
	if !s.enabled {
		return s
	}
	s.isTTY = isTerminal(w)
	s.wg.Add(1)
	go s.run()
	return s
}

func (s *Spinner) run() {
	defer s.wg.Done()
	tick := time.NewTicker(time.Second)
	defer tick.Stop()
	var secs int
	for {
		select {
		case <-s.done:
			return
		case <-tick.C:
			secs++
			s.render(secs)
		}
	}
}

func (s *Spinner) render(secs int) {
	line := fmt.Sprintf("%s... (%ds)", s.label, secs)
	if s.isTTY {
		fmt.Fprint(s.w, "\r"+line)
		return
	}
	if secs%10 == 0 {
		fmt.Fprintln(s.w, line)
	}
}

// Stop terminates the spinner. On a TTY the spinner line is erased so the
// next output starts on a clean line.
func (s *Spinner) Stop() {
	if !s.enabled {
		return
	}
	close(s.done)
	s.wg.Wait()
	if s.isTTY {
		blank := strings.Repeat(" ", len(s.label)+12)
		fmt.Fprint(s.w, "\r"+blank+"\r")
	}
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

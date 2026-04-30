package progress

import (
	"fmt"
	"io"
	"strings"
)

type Bar struct {
	w       io.Writer
	label   string
	total   int
	off     bool
	updates chan struct{}
	done    chan struct{}
}

func New(w io.Writer, label string, total int, off bool) *Bar {
	b := &Bar{w: w, label: label, total: total, off: off}
	if off || w == nil || total <= 0 {
		return b
	}
	b.updates = make(chan struct{}, 1)
	b.done = make(chan struct{})
	go b.run()
	return b
}

func (b *Bar) Step() {
	if b == nil || b.off || b.updates == nil {
		return
	}
	b.updates <- struct{}{}
}

func (b *Bar) Close() {
	if b == nil || b.updates == nil {
		return
	}
	close(b.updates)
	<-b.done
}

func (b *Bar) run() {
	done := 0
	for range b.updates {
		done++
		percent := done * 100 / b.total
		filled := percent / 5
		if filled > 20 {
			filled = 20
		}
		fmt.Fprintf(b.w, "%s [%s%s] %d/%d\n", b.label, strings.Repeat("#", filled), strings.Repeat("-", 20-filled), done, b.total)
	}
	close(b.done)
}

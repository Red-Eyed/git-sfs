package progress

import (
	"bytes"
	"testing"
)

func TestRenderBarCountMode(t *testing.T) {
	cases := []struct {
		done, total int64
		want        string
	}{
		{1, 2, "push [##########----------] 1/2"},
		{2, 2, "push [####################] 2/2"},
		{0, 4, "push [--------------------] 0/4"},
	}
	for _, tc := range cases {
		if got := renderBar("push", tc.done, tc.total, false); got != tc.want {
			t.Fatalf("renderBar(%d,%d) = %q, want %q", tc.done, tc.total, got, tc.want)
		}
	}
}

func TestRenderBarByteMode(t *testing.T) {
	got := renderBar("add", 6<<20, 24<<20, true)
	want := "add [#####---------------] 6.0 MiB/24.0 MiB"
	if got != want {
		t.Fatalf("renderBar bytes = %q, want %q", got, want)
	}
}

func TestHumanizeBytes(t *testing.T) {
	cases := map[int64]string{
		0:       "0 B",
		512:     "512 B",
		1024:    "1.0 KiB",
		1536:    "1.5 KiB",
		1 << 20: "1.0 MiB",
		3 << 30: "3.0 GiB",
	}
	for n, want := range cases {
		if got := humanizeBytes(n); got != want {
			t.Fatalf("humanizeBytes(%d) = %q, want %q", n, got, want)
		}
	}
}

// A byte-mode bar to a non-terminal writer must not flood per-step; it writes a
// single humanized summary on Close.
func TestBytesBarNonTerminalSummaryOnly(t *testing.T) {
	var buf bytes.Buffer
	bar := NewBytes(&buf, "add", 3<<20, false)
	bar.Add(1 << 20)
	bar.Add(2 << 20)
	bar.Close()
	if got := buf.String(); got != "add 3.0 MiB/3.0 MiB\n" {
		t.Fatalf("summary = %q", got)
	}
}

func TestBarDisabledWritesNothing(t *testing.T) {
	var buf bytes.Buffer
	bar := New(&buf, "push", 2, true)
	bar.Step()
	bar.Step()
	bar.Close()
	if buf.Len() != 0 {
		t.Fatalf("expected no progress output, got %q", buf.String())
	}
}

func TestBarZeroTotalWritesNothing(t *testing.T) {
	var buf bytes.Buffer
	bar := New(&buf, "push", 0, false)
	bar.Step()
	bar.Close()
	if buf.Len() != 0 {
		t.Fatalf("expected no output for zero total, got %q", buf.String())
	}
}

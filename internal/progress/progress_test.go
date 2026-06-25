package progress

import (
	"bytes"
	"strings"
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
		if got := HumanizeBytes(n); got != want {
			t.Fatalf("HumanizeBytes(%d) = %q, want %q", n, got, want)
		}
	}
}

// A byte-mode bar to a non-terminal writer must stay live: it emits a line as
// the percentage advances, not one dump at the end. With a 100-byte total each
// byte is 1%, so stepping byte-by-byte produces a line per percent.
func TestBytesBarNonTerminalEmitsPeriodicProgress(t *testing.T) {
	var buf bytes.Buffer
	bar := NewBytes(&buf, "add", 100, false)
	for i := 0; i < 100; i++ {
		bar.Add(1)
	}
	bar.Close()
	out := buf.String()
	if !strings.Contains(out, "add 50% 50 B/100 B") {
		t.Fatalf("missing mid-run progress line: %q", out)
	}
	if !strings.Contains(out, "add 100% 100 B/100 B") {
		t.Fatalf("missing final line: %q", out)
	}
	// Liveness: progress is spread across many lines, not a single end dump.
	if n := strings.Count(out, "\n"); n < 50 {
		t.Fatalf("expected many progress lines for liveness, got %d: %q", n, out)
	}
}

// Output must stay bounded: the percentage throttle caps lines at ~100 even when
// stepped far more often than that.
func TestBytesBarNonTerminalBoundsOutput(t *testing.T) {
	var buf bytes.Buffer
	bar := NewBytes(&buf, "add", 100000, false)
	for i := 0; i < 100000; i++ {
		bar.Add(1)
	}
	bar.Close()
	if n := strings.Count(buf.String(), "\n"); n > 101 {
		t.Fatalf("expected at most ~100 lines, got %d", n)
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

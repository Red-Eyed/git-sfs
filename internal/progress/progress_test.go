package progress

import (
	"bytes"
	"testing"
)

func TestBarLineFormat(t *testing.T) {
	cases := []struct {
		done, total int
		want        string
	}{
		{1, 2, "push [##########----------] 1/2"},
		{2, 2, "push [####################] 2/2"},
		{0, 4, "push [--------------------] 0/4"},
	}
	for _, tc := range cases {
		if got := barLine("push", tc.done, tc.total); got != tc.want {
			t.Fatalf("barLine(%d,%d) = %q, want %q", tc.done, tc.total, got, tc.want)
		}
	}
}

// A non-terminal writer (a buffer) must not be flooded with per-step redraws;
// only a single summary line is written when the bar closes.
func TestBarNonTerminalWritesSummaryOnly(t *testing.T) {
	var buf bytes.Buffer
	bar := New(&buf, "add", 3, false)
	bar.Step()
	bar.Step()
	bar.Step()
	bar.Close()
	if got := buf.String(); got != "add 3/3\n" {
		t.Fatalf("non-terminal output = %q, want %q", got, "add 3/3\n")
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

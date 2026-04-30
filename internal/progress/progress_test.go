package progress

import (
	"bytes"
	"strings"
	"testing"
)

func TestBarWritesProgress(t *testing.T) {
	var buf bytes.Buffer
	bar := New(&buf, "push", 2, false)
	bar.Step()
	bar.Step()
	bar.Close()
	got := buf.String()
	if !strings.Contains(got, "push [##########----------] 1/2\n") {
		t.Fatalf("missing first progress update: %q", got)
	}
	if !strings.Contains(got, "push [####################] 2/2\n") {
		t.Fatalf("missing final progress update: %q", got)
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

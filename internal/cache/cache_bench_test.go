package cache

import (
	"os"
	"path/filepath"
	"testing"

	"git-sfs/internal/hash"
)

func BenchmarkStore8MiB(b *testing.B) {
	dir := b.TempDir()
	src := filepath.Join(dir, "src.bin")
	payload := benchPayload(8 << 20)
	if err := os.WriteFile(src, payload, 0o644); err != nil {
		b.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		b.Fatal(err)
	}
	c := Cache{Root: filepath.Join(dir, "cache")}
	if err := c.Init(); err != nil {
		b.Fatal(err)
	}
	dst := c.FilePath(h)
	b.SetBytes(int64(len(payload)))
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = os.Remove(dst)
		if err := c.Store(src, h); err != nil {
			b.Fatal(err)
		}
	}
}

func benchPayload(n int) []byte {
	buf := make([]byte, n)
	for i := range buf {
		buf[i] = byte(i % 251)
	}
	return buf
}

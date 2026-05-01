package remote

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"git-sfs/internal/hash"
)

func BenchmarkFilesystemPull8MiB(b *testing.B) {
	ctx := context.Background()
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
	r := NewFilesystem(filepath.Join(dir, "remote"))
	if err := r.PushFile(ctx, h, src); err != nil {
		b.Fatal(err)
	}
	dst := filepath.Join(dir, "dst.bin")
	b.SetBytes(int64(len(payload)))
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = os.Remove(dst)
		if err := r.PullFile(ctx, h, dst); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkRcloneHasFile8MiB(b *testing.B) {
	benchmarkRcloneCheck(b, false)
}

func BenchmarkRcloneCheckFile8MiB(b *testing.B) {
	benchmarkRcloneCheck(b, true)
}

func benchmarkRcloneCheck(b *testing.B, withIntegrity bool) {
	if runtime.GOOS == "windows" {
		b.Skip("shell benchmark helper is not used on windows")
	}
	ctx := context.Background()
	dir := b.TempDir()
	bin := filepath.Join(dir, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		b.Fatal(err)
	}
	writeBenchTool(b, filepath.Join(bin, "rclone"), `set -eu
if [ "${1:-}" = "--config" ]; then
  shift 2
fi
cmd="$1"
map_path() {
  case "$1" in
    testremote:*) printf '%s/%s\n' "$RCLONE_TEST_ROOT" "${1#testremote:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
case "$cmd" in
  copyto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    cp "$src" "$dst"
    ;;
  moveto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    mv "$src" "$dst"
    ;;
  *)
    exit 2
    ;;
esac
`)
	b.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	b.Setenv("RCLONE_TEST_ROOT", filepath.Join(dir, "remote"))
	src := filepath.Join(dir, "src.bin")
	payload := benchPayload(8 << 20)
	if err := os.WriteFile(src, payload, 0o644); err != nil {
		b.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		b.Fatal(err)
	}
	r := rcloneRemote{url: "testremote:dataset"}
	if err := r.PushFile(ctx, h, src); err != nil {
		b.Fatal(err)
	}
	b.SetBytes(int64(len(payload)))
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		if withIntegrity {
			ok, err := r.CheckFile(ctx, h)
			if err != nil {
				b.Fatal(err)
			}
			if !ok {
				b.Fatal("remote file missing")
			}
			continue
		}
		ok, err := r.HasFile(ctx, h)
		if err != nil {
			b.Fatal(err)
		}
		if !ok {
			b.Fatal("remote file missing")
		}
	}
}

func writeBenchTool(b *testing.B, path, body string) {
	b.Helper()
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		b.Fatal(err)
	}
}

func benchPayload(n int) []byte {
	buf := make([]byte, n)
	for i := range buf {
		buf[i] = byte(i % 251)
	}
	return buf
}

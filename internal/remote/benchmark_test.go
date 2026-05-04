package remote

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"git-sfs/internal/hash"
)

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
  copy)
    ignore_existing=false; files_from=""; shift
    while [ "$#" -gt 2 ]; do
      case "$1" in
        --ignore-existing) ignore_existing=true; shift ;;
        --files-from) files_from="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    src_base="$(map_path "$1")"; dst_base="$(map_path "$2")"
    while IFS= read -r rel; do
      [ -z "$rel" ] && continue
      src_file="${src_base}/${rel}"; dst_file="${dst_base}/${rel}"
      if $ignore_existing && [ -e "$dst_file" ]; then continue; fi
      mkdir -p "$(dirname "$dst_file")"
      cp "$src_file" "$dst_file"
    done < "$files_from" ;;
  lsjson)
    src="$(map_path "$2")"
    if [ -e "$src" ]; then
      printf '[{"Path":"%s"}]\n' "$(basename "$src")"
    else
      printf '[]\n'
    fi
    ;;
  *)
    exit 2
    ;;
esac
`)
	b.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	b.Setenv("RCLONE_TEST_ROOT", filepath.Join(dir, "remote"))
	payload := benchPayload(8 << 20)
	srcFile := filepath.Join(dir, "src.bin")
	if err := os.WriteFile(srcFile, payload, 0o644); err != nil {
		b.Fatal(err)
	}
	h, err := hash.File(srcFile)
	if err != nil {
		b.Fatal(err)
	}
	relPath := hash.Algorithm + "/" + h.Prefix() + "/" + h.String()
	cacheFilesDir := filepath.Join(dir, "cache-files")
	cachedFile := filepath.Join(cacheFilesDir, relPath)
	if err := os.MkdirAll(filepath.Dir(cachedFile), 0o755); err != nil {
		b.Fatal(err)
	}
	if err := os.WriteFile(cachedFile, payload, 0o644); err != nil {
		b.Fatal(err)
	}
	r := rcloneRemote{url: "testremote:dataset"}
	if err := r.CopyToRemote(ctx, cacheFilesDir, []string{relPath}); err != nil {
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

package core

import (
	"bytes"
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
	"git-sfs/internal/sfspath"
)

func TestPushPullRoundTrip(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("large bytes"))

	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := app(&bytes.Buffer{}).Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := os.Remove(cacheFile); err != nil {
			t.Fatal(err)
		}
		if err := app(&bytes.Buffer{}).Pull(context.Background(), "", "data/blob"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheFile, h); err != nil {
			t.Fatal(err)
		}
	})
}

func TestPushPullRoundTripWithLocalRcloneRemote(t *testing.T) {
	if _, err := exec.LookPath("rclone"); err != nil {
		t.Skip("rclone is not installed")
	}
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	if err := os.MkdirAll(remoteDir, 0o755); err != nil {
		t.Fatal(err)
	}
	mustWrite(t, filepath.Join(repo, ".git-sfs", "config.toml"), []byte("version = 1\n\n[remotes.default]\nbackend = local\npath = "+remoteDir+"\nconfig = "+filepath.Join(repo, ".git-sfs", "rclone.conf")+"\n\n[settings]\nalgorithm = sha256\n"))
	mustWrite(t, filepath.Join(repo, ".git-sfs", "rclone.conf"), []byte("[local]\ntype = local\n"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("large bytes"))

	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := app(&bytes.Buffer{}).Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := os.Remove(cacheFile); err != nil {
			t.Fatal(err)
		}
		if err := app(&bytes.Buffer{}).Pull(context.Background(), "", "data/blob"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheFile, h); err != nil {
			t.Fatal(err)
		}
		remoteFile := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := hash.VerifyFile(remoteFile, h); err != nil {
			t.Fatal(err)
		}
	})
}

func TestPullCanRestoreOnlySelectedFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "two.bin"), []byte("two"))

	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h1, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "one.bin"))
		if err != nil {
			t.Fatal(err)
		}
		h2, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "two.bin"))
		if err != nil {
			t.Fatal(err)
		}
		cacheOne := filepath.Join(cacheDir, "files", hash.Algorithm, h1.Prefix(), h1.String())
		cacheTwo := filepath.Join(cacheDir, "files", hash.Algorithm, h2.Prefix(), h2.String())
		if err := os.Remove(cacheOne); err != nil {
			t.Fatal(err)
		}
		if err := os.Remove(cacheTwo); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/one.bin"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheOne, h1); err != nil {
			t.Fatal(err)
		}
		if _, err := os.Stat(cacheTwo); !os.IsNotExist(err) {
			t.Fatalf("unselected cache file was restored: %v", err)
		}
	})
}

func TestPullWithMixedPresentAndMissingCacheFiles(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "nested", "two.bin"), []byte("two"))

	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h1, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "one.bin"))
		if err != nil {
			t.Fatal(err)
		}
		h2, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "nested", "two.bin"))
		if err != nil {
			t.Fatal(err)
		}
		cacheOne := filepath.Join(cacheDir, "files", hash.Algorithm, h1.Prefix(), h1.String())
		cacheTwo := filepath.Join(cacheDir, "files", hash.Algorithm, h2.Prefix(), h2.String())
		if err := os.Remove(cacheTwo); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheOne, h1); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheTwo, h2); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/one.bin"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheOne, h1); err != nil {
			t.Fatal(err)
		}
	})
}

func TestPullFailsForMissingRemoteFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		if err := os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/blob"); err == nil {
			t.Fatal("expected missing remote file error")
		}
	})
}

func TestSelectedRemoteErrors(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), "missing"); err == nil {
			t.Fatal("expected missing remote error")
		}
	})
}

func TestPullFailsWhenDiskSpaceInsufficient(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	// Use a fake rclone that reports an impossibly large file size so the guard fires.
	bin := t.TempDir()
	writeTool(t, filepath.Join(bin, "rclone"), `set -eu
if [ "${1:-}" = "--config" ]; then shift 2; fi
cmd="${1:-}"
map_path() {
  case "$1" in
    localtest:*) printf '%s%s\n' "$RCLONE_TEST_ROOT" "${1#localtest:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
case "$cmd" in
  copyto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    cp "$src" "$dst" ;;
  lsjson)
    src="$(map_path "$2")"
    if [ -e "$src" ]; then
      printf '[{"Path":"%s","Size":999999999999999}]\n' "$(basename "$src")"
    else
      printf '[]\n'
    fi ;;
  moveto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    mv "$src" "$dst" ;;
  lsd)
    src="$(map_path "$2")"
    if [ -d "$src" ]; then exit 0; else printf 'directory not found: %s\n' "$src" >&2; exit 1; fi ;;
  *) exit 2 ;;
esac
`)
	remoteDir := filepath.Join(t.TempDir(), "remote")
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("RCLONE_TEST_ROOT", remoteDir)
	content := "version = 1\n\n[remotes.default]\nbackend = localtest\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))

	// Push using the standard fake (need a separate setup to push first).
	// Easier: manually create the remote file so pull has something to pull.
	writeDataset(t, repo, remoteDir) // sets up config and standard fake rclone temporarily
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		// Remove cache so pull will try to fetch.
		if err := os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())); err != nil {
			t.Fatal(err)
		}
		// Now override PATH to use the huge-size fake rclone.
		t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
		err = app(&bytes.Buffer{}).Pull(context.Background(), "", ".")
		if err == nil {
			t.Fatal("expected disk space error")
		}
		if !strings.Contains(err.Error(), "disk space") {
			t.Fatalf("unexpected error: %v", err)
		}
	})
}

func TestPushFailsWhenRcloneNotOnPath(t *testing.T) {
	repo := newRepo(t)
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir) // sets config.toml and a fake rclone on PATH
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	// Override PATH to an empty directory so rclone cannot be found.
	emptyBin := t.TempDir()
	t.Setenv("PATH", emptyBin)
	inDir(t, repo, func() {
		err := app(&bytes.Buffer{}).Push(context.Background(), "")
		if err == nil {
			t.Fatal("expected error when rclone is not on PATH")
		}
		if !strings.Contains(err.Error(), "not found") && !strings.Contains(err.Error(), "no such file") {
			t.Fatalf("unexpected error: %v", err)
		}
	})
}

func TestPushFailsForMissingRemotePath(t *testing.T) {
	repo := newRepo(t)
	// Set up with a valid remote so that Add works, then switch RCLONE_TEST_ROOT
	// to a non-existent path before Push to exercise the RequireExists guard.
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		// Point RCLONE_TEST_ROOT at a path that does not exist.
		t.Setenv("RCLONE_TEST_ROOT", filepath.Join(t.TempDir(), "nonexistent"))
		err := a.Push(context.Background(), "")
		if err == nil {
			t.Fatal("expected error when remote root does not exist")
		}
		if !strings.Contains(err.Error(), "does not exist") {
			t.Fatalf("unexpected error: %v", err)
		}
	})
}

func TestPushSkipsExistingRemoteFileAndRejectsMissingCache(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		if err := os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err == nil {
			t.Fatal("expected missing cache file error")
		}
	})
}

func TestPullSkipsExistingValidCacheFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/blob"); err != nil {
			t.Fatal(err)
		}
	})
}

func TestPullFailsForMissingRemotePath(t *testing.T) {
	repo := newRepo(t)
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		t.Setenv("RCLONE_TEST_ROOT", filepath.Join(t.TempDir(), "nonexistent"))
		err := a.Pull(context.Background(), "", ".")
		if err == nil {
			t.Fatal("expected error when remote root does not exist")
		}
		if !strings.Contains(err.Error(), "does not exist") {
			t.Fatalf("unexpected error: %v", err)
		}
	})
}

func TestPullRejectsHashMismatch(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	bin := filepath.Join(t.TempDir(), "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	// Fake rclone: copy writes wrong content for every file in the list.
	writeTool(t, filepath.Join(bin, "rclone"), `set -eu
if [ "${1:-}" = "--config" ]; then shift 2; fi
cmd="${1:-}"
map_path() {
  case "$1" in
    localtest:*) printf '%s%s\n' "$RCLONE_TEST_ROOT" "${1#localtest:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
case "$cmd" in
  copy)
    files_from=""; shift
    while [ "$#" -gt 2 ]; do
      case "$1" in
        --ignore-existing) shift ;;
        --files-from) files_from="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    dst_base="$2"
    while IFS= read -r rel; do
      [ -z "$rel" ] && continue
      dst_file="$(map_path "${dst_base}/${rel}")"
      mkdir -p "$(dirname "$dst_file")"
      printf 'wrong content\n' > "$dst_file"
    done < "$files_from" ;;
  lsjson)
    src="$(map_path "$2")"
    if [ -f "$src" ]; then
      size=$(wc -c < "$src" | tr -d ' \t')
      printf '[{"Path":"%s","Size":%s}]\n' "$(basename "$src")" "$size"
    elif [ -e "$src" ]; then
      printf '[{"Path":"%s","Size":0}]\n' "$(basename "$src")"
    else
      printf 'directory not found: %s\n' "$src" >&2; exit 1
    fi ;;
  lsd)
    src="$(map_path "$2")"
    if [ -d "$src" ]; then exit 0; else printf 'directory not found: %s\n' "$src" >&2; exit 1; fi ;;
  *) exit 2 ;;
esac
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("RCLONE_TEST_ROOT", remoteDir)
	if err := os.MkdirAll(remoteDir, 0o755); err != nil {
		t.Fatal(err)
	}
	content := "version = 1\n\n[remotes.default]\nbackend = localtest\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm)
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		cacheFile = filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := os.Remove(cacheFile); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/blob"); err == nil {
			t.Fatal("expected error on hash mismatch")
		}
	})
}

func TestPushFailsForMissingCacheFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		if err := os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())); err != nil {
			t.Fatal(err)
		}
		err = a.Push(context.Background(), "")
		if !errors.Is(err, errs.ErrMissingCachedFile) {
			t.Fatalf("expected ErrMissingCachedFile, got: %v", err)
		}
	})
}

package core

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"git-sfs/internal/hash"
	"git-sfs/internal/sfspath"
)

func TestAddVerifyDematerializeMaterializeAndStatus(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "nested", "two.bin"), []byte("two"))

	stdout := &bytes.Buffer{}
	app := app(stdout)
	inDir(t, repo, func() {
		if err := app.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		if err := app.Verify(context.Background()); err != nil {
			t.Fatal(err)
		}
		if err := app.Status(context.Background()); err != nil {
			t.Fatal(err)
		}
	})

	for _, rel := range []string{"data/one.bin", "data/nested/two.bin"} {
		path := filepath.Join(repo, rel)
		info, err := os.Lstat(path)
		if err != nil {
			t.Fatal(err)
		}
		if info.Mode()&os.ModeSymlink == 0 {
			t.Fatalf("%s is not a symlink", rel)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, path)
		if err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String()), h); err != nil {
			t.Fatal(err)
		}
	}
}

func TestPushPullRoundTripWithFilesystemRemote(t *testing.T) {
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
		if err := app(&bytes.Buffer{}).Pull(context.Background(), "data/blob"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheFile, h); err != nil {
			t.Fatal(err)
		}
	})
}

func TestPushUsesParallelRcloneUploads(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteRoot := filepath.Join(t.TempDir(), "remote")
	bin := filepath.Join(t.TempDir(), "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	writeTool(t, filepath.Join(bin, "rclone"), `set -eu
if [ "${1:-}" = "--config" ]; then
  shift 2
fi
cmd="$1"
src="$2"
dst="$3"
map_path() {
  case "$1" in
    testremote:*) printf '%s/%s\n' "$RCLONE_TEST_ROOT" "${1#testremote:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
src="$(map_path "$src")"
dst="$(map_path "$dst")"
case "$cmd" in
  copyto)
    case "$src" in
      "$RCLONE_TEST_ROOT"/*)
      mkdir -p "$(dirname "$dst")"
      cp "$src" "$dst"
        ;;
      *)
      printf 'start %s\n' "$src" >> "$RCLONE_TEST_LOG"
      sleep 1
      mkdir -p "$(dirname "$dst")"
      cp "$src" "$dst"
      printf 'end %s\n' "$src" >> "$RCLONE_TEST_LOG"
        ;;
    esac
    ;;
  moveto)
    mkdir -p "$(dirname "$dst")"
    mv "$src" "$dst"
    ;;
  *)
    exit 2
    ;;
esac
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("RCLONE_TEST_ROOT", remoteRoot)
	logPath := filepath.Join(t.TempDir(), "rclone.log")
	t.Setenv("RCLONE_TEST_LOG", logPath)
	writeRcloneDataset(t, repo, "testremote", "dataset")
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "two.bin"), []byte("two"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		start := time.Now()
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		if time.Since(start) > 1800*time.Millisecond {
			t.Fatalf("push took too long to be parallel: %s", time.Since(start))
		}
	})
	log, err := os.ReadFile(logPath)
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(string(log)), "\n")
	if len(lines) < 4 {
		t.Fatalf("unexpected rclone log:\n%s", log)
	}
	if !strings.HasPrefix(lines[0], "start ") || !strings.HasPrefix(lines[1], "start ") {
		t.Fatalf("uploads did not start in parallel:\n%s", log)
	}
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
		if err := a.Pull(context.Background(), "data/one.bin"); err != nil {
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

func TestMoveFileIntoCacheWithoutCopyingToRepo(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	src := filepath.Join(t.TempDir(), "outside.bin")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, src, []byte("large payload"))
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Import(context.Background(), src, "data/blob.bin"); err != nil {
			t.Fatal(err)
		}
	})
	if _, err := os.Stat(src); !os.IsNotExist(err) {
		t.Fatalf("source still exists after move: %v", err)
	}
	dst := filepath.Join(repo, "data", "blob.bin")
	info, err := os.Lstat(dst)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode()&os.ModeSymlink == 0 {
		t.Fatal("destination should be a symlink")
	}
	h, _, err := sfspath.ParseGitSymlink(repo, dst)
	if err != nil {
		t.Fatal(err)
	}
	cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
	if err := hash.VerifyFile(cacheFile, h); err != nil {
		t.Fatal(err)
	}
	if info, err := os.Stat(cacheFile); err != nil || info.Mode().Perm()&0o222 != 0 {
		t.Fatalf("cache file should exist and be read-only: info=%v err=%v", info, err)
	}
}

func TestImportResolvesSourceFileSymlink(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	src := filepath.Join(t.TempDir(), "outside.bin")
	link := filepath.Join(t.TempDir(), "outside-link.bin")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, src, []byte("large payload"))
	if err := os.Symlink(src, link); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).ImportWithOptions(context.Background(), link, "data/blob.bin", ImportOptions{FollowSymlinks: true}); err != nil {
			t.Fatal(err)
		}
	})
	if _, err := os.Lstat(link); !os.IsNotExist(err) {
		t.Fatalf("source symlink should be removed after import: %v", err)
	}
	if _, err := os.Stat(src); !os.IsNotExist(err) {
		t.Fatalf("resolved source should be moved into cache: %v", err)
	}
	dst := filepath.Join(repo, "data", "blob.bin")
	h, _, err := sfspath.ParseGitSymlink(repo, dst)
	if err != nil {
		t.Fatal(err)
	}
	cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
	if err := hash.VerifyFile(cacheFile, h); err != nil {
		t.Fatal(err)
	}
}

func TestImportRejectsSourceSymlinkWithoutFollowFlag(t *testing.T) {
	repo := newRepo(t)
	src := filepath.Join(t.TempDir(), "outside.bin")
	link := filepath.Join(t.TempDir(), "outside-link.bin")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, src, []byte("large payload"))
	if err := os.Symlink(src, link); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Import(context.Background(), link, "data/blob.bin"); err == nil {
			t.Fatal("expected source symlink import without -L to fail")
		}
	})
	if _, err := os.Lstat(link); err != nil {
		t.Fatalf("source symlink should remain after failed import: %v", err)
	}
	if _, err := os.Stat(src); err != nil {
		t.Fatalf("resolved source should remain after failed import: %v", err)
	}
}

func TestMoveDirectoryIntoCache(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	srcDir := filepath.Join(t.TempDir(), "incoming")
	linkedSrc := filepath.Join(t.TempDir(), "linked.bin")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(srcDir, "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(srcDir, "nested", "two.bin"), []byte("two"))
	mustWrite(t, linkedSrc, []byte("linked"))
	if err := os.Symlink(linkedSrc, filepath.Join(srcDir, "nested", "linked.bin")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(srcDir, "one.bin"), filepath.Join(srcDir, "nested", "one-link.bin")); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).ImportWithOptions(context.Background(), srcDir, "data/imported", ImportOptions{FollowSymlinks: true}); err != nil {
			t.Fatal(err)
		}
	})
	if _, err := os.Stat(srcDir); !os.IsNotExist(err) {
		t.Fatalf("source directory should be removed when empty: %v", err)
	}
	if _, err := os.Stat(linkedSrc); !os.IsNotExist(err) {
		t.Fatalf("nested symlink target should be moved into cache: %v", err)
	}
	for _, rel := range []string{"data/imported/one.bin", "data/imported/nested/two.bin", "data/imported/nested/linked.bin", "data/imported/nested/one-link.bin"} {
		info, err := os.Lstat(filepath.Join(repo, rel))
		if err != nil || info.Mode()&os.ModeSymlink == 0 {
			t.Fatalf("%s should be a symlink: info=%v err=%v", rel, info, err)
		}
	}
}

func TestImportResolvesSourceDirectorySymlink(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	srcDir := filepath.Join(t.TempDir(), "incoming")
	link := filepath.Join(t.TempDir(), "incoming-link")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(srcDir, "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(srcDir, "nested", "two.bin"), []byte("two"))
	if err := os.Symlink(srcDir, link); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).ImportWithOptions(context.Background(), link, "data/imported", ImportOptions{FollowSymlinks: true}); err != nil {
			t.Fatal(err)
		}
	})
	if _, err := os.Lstat(link); !os.IsNotExist(err) {
		t.Fatalf("source symlink should be removed after import: %v", err)
	}
	if _, err := os.Stat(srcDir); !os.IsNotExist(err) {
		t.Fatalf("resolved source directory should be removed when empty: %v", err)
	}
	for _, rel := range []string{"data/imported/one.bin", "data/imported/nested/two.bin"} {
		info, err := os.Lstat(filepath.Join(repo, rel))
		if err != nil || info.Mode()&os.ModeSymlink == 0 {
			t.Fatalf("%s should be a symlink: info=%v err=%v", rel, info, err)
		}
	}
}

func TestStatusReportsUnconvertedAndCorruptCache(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("large bytes"))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		if err := app(stdout).Status(context.Background()); err == nil {
			t.Fatal("status should fail for unconverted file")
		}
		if !strings.Contains(stdout.String(), "unconverted files: 1") ||
			!strings.Contains(stdout.String(), "unconverted file: data/blob") {
			t.Fatalf("status did not report unconverted file: %q", stdout.String())
		}
		stdout.Reset()
		if err := app(stdout).Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := os.Chmod(cacheFile, 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(cacheFile, []byte("corrupt"), 0o644); err != nil {
			t.Fatal(err)
		}
		stdout.Reset()
		if err := app(stdout).Verify(context.Background()); err == nil {
			t.Fatal("verify should fail for corrupt cache file")
		}
		if !strings.Contains(stdout.String(), "corrupt cache files: 1") ||
			!strings.Contains(stdout.String(), "corrupt cache file: data/blob") {
			t.Fatalf("verify did not report corrupt cache: %q", stdout.String())
		}
	})
}

func TestGCDoesNotDeleteReferencedFiles(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("live"))

	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		dead := filepath.Join(cacheDir, "files", hash.Algorithm, "ff", strings.Repeat("f", 64))
		if err := os.MkdirAll(filepath.Dir(dead), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(dead, []byte("dead"), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := app(&bytes.Buffer{}).GC(context.Background(), GCOptions{Files: true}); err != nil {
			t.Fatal(err)
		}
		if _, err := os.Stat(dead); !os.IsNotExist(err) {
			t.Fatalf("unreferenced file was not removed: %v", err)
		}
		live := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := hash.VerifyFile(live, h); err != nil {
			t.Fatal(err)
		}
	})
}

func TestInitSetupAndGitignore(t *testing.T) {
	repo := newRepo(t)
	inDir(t, repo, func() {
		stdout := &bytes.Buffer{}
		a := app(stdout)
		if err := a.Init(context.Background(), false); err != nil {
			t.Fatal(err)
		}
		if err := a.Setup(context.Background()); err != nil {
			t.Fatal(err)
		}
		if target, err := os.Readlink(filepath.Join(repo, ".git-sfs", "cache")); err != nil || target == "" {
			t.Fatalf("cache symlink missing: target=%q err=%v", target, err)
		}
		if info, err := os.Stat(filepath.Join(repo, ".git-sfs", ".cache", "files")); err != nil || !info.IsDir() {
			t.Fatalf("default cache missing: %v", err)
		}
		if err := a.Init(context.Background(), false); err == nil {
			t.Fatal("init should not overwrite config")
		}
		if err := a.Init(context.Background(), true); err != nil {
			t.Fatal(err)
		}
		gitignore, err := os.ReadFile(filepath.Join(repo, ".gitignore"))
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(string(gitignore), ".git-sfs/cache") {
			t.Fatalf(".gitignore missing .git-sfs/: %q", gitignore)
		}
	})
}

func TestStatusReportsInvalidConfig(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	content := "version = 1\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
	writeLocal(t, repo, cacheDir)
	inDir(t, repo, func() {
		stdout := &bytes.Buffer{}
		if err := app(stdout).Status(context.Background()); err == nil {
			t.Fatal("expected invalid config status")
		}
		if !strings.Contains(stdout.String(), "missing default remote") {
			t.Fatalf("missing status output: %q", stdout.String())
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
		if err := a.Pull(context.Background(), "data/blob"); err == nil {
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

func TestMaterializeIgnoresNonGitSFSSymlinkSelection(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	if err := os.Symlink("elsewhere", filepath.Join(repo, "link")); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Materialize(context.Background(), "."); err != nil {
			t.Fatal(err)
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
		if err := a.Pull(context.Background(), "data/blob"); err != nil {
			t.Fatal(err)
		}
	})
}

func app(stdout *bytes.Buffer) App {
	return App{
		Stdout:     stdout,
		Stderr:     &bytes.Buffer{},
		ConfigPath: ".git-sfs/config.toml",
	}
}

func newRepo(t *testing.T) string {
	t.Helper()
	repo := t.TempDir()
	if err := os.Mkdir(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	return repo
}

func writeDataset(t *testing.T, repo, remote string) {
	t.Helper()
	content := "version = 1\n\n[remotes.default]\ntype = filesystem\nurl = " + remote + "\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
}

func writeRcloneDataset(t *testing.T, repo, host, path string) {
	t.Helper()
	content := "version = 1\n\n[remotes.default]\ntype = rclone\nhost = " + host + "\npath = " + path + "\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
}

func writeLocal(t *testing.T, repo, cacheDir string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Join(repo, ".git-sfs"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(cacheDir, filepath.Join(repo, ".git-sfs", "cache")); err != nil {
		t.Fatal(err)
	}
}

func mustWrite(t *testing.T, path string, content []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, content, 0o644); err != nil {
		t.Fatal(err)
	}
}

func writeTool(t *testing.T, path, body string) {
	t.Helper()
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		t.Fatal(err)
	}
}

func inDir(t *testing.T, dir string, fn func()) {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chdir(dir); err != nil {
		t.Fatal(err)
	}
	defer func() {
		if err := os.Chdir(wd); err != nil {
			t.Fatal(err)
		}
	}()
	fn()
}

package cli

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestHelp(t *testing.T) {
	var stdout, stderr bytes.Buffer
	if err := run(context.Background(), []string{"help"}, &stdout, &stderr); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(stdout.String(), "commands:") {
		t.Fatalf("help output missing commands: %q", stdout.String())
	}
}

func TestRunHelp(t *testing.T) {
	if err := Run(context.Background(), []string{"help"}); err != nil {
		t.Fatal(err)
	}
}

func TestUnknownCommandFails(t *testing.T) {
	var stdout, stderr bytes.Buffer
	if err := run(context.Background(), []string{"wat"}, &stdout, &stderr); err == nil {
		t.Fatal("expected unknown command to fail")
	}
}

func TestCommandDispatch(t *testing.T) {
	repo := t.TempDir()
	if err := os.Mkdir(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		var stdout, stderr bytes.Buffer
		if err := run(context.Background(), []string{"init"}, &stdout, &stderr); err != nil {
			t.Fatal(err)
		}
		remoteDir := filepath.Join(t.TempDir(), "remote")
		dataset := "version = 1\n\n[remotes.default]\ntype = filesystem\nurl = " + remoteDir + "\n\n[settings]\nalgorithm = sha256\n"
		if err := os.WriteFile(filepath.Join(repo, ".git-sfs/config.toml"), []byte(dataset), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.MkdirAll(filepath.Join(repo, "data"), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(repo, "data", "blob"), []byte("payload"), 0o644); err != nil {
			t.Fatal(err)
		}
		for _, args := range [][]string{
			{"setup"},
			{"add", "data/blob"},
			{"status"},
			{"verify"},
			{"push"},
			{"pull", "data/blob"},
			{"gc", "--dry-run"},
		} {
			if err := run(context.Background(), args, &stdout, &stderr); err != nil {
				t.Fatalf("%v: %v stderr=%s stdout=%s", args, err, stderr.String(), stdout.String())
			}
		}
	})
}

func TestCommandDispatchImport(t *testing.T) {
	repo := t.TempDir()
	if err := os.Mkdir(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	src := filepath.Join(t.TempDir(), "outside.bin")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		var stdout, stderr bytes.Buffer
		if err := run(context.Background(), []string{"init"}, &stdout, &stderr); err != nil {
			t.Fatal(err)
		}
		if err := run(context.Background(), []string{"import", src, "data/blob"}, &stdout, &stderr); err != nil {
			t.Fatalf("import failed: %v stderr=%s stdout=%s", err, stderr.String(), stdout.String())
		}
		info, err := os.Lstat(filepath.Join(repo, "data", "blob"))
		if err != nil || info.Mode()&os.ModeSymlink == 0 {
			t.Fatalf("import should create a destination symlink: info=%v err=%v", info, err)
		}
	})
}

func TestImportRequiresSourceAndDestination(t *testing.T) {
	var stdout, stderr bytes.Buffer
	if err := run(context.Background(), []string{"import", "only-one"}, &stdout, &stderr); err == nil {
		t.Fatal("expected import without source and destination to fail")
	}
}

func TestImportParsesFollowSymlinkFlag(t *testing.T) {
	repo := t.TempDir()
	if err := os.Mkdir(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	src := filepath.Join(dir, "outside.bin")
	link := filepath.Join(dir, "outside-link.bin")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(src, link); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		var stdout, stderr bytes.Buffer
		if err := run(context.Background(), []string{"init"}, &stdout, &stderr); err != nil {
			t.Fatal(err)
		}
		if err := run(context.Background(), []string{"import", "-L", link, "data/blob"}, &stdout, &stderr); err != nil {
			t.Fatalf("import -L failed: %v stderr=%s stdout=%s", err, stderr.String(), stdout.String())
		}
		info, err := os.Lstat(filepath.Join(repo, "data", "blob"))
		if err != nil || info.Mode()&os.ModeSymlink == 0 {
			t.Fatalf("import -L should create a destination symlink: info=%v err=%v", info, err)
		}
	})
}

func TestAddRequiresPath(t *testing.T) {
	var stdout, stderr bytes.Buffer
	if err := run(context.Background(), []string{"add"}, &stdout, &stderr); err == nil {
		t.Fatal("expected add without paths to fail")
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

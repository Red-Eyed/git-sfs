package cli

import (
	"archive/tar"
	"archive/zip"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"

	"git-sfs/internal/progress"
	"git-sfs/internal/version"
)

// selfEnv holds the env-var configuration for self-update.
// It mirrors the surface of install.sh exactly so corporate users who already
// configured these vars for the initial install get self-update for free.
// HTTP_PROXY / HTTPS_PROXY / NO_PROXY are honored automatically via
// http.DefaultTransport's ProxyFromEnvironment — no explicit handling needed.
type selfEnv struct {
	repo             string // GIT_SFS_REPO
	releaseBaseURL   string // GIT_SFS_RELEASE_BASE_URL
	releaseLatestURL string // GIT_SFS_RELEASE_LATEST_URL
	rcloneBaseURL    string // GIT_SFS_RCLONE_BASE_URL
	caFile           string // GIT_SFS_SSL_CERT_FILE | SSL_CERT_FILE | CURL_CA_BUNDLE
	insecureTLS      bool   // GIT_SFS_INSECURE_TLS=1
}

func loadSelfEnv() selfEnv {
	repo := coalesce(os.Getenv("GIT_SFS_REPO"), "Red-Eyed/git-sfs")
	e := selfEnv{
		repo: repo,
		rcloneBaseURL: coalesce(
			os.Getenv("GIT_SFS_RCLONE_BASE_URL"),
			"https://downloads.rclone.org",
		),
		caFile: coalesce(
			os.Getenv("GIT_SFS_SSL_CERT_FILE"),
			os.Getenv("SSL_CERT_FILE"),
			os.Getenv("CURL_CA_BUNDLE"),
		),
		insecureTLS: os.Getenv("GIT_SFS_INSECURE_TLS") == "1",
	}
	e.releaseBaseURL = coalesce(
		os.Getenv("GIT_SFS_RELEASE_BASE_URL"),
		"https://github.com/"+repo+"/releases/download",
	)
	e.releaseLatestURL = coalesce(
		os.Getenv("GIT_SFS_RELEASE_LATEST_URL"),
		"https://github.com/"+repo+"/releases/latest",
	)
	return e
}

// coalesce returns the first non-empty string.
func coalesce(vals ...string) string {
	for _, v := range vals {
		if v != "" {
			return v
		}
	}
	return ""
}

// newHTTPClient builds a client with TLS settings from env.
// It clones DefaultTransport to preserve proxy and dialer configuration.
func (e selfEnv) newHTTPClient() (*http.Client, error) {
	base := http.DefaultTransport.(*http.Transport).Clone()
	cfg := &tls.Config{}

	if e.caFile != "" {
		pem, err := os.ReadFile(e.caFile)
		if err != nil {
			return nil, fmt.Errorf("reading CA bundle %s: %w", e.caFile, err)
		}
		pool := x509.NewCertPool()
		if !pool.AppendCertsFromPEM(pem) {
			return nil, fmt.Errorf("no valid certificates in %s", e.caFile)
		}
		cfg.RootCAs = pool
	}
	if e.insecureTLS {
		cfg.InsecureSkipVerify = true //nolint:gosec // user-controlled opt-in
	}

	base.TLSClientConfig = cfg
	return &http.Client{Transport: base}, nil
}

// selfUpdate updates git-sfs and rclone binaries to their latest releases.
// Both are replaced atomically (temp file + rename). Each binary reports
// "already up to date" if it is already at the latest version.
func selfUpdate(ctx context.Context, stdout, stderr io.Writer, quiet bool) error {
	env := loadSelfEnv()

	if env.caFile != "" {
		fmt.Fprintf(stderr, "using TLS CA bundle from %s\n", env.caFile)
	}
	if env.insecureTLS {
		fmt.Fprintln(stderr, "warning: GIT_SFS_INSECURE_TLS=1 disables TLS certificate verification")
	}

	client, err := env.newHTTPClient()
	if err != nil {
		return err
	}

	exePath, err := resolveExe()
	if err != nil {
		return err
	}
	installDir := filepath.Dir(exePath)

	if err := updateGitSFS(ctx, client, env, exePath, stdout, stderr, quiet); err != nil {
		return err
	}
	return updateRclone(ctx, client, env, filepath.Join(installDir, "rclone"), stdout, stderr, quiet)
}

func updateGitSFS(ctx context.Context, client *http.Client, env selfEnv, destPath string, stdout, stderr io.Writer, quiet bool) error {
	spin := progress.NewSpinner(stderr, "checking git-sfs version", quiet)
	latest, err := latestGitSFSVersion(ctx, client, env.releaseLatestURL)
	spin.Stop()
	if err != nil {
		return fmt.Errorf("git-sfs: cannot reach release server: %w", err)
	}

	current := version.Version
	if current == latest {
		fmt.Fprintf(stdout, "git-sfs %s already up to date\n", current)
		return nil
	}

	asset := fmt.Sprintf("git-sfs-%s-%s-%s.tar.gz", latest, runtime.GOOS, runtime.GOARCH)
	url := env.releaseBaseURL + "/" + latest + "/" + asset

	label := fmt.Sprintf("downloading git-sfs %s", latest)
	data, err := fetchBody(ctx, client, url, label, stderr, quiet)
	if err != nil {
		return fmt.Errorf("git-sfs: download failed (%s): %w", url, err)
	}

	binary, err := extractFromTar(data, "git-sfs")
	if err != nil {
		return fmt.Errorf("git-sfs: %w", err)
	}
	if err := atomicReplace(destPath, binary); err != nil {
		return fmt.Errorf("git-sfs: install failed: %w", err)
	}
	fmt.Fprintf(stdout, "git-sfs %s → %s\n", current, latest)
	return nil
}

func updateRclone(ctx context.Context, client *http.Client, env selfEnv, destPath string, stdout, stderr io.Writer, quiet bool) error {
	spin := progress.NewSpinner(stderr, "checking rclone version", quiet)
	latest, err := latestRcloneVersion(ctx, client, env.rcloneBaseURL)
	spin.Stop()
	if err != nil {
		return fmt.Errorf("rclone: cannot reach release server: %w", err)
	}

	current, _ := currentRcloneVersion(destPath)
	if current == latest {
		fmt.Fprintf(stdout, "rclone %s already up to date\n", current)
		return nil
	}

	rcloneOS := runtime.GOOS
	if rcloneOS == "darwin" {
		rcloneOS = "osx"
	}
	asset := fmt.Sprintf("rclone-%s-%s-%s.zip", latest, rcloneOS, runtime.GOARCH)
	url := env.rcloneBaseURL + "/" + latest + "/" + asset

	label := fmt.Sprintf("downloading rclone %s", latest)
	data, err := fetchBody(ctx, client, url, label, stderr, quiet)
	if err != nil {
		return fmt.Errorf("rclone: download failed (%s): %w", url, err)
	}

	binary, err := extractFromZip(data, "rclone")
	if err != nil {
		return fmt.Errorf("rclone: %w", err)
	}
	if err := atomicReplace(destPath, binary); err != nil {
		return fmt.Errorf("rclone: install failed: %w", err)
	}

	if current == "" {
		fmt.Fprintf(stdout, "rclone %s installed\n", latest)
	} else {
		fmt.Fprintf(stdout, "rclone %s → %s\n", current, latest)
	}
	return nil
}

// fetchBody downloads url, showing a byte-mode progress bar when the server
// sends Content-Length or a spinner otherwise. All bytes are returned in memory
// (zip extraction requires a seekable reader; tar extraction is also buffered
// for simplicity since the blobs are typically 10–50 MB).
func fetchBody(ctx context.Context, client *http.Client, url, label string, w io.Writer, quiet bool) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("HTTP %d", resp.StatusCode)
	}

	var body io.Reader = resp.Body
	size := resp.ContentLength // -1 when unknown

	var bar *progress.Bar
	var spin *progress.Spinner
	if size > 0 {
		bar = progress.NewBytes(w, label, size, quiet)
		body = &countReader{r: resp.Body, bar: bar}
	} else {
		spin = progress.NewSpinner(w, label, quiet)
	}

	data, readErr := io.ReadAll(body)

	if bar != nil {
		bar.Close()
	}
	if spin != nil {
		spin.Stop()
	}
	if readErr != nil {
		return nil, fmt.Errorf("reading response: %w", readErr)
	}
	return data, nil
}

// countReader wraps a reader and advances a progress.Bar as bytes are read.
type countReader struct {
	r   io.Reader
	bar *progress.Bar
}

func (cr *countReader) Read(p []byte) (int, error) {
	n, err := cr.r.Read(p)
	cr.bar.Add(n)
	return n, err
}

// latestGitSFSVersion resolves the latest release tag by following the
// GitHub /releases/latest redirect and reading the final path segment.
func latestGitSFSVersion(ctx context.Context, client *http.Client, latestURL string) (string, error) {
	noRedir := *client
	noRedir.CheckRedirect = func(*http.Request, []*http.Request) error {
		return http.ErrUseLastResponse
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodHead, latestURL, nil)
	if err != nil {
		return "", err
	}
	resp, err := noRedir.Do(req)
	if err != nil {
		return "", err
	}
	resp.Body.Close()

	loc := resp.Header.Get("Location")
	if loc == "" {
		return "", fmt.Errorf("no redirect from %s", latestURL)
	}
	parts := strings.Split(strings.TrimRight(loc, "/"), "/")
	tag := parts[len(parts)-1]
	if tag == "" {
		return "", fmt.Errorf("could not parse version from redirect: %s", loc)
	}
	return tag, nil
}

// latestRcloneVersion fetches the latest stable rclone version from version.txt.
func latestRcloneVersion(ctx context.Context, client *http.Client, baseURL string) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, baseURL+"/version.txt", nil)
	if err != nil {
		return "", err
	}
	resp, err := client.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("HTTP %d from %s/version.txt", resp.StatusCode, baseURL)
	}
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", fmt.Errorf("reading version.txt: %w", err)
	}
	// version.txt: "rclone v1.68.2"
	for _, f := range strings.Fields(string(body)) {
		if strings.HasPrefix(f, "v") {
			return f, nil
		}
	}
	return "", fmt.Errorf("could not parse rclone version from version.txt")
}

// currentRcloneVersion returns the installed rclone version string.
// Returns empty string (no error) when rclone is not found.
func currentRcloneVersion(rclonePath string) (string, error) {
	out, err := exec.Command(rclonePath, "--version").Output()
	if err != nil {
		return "", nil // not installed yet
	}
	// First line: "rclone v1.68.2 ..."
	line, _, _ := strings.Cut(string(out), "\n")
	fields := strings.Fields(line)
	if len(fields) >= 2 {
		return fields[1], nil
	}
	return "", nil
}

// extractFromTar returns the contents of the entry whose base name matches
// binaryName from a gzip-compressed tar archive.
func extractFromTar(data []byte, binaryName string) ([]byte, error) {
	gz, err := gzip.NewReader(bytes.NewReader(data))
	if err != nil {
		return nil, fmt.Errorf("decompressing tar.gz: %w", err)
	}
	defer gz.Close()

	tr := tar.NewReader(gz)
	for {
		hdr, err := tr.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, fmt.Errorf("reading archive: %w", err)
		}
		if filepath.Base(hdr.Name) == binaryName {
			var buf bytes.Buffer
			if _, err := io.Copy(&buf, tr); err != nil {
				return nil, fmt.Errorf("extracting %s: %w", binaryName, err)
			}
			return buf.Bytes(), nil
		}
	}
	return nil, fmt.Errorf("%s not found in archive", binaryName)
}

// extractFromZip returns the contents of the entry whose base name matches
// binaryName from a zip archive.
func extractFromZip(data []byte, binaryName string) ([]byte, error) {
	zr, err := zip.NewReader(bytes.NewReader(data), int64(len(data)))
	if err != nil {
		return nil, fmt.Errorf("reading zip: %w", err)
	}

	for _, f := range zr.File {
		if filepath.Base(f.Name) == binaryName && !f.FileInfo().IsDir() {
			rc, err := f.Open()
			if err != nil {
				return nil, err
			}
			defer rc.Close()
			var buf bytes.Buffer
			if _, err := io.Copy(&buf, rc); err != nil {
				return nil, fmt.Errorf("extracting %s: %w", binaryName, err)
			}
			return buf.Bytes(), nil
		}
	}
	return nil, fmt.Errorf("%s not found in archive", binaryName)
}

// atomicReplace writes data to a temp file beside destPath then renames it
// over destPath. On Linux/macOS this is safe for the running binary because
// the kernel holds the old inode open until the process exits.
func atomicReplace(destPath string, data []byte) error {
	tmp, err := os.CreateTemp(filepath.Dir(destPath), ".git-sfs-update-*")
	if err != nil {
		return fmt.Errorf("creating temp file: %w", err)
	}
	tmpPath := tmp.Name()
	defer os.Remove(tmpPath) // no-op after a successful rename

	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		return fmt.Errorf("writing: %w", err)
	}
	if err := tmp.Chmod(0755); err != nil {
		tmp.Close()
		return fmt.Errorf("setting permissions: %w", err)
	}
	if err := tmp.Close(); err != nil {
		return fmt.Errorf("closing: %w", err)
	}
	return os.Rename(tmpPath, destPath)
}

// resolveExe returns the real path of the running executable with symlinks resolved.
func resolveExe() (string, error) {
	p, err := os.Executable()
	if err != nil {
		return "", fmt.Errorf("resolving executable path: %w", err)
	}
	p, err = filepath.EvalSymlinks(p)
	if err != nil {
		return "", fmt.Errorf("resolving executable symlinks: %w", err)
	}
	return p, nil
}

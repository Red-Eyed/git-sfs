package core

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"sort"
	"sync"

	"git-sfs/internal/cache"
	"git-sfs/internal/hash"
	"git-sfs/internal/progress"
	"git-sfs/internal/remote"
)

// sizeUnknown marks a file whose byte size could not be determined without
// downloading it: it is absent from the local cache and either the remote was
// not consulted or does not have it.
const sizeUnknown int64 = -1

// fileState is the inspection result for one unique tracked hash. Remote is a
// tri-state: nil means the remote was not consulted (local-only run); a non-nil
// pointer reports whether the file is present on the remote.
type fileState struct {
	Hash   hash.Hash
	Size   int64 // bytes, or sizeUnknown
	Cached bool
	Remote *bool
}

// fileRecord pairs a tracked symlink path with the state of the file it points
// to. Several symlinks may share one hash, so each record reuses the per-hash
// state computed once.
type fileRecord struct {
	Path  string
	State fileState
}

// Status inspects tracked symlinks and reports each file's size and whether it
// is cached locally — without downloading any bytes. A non-empty remoteName
// additionally consults that remote (metadata only) to report remote presence
// and to recover sizes for files that are not cached locally; an empty
// remoteName keeps the command local-only and makes no network calls.
func (a App) Status(ctx context.Context, remoteName string, asJSON bool, path string) (err error) {
	a.debugf("status: start remote=%s path=%s", remoteName, path)
	defer a.debugDone("status", &err)
	s, err := a.open()
	if err != nil {
		return err
	}
	repo, c, cfg := s.repo, s.cache, s.cfg

	checkRemote := remoteName != ""
	var r remote.Remote
	if checkRemote {
		r, err = a.selectRemote(s, remoteName)
		if err != nil {
			return err
		}
		if err := a.preflight(ctx, cfg, r); err != nil {
			return err
		}
	}

	links, err := collectGitSFSSymlinks(repo, path)
	if err != nil {
		return err
	}
	hashes := uniqueHashesFromTracked(links)
	states := inspectFiles(ctx, c, r, hashes, checkRemote, a.jobs(cfg, len(hashes)))

	records := make([]fileRecord, len(links))
	for i, l := range links {
		records[i] = fileRecord{Path: rel(repo, l.Path), State: states[l.Hash]}
	}
	sort.Slice(records, func(i, j int) bool { return records[i].Path < records[j].Path })

	if asJSON {
		return writeStatusJSON(a.Stdout, records, checkRemote)
	}
	writeStatusText(a.Stdout, records, checkRemote, a.Verbose)
	return nil
}

// inspectFiles computes the state of every unique hash. Local cache stats are
// cheap and done inline; remote metadata lookups are network calls, so they run
// across the configured worker pool exactly like verify's remote checks.
func inspectFiles(ctx context.Context, c cache.Cache, r remote.Remote, hashes []hash.Hash, checkRemote bool, workers int) map[hash.Hash]fileState {
	states := make(map[hash.Hash]fileState, len(hashes))
	for _, h := range hashes {
		states[h] = localState(c, h)
	}
	if !checkRemote {
		return states
	}
	var mu sync.Mutex
	runIndexed(ctx, len(hashes), workers, func(i int) error {
		st := states[hashes[i]]
		applyRemoteState(ctx, r, &st)
		mu.Lock()
		states[hashes[i]] = st
		mu.Unlock()
		return nil
	}, func(int, error) {})
	return states
}

// localState reports cache presence and, when present, the on-disk size. The
// content-addressed cache file is immutable once stored, so its stat size is the
// true byte length without re-hashing.
func localState(c cache.Cache, h hash.Hash) fileState {
	st := fileState{Hash: h, Size: sizeUnknown}
	if info, err := os.Stat(c.FilePath(h)); err == nil {
		st.Cached = true
		st.Size = info.Size()
	}
	return st
}

// applyRemoteState records whether the file is on the remote and fills in a size
// that the local cache could not provide. FileSize returns -1 when the file is
// absent, which doubles as the presence signal — one rclone metadata call, no
// bytes transferred.
func applyRemoteState(ctx context.Context, r remote.Remote, st *fileState) {
	size, err := r.FileSize(ctx, st.Hash)
	if err != nil {
		present := false
		st.Remote = &present
		return
	}
	present := size >= 0
	st.Remote = &present
	if !st.Cached && present {
		st.Size = size
	}
}

type statusFileJSON struct {
	Path   string `json:"path"`
	Hash   string `json:"hash"`
	Size   int64  `json:"size"`
	Cached bool   `json:"cached"`
	Remote *bool  `json:"remote,omitempty"`
}

type statusJSON struct {
	Tracked       int              `json:"tracked"`
	UniqueFiles   int              `json:"unique_files"`
	Cached        int              `json:"cached"`
	MissingLocal  int              `json:"missing_local"`
	TotalSize     int64            `json:"total_size"`
	RemoteChecked bool             `json:"remote_checked"`
	OnRemote      *int             `json:"on_remote,omitempty"`
	Unpushed      *int             `json:"unpushed,omitempty"`
	Files         []statusFileJSON `json:"files"`
}

func writeStatusJSON(w io.Writer, records []fileRecord, checkRemote bool) error {
	s := newSummary(records, checkRemote)
	out := statusJSON{
		Tracked:       len(records),
		UniqueFiles:   s.unique,
		Cached:        s.cached,
		MissingLocal:  s.unique - s.cached,
		TotalSize:     s.totalSize,
		RemoteChecked: checkRemote,
		Files:         make([]statusFileJSON, len(records)),
	}
	if checkRemote {
		onRemote := s.onRemote
		unpushed := s.unique - s.onRemote
		out.OnRemote = &onRemote
		out.Unpushed = &unpushed
	}
	for i, rec := range records {
		out.Files[i] = statusFileJSON{
			Path:   rec.Path,
			Hash:   rec.State.Hash.String(),
			Size:   rec.State.Size,
			Cached: rec.State.Cached,
			Remote: rec.State.Remote,
		}
	}
	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	return enc.Encode(out)
}

func writeStatusText(w io.Writer, records []fileRecord, checkRemote, verbose bool) {
	s := newSummary(records, checkRemote)
	fmt.Fprintf(w, "tracked symlinks: %d\n", len(records))
	fmt.Fprintf(w, "unique files: %d\n", s.unique)
	fmt.Fprintf(w, "cached locally: %d\n", s.cached)
	fmt.Fprintf(w, "missing locally: %d\n", s.unique-s.cached)
	fmt.Fprintf(w, "total size: %s\n", progress.HumanizeBytes(s.totalSize))
	if checkRemote {
		fmt.Fprintf(w, "on remote: %d\n", s.onRemote)
		fmt.Fprintf(w, "unpushed: %d\n", s.unique-s.onRemote)
	}
	if len(records) == 0 {
		return
	}
	fmt.Fprintln(w, "details:")
	for _, rec := range records {
		fmt.Fprintln(w, formatStatusLine(rec, checkRemote, verbose))
	}
}

// formatStatusLine renders one details row: path, size, local state, optional
// remote state, then the hash. The local and remote states are written as
// explicit "local=" / "remote=" key-value pairs so they cannot be misread as a
// single phrase (e.g. a file cached nowhere but present remotely reads as
// "local=missing remote=present", never "missing on remote"). Size is "-" when
// unknown — not cached and not found on a consulted remote.
func formatStatusLine(rec fileRecord, checkRemote, verbose bool) string {
	size := "-"
	if rec.State.Size != sizeUnknown {
		size = progress.HumanizeBytes(rec.State.Size)
	}
	line := fmt.Sprintf("%s: %s local=%s", rec.Path, size, localWord(rec.State.Cached))
	if checkRemote {
		line += " remote=" + remoteWord(rec.State.Remote)
	}
	if verbose {
		line += " " + rec.State.Hash.String()
	}
	return line
}

func localWord(cached bool) string {
	if cached {
		return "cached"
	}
	return "missing"
}

func remoteWord(remote *bool) string {
	if remote != nil && *remote {
		return "present"
	}
	return "missing"
}

type summary struct {
	unique    int
	cached    int
	onRemote  int
	totalSize int64
}

// newSummary aggregates over unique hashes, not symlinks, so files shared by
// several symlinks are counted and sized once. totalSize sums every known size,
// preferring local cache sizes and falling back to remote-reported sizes.
func newSummary(records []fileRecord, checkRemote bool) summary {
	seen := map[hash.Hash]bool{}
	var s summary
	for _, rec := range records {
		st := rec.State
		if seen[st.Hash] {
			continue
		}
		seen[st.Hash] = true
		s.unique++
		if st.Cached {
			s.cached++
		}
		if checkRemote && st.Remote != nil && *st.Remote {
			s.onRemote++
		}
		if st.Size != sizeUnknown {
			s.totalSize += st.Size
		}
	}
	return s
}

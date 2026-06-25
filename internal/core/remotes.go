package core

import (
	"encoding/json"
	"fmt"
	"io"
	"sort"

	"git-sfs/internal/config"
	"git-sfs/internal/localstate"
)

// defaultRemoteName is the remote push/pull/verify use when none is named.
const defaultRemoteName = "default"

// remoteEntry is one configured remote, flattened for display. Default marks the
// remote named "default", which the byte-moving commands use when -r is omitted.
type remoteEntry struct {
	Name    string `json:"name"`
	Backend string `json:"backend"`
	Path    string `json:"path,omitempty"`
	Config  string `json:"config,omitempty"`
	Default bool   `json:"default"`
}

// Remotes lists the remotes configured in .git-sfs/config.toml. It reads only
// the committed config — the source of truth — and never contacts a backend;
// use git-sfs doctor to test connectivity.
func (a App) Remotes(asJSON bool) (err error) {
	a.debugf("remotes: start json=%t", asJSON)
	defer a.debugDone("remotes", &err)
	repo, err := localstate.ResolveRepo()
	if err != nil {
		return err
	}
	cfg, err := config.Load(a.resolvedConfigPath(repo))
	if err != nil {
		return err
	}
	entries := remoteEntries(cfg)
	if asJSON {
		return writeRemotesJSON(a.Stdout, entries)
	}
	writeRemotesText(a.Stdout, entries)
	return nil
}

// remoteEntries flattens the config's remote map into a name-sorted slice so the
// listing is stable across runs (Go map iteration order is randomized).
func remoteEntries(cfg config.Config) []remoteEntry {
	entries := make([]remoteEntry, 0, len(cfg.Remotes))
	for name, rc := range cfg.Remotes {
		entries = append(entries, remoteEntry{
			Name:    name,
			Backend: rc.Backend,
			Path:    rc.Path,
			Config:  rc.Config,
			Default: name == defaultRemoteName,
		})
	}
	sort.Slice(entries, func(i, j int) bool { return entries[i].Name < entries[j].Name })
	return entries
}

func writeRemotesJSON(w io.Writer, entries []remoteEntry) error {
	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	return enc.Encode(struct {
		Remotes []remoteEntry `json:"remotes"`
	}{Remotes: entries})
}

func writeRemotesText(w io.Writer, entries []remoteEntry) {
	fmt.Fprintf(w, "remotes: %d\n", len(entries))
	for _, e := range entries {
		fmt.Fprintln(w, formatRemoteLine(e))
	}
}

// formatRemoteLine renders one remote as "name: backend=… [path=…] [config=…]
// [(default)]", omitting empty optional fields.
func formatRemoteLine(e remoteEntry) string {
	line := fmt.Sprintf("%s: backend=%s", e.Name, e.Backend)
	if e.Path != "" {
		line += " path=" + e.Path
	}
	if e.Config != "" {
		line += " config=" + e.Config
	}
	if e.Default {
		line += " (default)"
	}
	return line
}

# Roadmap

Planned improvements that are scoped and ready to implement but not yet scheduled.

---

## Force re-push for specific objects

`git sfs push --force <hash> [<hash>...]` — bypass rclone's skip logic and unconditionally re-upload the named cache object(s).

**Why it is needed.** `git sfs push` uses `--checksum` to skip remote files whose checksum matches. On backends that do not expose a native checksum (some SFTP servers, WebDAV), rclone falls back to size+modtime. A corrupt remote file with the correct size and an equal-or-newer modtime is never re-uploaded. There is currently no supported escape hatch: the user must invoke rclone directly.

**Proposed behavior.**
- Accepts one or more SHA-256 hashes as positional arguments.
- Uploads each named object unconditionally (no `--checksum`, no `--ignore-existing`).
- Exits non-zero if any object is not present in the local cache.
- Works with `-r` to target a named remote.

**Example.**
```sh
git sfs push --force ab12cd...  # re-push one object
git sfs push --force $(git sfs status --json | jq -r '.files[].hash')  # re-push all
```

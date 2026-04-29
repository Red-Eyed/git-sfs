# Project Status

`merk` is early.

The baseline implementation exists and covers the core local workflow:

```text
init
setup
add
push
pull
materialize
dematerialize
status
verify
gc
```

Implemented:

- Git-tracked symlink model
- Local cache
- SHA-256 file verification
- Local materialization through `.ds/worktree`
- Filesystem remote
- Initial rsync/ssh command backend
- CI
- Release archives
- Install script
- Smoke test

Still needs work:

- clearer `status` output
- stricter `verify` behavior
- real Git integration tests
- real rsync/ssh integration tests
- rclone backend
- stronger concurrency tests
- fault-injection tests
- better `gc` reports
- typed errors for important failure cases

See [../ROADMAP.md](../ROADMAP.md) for the task list.

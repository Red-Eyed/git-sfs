# Installation

## Standard install

```sh
curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

## Behind a proxy that blocks raw.githubusercontent.com

Download the install script from the release assets instead — served from `github.com/releases/download`, a different host:

```sh
curl -LsSf https://github.com/Red-Eyed/git-sfs/releases/latest/download/install.sh | sh
```

Or download a specific version:

```sh
curl -LsSf https://github.com/Red-Eyed/git-sfs/releases/download/vX.Y.Z/install.sh | sh -s -- --version vX.Y.Z
```

## Build from source

Requires only `git` access to `github.com` and a Rust toolchain:

```sh
git clone https://github.com/Red-Eyed/git-sfs
cd git-sfs
cargo install --path crates/git-sfs --locked
```

---

## Installer options

By default installs to `$HOME/.local/bin`. Override:

```sh
curl -LsSf .../install.sh | sh -s -- --install-dir /usr/local/bin
```

The installer also installs the latest stable `rclone` if not already on `PATH`. To skip:

```sh
curl -LsSf .../install.sh | sh -s -- --no-install-rclone
```

Install a specific version:

```sh
curl -LsSf .../install.sh | sh -s -- --version vX.Y.Z
```

Include prerelease versions when resolving the latest git-sfs release:

```sh
curl -LsSf .../install.sh | sh -s -- --pre
```

`--pre` expands the eligible versions; it does not force a prerelease when a
newer stable version exists. An explicit `--version` takes precedence. rclone
is still installed from its stable channel.

Corporate CA bundle:

```sh
curl -LsSf .../install.sh | sh -s -- --ca-bundle /path/to/corporate-ca.pem
```

Skip TLS verification entirely (last resort):

```sh
curl -kLsSf .../install.sh | sh -s -- --insecure-tls
```

## Updating

Once installed, update both `git-sfs` and `rclone` with:

```sh
git-sfs self update
git-sfs self update --pre
```

The default command considers stable releases. `--pre` includes git-sfs
prereleases while keeping rclone on its stable channel. Both forms replace
binaries atomically in the same directory as the running `git-sfs` executable
and honor the installer environment variables; see
[Corporate environments](commands.md#corporate-environments) for proxy and
custom CA options.

## Supported targets

```text
darwin/amd64
darwin/arm64
linux/amd64
linux/arm64
```

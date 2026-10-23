# Installation

## Install with uv

```sh
uv tool install git-sfs
git sfs --version
rclone version
```

The PyPI wheels include both executables for Linux and macOS on x86_64 and
ARM64. No Rust or Go compiler is needed. macOS wheels require macOS 12 or newer.
The bundled rclone supports git-sfs transfers and omits the optional cgo-based
`cmount` integration. License notices, complete rclone and dependency sources,
and rebuild instructions are installed under `share/licenses/git-sfs/rclone`
in the tool environment. These cover rclone's MIT license and its dependencies,
including LGPLv3 and MPL libraries. git-sfs itself remains MIT licensed.

Upgrade both executables through uv:

```sh
uv tool upgrade git-sfs
```

For a prerelease, use `uv tool install --prerelease allow git-sfs` and
`uv tool upgrade --prerelease allow git-sfs`. An exact version can be installed
with `uv tool install 'git-sfs==2.0.0rc10'`.

Use uv to upgrade a uv-managed installation. `git sfs self update` replaces
executables directly and leaves uv's installed package metadata out of sync.
An existing standalone `rclone` on PATH can conflict with uv exposing its
bundled executable; resolve that installation before installing the wheel.

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

## Updating standalone installations

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

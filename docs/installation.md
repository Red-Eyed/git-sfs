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
curl -LsSf https://github.com/Red-Eyed/git-sfs/releases/download/v1.6.0/install.sh | sh
```

## Build from source

Requires only `git` access to `github.com` and a Go toolchain:

```sh
git clone https://github.com/Red-Eyed/git-sfs
cd git-sfs
go build -o ~/.local/bin/git-sfs ./cmd/git-sfs
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
curl -LsSf .../install.sh | sh -s -- --version v1.6.0
```

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
```

This downloads the latest release of each binary and replaces it atomically in the same directory as the running `git-sfs` executable. Honors the same env vars as the installer — see [Corporate environments](commands.md#corporate-environments) for proxy and custom CA options.

## Supported targets

```text
darwin/amd64
darwin/arm64
linux/amd64
linux/arm64
```

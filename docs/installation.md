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
GIT_SFS_INSTALL_DIR=/usr/local/bin curl -LsSf .../install.sh | sh
```

The installer also installs the latest stable `rclone` if not already on `PATH`. To skip:

```sh
GIT_SFS_INSTALL_RCLONE=0 curl -LsSf .../install.sh | sh
```

Install a specific version:

```sh
GIT_SFS_VERSION=v1.6.0 curl -LsSf .../install.sh | sh
```

Corporate CA bundle:

```sh
SSL_CERT_FILE=/path/to/corporate-ca.pem curl -LsSf .../install.sh | SSL_CERT_FILE=/path/to/corporate-ca.pem sh
```

Skip TLS verification entirely (last resort):

```sh
curl -kLsSf .../install.sh | GIT_SFS_INSECURE_TLS=1 sh
```

## Supported targets

```text
darwin/amd64
darwin/arm64
linux/amd64
linux/arm64
```

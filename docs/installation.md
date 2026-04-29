# Installation

Install the latest release:

```sh
curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

By default this installs to:

```text
$HOME/.local/bin/git-sfs
```

Override the install directory:

```sh
GIT_SFS_INSTALL_DIR=/usr/local/bin curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

Install a specific version:

```sh
GIT_SFS_VERSION=v0.1.0 curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

Supported release targets:

```text
darwin/amd64
darwin/arm64
linux/amd64
linux/arm64
```

Build from source:

```sh
go build ./cmd/git-sfs
```

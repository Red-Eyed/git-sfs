# Installation

Install the latest release:

```sh
curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

By default this installs to:

```text
$HOME/.local/bin/git-sfs
```

The installer also installs the latest stable `rclone` into the same directory
by default. To skip this:

```sh
GIT_SFS_INSTALL_RCLONE=0 curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

Override the install directory:

```sh
GIT_SFS_INSTALL_DIR=/usr/local/bin curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

Install a specific version:

```sh
GIT_SFS_VERSION=v0.1.0 curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

Corporate networks that intercept TLS with a private certificate authority can
point curl and the installer at that CA bundle:

```sh
SSL_CERT_FILE=/path/to/corporate-ca.pem curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | SSL_CERT_FILE=/path/to/corporate-ca.pem sh
```

The installer respects `GIT_SFS_SSL_CERT_FILE`, `SSL_CERT_FILE`, and
`CURL_CA_BUNDLE` for its GitHub and rclone downloads. If a trusted CA bundle is
not available, it can opt into insecure download TLS:

```sh
curl -kLsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | GIT_SFS_INSECURE_TLS=1 sh
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

#!/bin/sh
# Installer script for jotto, wordle, contexto, flights, and arcade (https://github.com/guslipkin/jotto-cli)
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/guslipkin/jotto-cli/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/guslipkin/jotto-cli/main/install.sh | sh -s -- --bin-dir /usr/local/bin

set -eu

REPO="guslipkin/jotto-cli"
BINARIES="jotto wordle contexto flights arcade"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${VERSION:-latest}"
FROM_SOURCE=0

usage() {
    cat <<EOF
Install jotto, wordle, contexto, flights, and arcade from GitHub Releases ($REPO)

Usage:
  install.sh [OPTIONS]

Options:
  -b, --bin-dir <DIR>   Installation directory (default: ~/.local/bin or \$INSTALL_DIR)
  -v, --version <TAG>   Release tag to install, e.g. v0.1.0 (default: latest or \$VERSION)
      --from-source     Build and install from source using cargo
  -h, --help            Show this help message
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        -b|--bin-dir)
            if [ $# -lt 2 ]; then
                echo "Error: $1 requires a directory argument." >&2
                exit 1
            fi
            INSTALL_DIR="$2"
            shift 2
            ;;
        -v|--version)
            if [ $# -lt 2 ]; then
                echo "Error: $1 requires a version tag argument." >&2
                exit 1
            fi
            VERSION="$2"
            shift 2
            ;;
        --from-source)
            FROM_SOURCE=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Error: unknown option '$1'" >&2
            usage >&2
            exit 1
            ;;
    esac
done

IS_ANDROID=0
if [ "$(uname -o 2>/dev/null || true)" = "Android" ] \
    || [ -n "${ANDROID_ROOT:-}" ] \
    || [ -n "${ANDROID_DATA:-}" ] \
    || [ -e "/system/bin/linker64" ] \
    || [ -e "/system/bin/linker" ]; then
    IS_ANDROID=1
fi

TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t jotto-install)"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

check_path_hint() {
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*)
            ;;
        *)
            echo ""
            echo "Note: $INSTALL_DIR is not currently on your PATH."
            echo "Add it to your shell profile (e.g. ~/.zshrc or ~/.bashrc):"
            echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac
}

install_from_source() {
    if ! command -v cargo >/dev/null 2>&1; then
        echo "Error: 'cargo' is required to build from source. Install Rust from https://rustup.rs/" >&2
        exit 1
    fi
    echo "Building and installing $BINARIES from source via cargo into $INSTALL_DIR..."
    CARGO_ROOT="$TMP_DIR/cargo-root"
    if [ "$VERSION" = "latest" ]; then
        cargo install --git "https://github.com/$REPO.git" --bins --root "$CARGO_ROOT"
    else
        cargo install --git "https://github.com/$REPO.git" --tag "$VERSION" --bins --root "$CARGO_ROOT"
    fi

    mkdir -p "$INSTALL_DIR"
    INSTALLED=""
    for bin in $BINARIES; do
        if [ -f "$CARGO_ROOT/bin/$bin" ]; then
            rm -f "$INSTALL_DIR/$bin"
            cp "$CARGO_ROOT/bin/$bin" "$INSTALL_DIR/$bin"
            chmod 755 "$INSTALL_DIR/$bin"
            INSTALLED="$INSTALLED $bin"
        fi
    done

    echo "Installed:$INSTALLED -> $INSTALL_DIR"
    check_path_hint
}

if [ "$FROM_SOURCE" -eq 1 ]; then
    install_from_source
    exit 0
fi

OS="$(uname -s)"
ARCH="$(uname -m)"

if [ "$IS_ANDROID" -eq 1 ]; then
    OS_NAME="android"
else
    case "$OS" in
        Linux)
            OS_NAME="linux"
            ;;
        Darwin)
            OS_NAME="macos"
            ;;
        *)
            echo "Error: unsupported OS '$OS' for pre-built Unix archives." >&2
            echo "Please install on Windows using install.ps1 or pass --from-source to build with cargo." >&2
            exit 1
            ;;
    esac
fi

case "$ARCH" in
    x86_64|amd64)
        ARCH_NAME="x86_64"
        ;;
    aarch64|arm64)
        ARCH_NAME="aarch64"
        ;;
    *)
        echo "Error: unsupported architecture '$ARCH' for pre-built archives." >&2
        echo "Pass --from-source to build and install with cargo." >&2
        exit 1
        ;;
esac

ASSET_NAME="${OS_NAME}-${ARCH_NAME}"
ARCHIVE_FILE="jotto-${ASSET_NAME}.tar.gz"

if [ "$VERSION" = "latest" ]; then
    BASE_URL="https://github.com/$REPO/releases/latest/download"
else
    BASE_URL="https://github.com/$REPO/releases/download/$VERSION"
fi

ARCHIVE_URL="$BASE_URL/$ARCHIVE_FILE"
CHECKSUMS_URL="$BASE_URL/SHA256SUMS.txt"

download() {
    url="$1"
    dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$dest"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$dest" "$url"
    else
        echo "Error: either 'curl' or 'wget' is required to download releases." >&2
        exit 1
    fi
}

echo "Downloading $ARCHIVE_FILE ($VERSION) for $ASSET_NAME..."
if ! download "$ARCHIVE_URL" "$TMP_DIR/$ARCHIVE_FILE"; then
    echo "Error: failed to download pre-built archive $ARCHIVE_URL" >&2
    echo "Make sure a GitHub release containing $ARCHIVE_FILE has been published, or pass --from-source." >&2
    exit 1
fi

if download "$CHECKSUMS_URL" "$TMP_DIR/SHA256SUMS.txt" 2>/dev/null; then
    EXPECTED_SUM="$(grep " $ARCHIVE_FILE\$" "$TMP_DIR/SHA256SUMS.txt" | awk '{print $1}' || true)"
    if [ -n "$EXPECTED_SUM" ]; then
        ACTUAL_SUM=""
        if command -v sha256sum >/dev/null 2>&1; then
            ACTUAL_SUM="$(sha256sum "$TMP_DIR/$ARCHIVE_FILE" | awk '{print $1}')"
        elif command -v shasum >/dev/null 2>&1; then
            ACTUAL_SUM="$(shasum -a 256 "$TMP_DIR/$ARCHIVE_FILE" | awk '{print $1}')"
        fi
        if [ -n "$ACTUAL_SUM" ]; then
            if [ "$ACTUAL_SUM" != "$EXPECTED_SUM" ]; then
                echo "Error: SHA-256 checksum mismatch for $ARCHIVE_FILE!" >&2
                echo "  Expected: $EXPECTED_SUM" >&2
                echo "  Actual:   $ACTUAL_SUM" >&2
                exit 1
            fi
            echo "Verified SHA-256 checksum."
        fi
    fi
fi

tar -xzf "$TMP_DIR/$ARCHIVE_FILE" -C "$TMP_DIR"
STAGING_DIR="$TMP_DIR/jotto-${ASSET_NAME}"

mkdir -p "$INSTALL_DIR"

INSTALLED=""
for bin in $BINARIES; do
    if [ -f "$STAGING_DIR/$bin" ]; then
        rm -f "$INSTALL_DIR/$bin"
        cp "$STAGING_DIR/$bin" "$INSTALL_DIR/$bin"
        chmod 755 "$INSTALL_DIR/$bin"
        INSTALLED="$INSTALLED $bin"
    fi
done

if [ -z "$INSTALLED" ]; then
    echo "Error: no binaries found in archive $ARCHIVE_FILE." >&2
    exit 1
fi

echo "Installed:$INSTALLED -> $INSTALL_DIR"
check_path_hint

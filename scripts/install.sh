#!/usr/bin/env bash
# TurboLog installer — curl -fsSL https://raw.githubusercontent.com/shguddn8591/turbolog/main/scripts/install.sh | bash
set -euo pipefail

REPO="shguddn8591/turbolog"
INSTALL_DIR="${TURBOLOG_INSTALL_DIR:-/usr/local/bin}"
BINARY="turbolog"

detect_target() {
    local os arch
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)

    case "$os" in
        linux)
            case "$arch" in
                x86_64)  echo "x86_64-linux" ;;
                aarch64) echo "aarch64-linux" ;;
                arm64)   echo "aarch64-linux" ;;
                *)       echo "unsupported arch: $arch" >&2; exit 1 ;;
            esac
            ;;
        darwin)
            case "$arch" in
                x86_64)  echo "x86_64-macos" ;;
                arm64)   echo "aarch64-macos" ;;
                *)       echo "unsupported arch: $arch" >&2; exit 1 ;;
            esac
            ;;
        *)
            echo "unsupported OS: $os" >&2
            exit 1
            ;;
    esac
}

main() {
    echo "Detecting platform…"
    TARGET=$(detect_target)
    echo "Platform: $TARGET"

    echo "Fetching latest release version…"
    VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \\
        | grep '"tag_name"' | sed 's/.*"tag_name": *"\\(.*\\)".*/\\1/' || true)

    if [ -z "$VERSION" ]; then
        echo "Failed to fetch latest version" >&2
        exit 1
    fi
    echo "Latest version: $VERSION"

    ARCHIVE="turbolog-${TARGET}.tar.gz"
    URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"

    echo "Downloading ${ARCHIVE}…"
    TMP=$(mktemp -d)
    trap 'rm -rf "$TMP"' EXIT
    curl -fsSL "$URL" -o "$TMP/$ARCHIVE"

    echo "Extracting…"
    tar -xzf "$TMP/$ARCHIVE" -C "$TMP"

    echo "Installing to ${INSTALL_DIR}/${BINARY}…"
    chmod +x "$TMP/$BINARY"
    if [ -w "$INSTALL_DIR" ]; then
        mkdir -p "$INSTALL_DIR"
        mv "$TMP/$BINARY" "${INSTALL_DIR}/${BINARY}"
    elif [ ! -d "$INSTALL_DIR" ] && [ -w "$(dirname "$INSTALL_DIR")" ]; then
        mkdir -p "$INSTALL_DIR"
        mv "$TMP/$BINARY" "${INSTALL_DIR}/${BINARY}"
    else
        sudo mkdir -p "$INSTALL_DIR"
        sudo mv "$TMP/$BINARY" "${INSTALL_DIR}/${BINARY}"
    fi

    echo ""
    echo "✅  TurboLog ${VERSION} installed to ${INSTALL_DIR}/${BINARY}"
    echo ""
    echo "Quick start:"
    echo "  tail -f /var/log/syslog | turbolog watch"
    echo "  cat app.log | turbolog scan"
    echo "  turbolog serve  # HTTP server on :8087"
}

main "$@"

#!/bin/bash
set -e

REPO="sting8k/tilth"
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$ARCH" in
    x86_64) ARCH="x86_64" ;;
    arm64|aarch64) ARCH="arm64" ;;
    *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

echo "Installing tilth for ${OS}-${ARCH}..."

# Get latest version tag
VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"v([^"]+)".*/\1/')
if [ -z "$VERSION" ]; then
    echo "Error: could not determine latest version"
    exit 1
fi

URL="https://github.com/${REPO}/releases/download/v${VERSION}/tilth-${VERSION}-${OS}-${ARCH}.tar.gz"
echo "  Downloading v${VERSION}..."

curl -fsSL "$URL" | tar xz -C "$INSTALL_DIR/"
chmod +x "${INSTALL_DIR}/tilth-${VERSION}-${OS}-${ARCH}"
mv "${INSTALL_DIR}/tilth-${VERSION}-${OS}-${ARCH}" "${INSTALL_DIR}/tilth"

echo ""
echo "tilth v${VERSION} installed to ${INSTALL_DIR}/tilth"
echo ""
echo "MCP config (add to your AI tool settings):"
echo '  { "command": "tilth", "args": ["--mcp"] }'
echo ""
echo "Or install from source: cargo install tilth"

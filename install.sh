#!/bin/bash
set -e

REPO="sting8k/srcwalk"
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$ARCH" in
    x86_64) ARCH="x86_64" ;;
    arm64|aarch64) ARCH="arm64" ;;
    *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

echo "Installing srcwalk for ${OS}-${ARCH}..."

# Get latest version tag
VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"v([^"]+)".*/\1/')
if [ -z "$VERSION" ]; then
    echo "Error: could not determine latest version"
    exit 1
fi

URL="https://github.com/${REPO}/releases/download/v${VERSION}/srcwalk-${VERSION}-${OS}-${ARCH}.tar.gz"
echo "  Downloading v${VERSION}..."

curl -fsSL "$URL" | tar xz -C "$INSTALL_DIR/"
chmod +x "${INSTALL_DIR}/srcwalk-${VERSION}-${OS}-${ARCH}"
mv "${INSTALL_DIR}/srcwalk-${VERSION}-${OS}-${ARCH}" "${INSTALL_DIR}/srcwalk"

echo ""
echo "srcwalk v${VERSION} installed to ${INSTALL_DIR}/srcwalk"
echo ""
echo "MCP config (add to your AI tool settings):"
echo '  { "command": "srcwalk", "args": ["--mcp"] }'
echo ""
echo "Or install from source: cargo install srcwalk"

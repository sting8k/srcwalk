#!/bin/bash
set -e

VERSION=$(cargo metadata --format-version 1 | jq -r '.packages[] | select(.name=="srcwalk") | .version')
DIST="dist"
mkdir -p "$DIST"

echo "Building srcwalk v${VERSION}..."

# macOS arm64 (native)
echo "  → darwin-arm64 (native)"
cargo build --release
cp target/release/srcwalk "${DIST}/srcwalk-${VERSION}-darwin-arm64"

# macOS x86_64 (cross — requires: rustup target add x86_64-apple-darwin)
if rustup target list --installed | grep -q x86_64-apple-darwin; then
    echo "  → darwin-x86_64 (cross)"
    cargo build --release --target x86_64-apple-darwin
    cp target/x86_64-apple-darwin/release/srcwalk "${DIST}/srcwalk-${VERSION}-darwin-x86_64"
else
    echo "  ⚠ Skipping darwin-x86_64 (run: rustup target add x86_64-apple-darwin)"
fi

# Linux x86_64 (via cross — requires: cargo install cross)
if command -v cross &>/dev/null; then
    echo "  → linux-x86_64 (cross)"
    cross build --release --target x86_64-unknown-linux-gnu
    cp target/x86_64-unknown-linux-gnu/release/srcwalk "${DIST}/srcwalk-${VERSION}-linux-x86_64"
else
    echo "  ⚠ Skipping linux-x86_64 (run: cargo install cross)"
fi

# Create tarballs
echo "Creating tarballs..."
cd "$DIST"
for f in srcwalk-${VERSION}-*; do
    [ -f "$f" ] && tar czf "${f}.tar.gz" "$f" && echo "  → ${f}.tar.gz"
done

echo ""
echo "Done. Upload with:"
echo "  gh release create v${VERSION} dist/*.tar.gz"

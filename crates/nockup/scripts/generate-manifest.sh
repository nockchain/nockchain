#!/bin/bash

set -e  # Exit on error

BINARY=$1
PLATFORM=$2  # linux64 or darwin64
CHANNEL=${3:-stable}

if [ -z "$BINARY" ] || [ -z "$PLATFORM" ]; then
    echo "Usage: $0 <binary> <platform> [channel]" >&2
    echo "Example: $0 hoonc linux64 stable" >&2
    exit 1
fi

# Get nockchain info from environment or git
NOCKCHAIN_OWNER=${NOCKCHAIN_OWNER:-sigilante}
NOCKCHAIN_REPO=${NOCKCHAIN_REPO:-nockchain}
NOCKCHAIN_COMMIT=${NOCKCHAIN_COMMIT:-$(git rev-parse HEAD)}
NOCKCHAIN_SHORT=$(echo $NOCKCHAIN_COMMIT | cut -c1-7)
DATE=$(date +%Y-%m-%d)

echo "Building manifest for:" >&2
echo "  Binary: $BINARY" >&2
echo "  Platform: $PLATFORM" >&2
echo "  Channel: $CHANNEL" >&2
echo "  Commit: $NOCKCHAIN_COMMIT" >&2

# Map platform to Rust target triple
case "$PLATFORM" in
    linux64)
        TARGET="x86_64-unknown-linux-gnu"
        ;;
    darwin64)
        # Apple Silicon (M1/M2/M3/M4/M5)
        TARGET="aarch64-apple-darwin"
        ;;
    darwinx86)
        # Intel Macs (legacy, if needed)
        TARGET="x86_64-apple-darwin"
        ;;
    *)
        echo "Unknown platform: $PLATFORM" >&2
        exit 1
        ;;
esac

# Generate the release tag for THIS specific commit
RELEASE_TAG="${CHANNEL}-build-${NOCKCHAIN_COMMIT}"

# Default version
VERSION="0.1.0"

# Try to get version from the specific release for this commit
RELEASE_API="https://api.github.com/repos/$NOCKCHAIN_OWNER/$NOCKCHAIN_REPO/releases/tags/${RELEASE_TAG}"
echo "Fetching release info for tag: $RELEASE_TAG" >&2

RELEASE_JSON=$(curl -s "$RELEASE_API")

# Check if release exists
if echo "$RELEASE_JSON" | grep -q '"message".*"Not Found"'; then
    echo "Warning: Release not found for tag $RELEASE_TAG" >&2
else
    # Extract version from asset names in this specific release
    VERSION_FROM_ASSETS=$(echo "$RELEASE_JSON" | \
        grep -o '"name"[[:space:]]*:[[:space:]]*"[^"]*"' | \
        grep -o "${BINARY}-${CHANNEL}-[0-9]\+\.[0-9]\+\.[0-9]\+-" | \
        head -1 | \
        sed "s/${BINARY}-${CHANNEL}-//" | \
        sed 's/-$//')
    
    if [ -n "$VERSION_FROM_ASSETS" ]; then
        VERSION="$VERSION_FROM_ASSETS"
        echo "Found version from release assets: $VERSION" >&2
    fi
fi

# If version extraction failed, try to fetch from Cargo.toml at this commit
if [ "$VERSION" = "0.1.0" ]; then
    echo "Attempting to fetch version from Cargo.toml at commit $NOCKCHAIN_COMMIT" >&2
    
    # Try local file first (since we're in the nockchain repo)
    if [ -f "crates/${BINARY}/Cargo.toml" ]; then
        VERSION_FROM_CARGO=$(grep '^version[[:space:]]*=' "crates/${BINARY}/Cargo.toml" | head -1 | sed 's/.*"\([0-9.]*\)".*/\1/')
        if [ -n "$VERSION_FROM_CARGO" ]; then
            VERSION="$VERSION_FROM_CARGO"
            echo "Found version from local Cargo.toml: $VERSION" >&2
        fi
    else
        # Fall back to fetching from GitHub
        CARGO_URL="https://raw.githubusercontent.com/$NOCKCHAIN_OWNER/$NOCKCHAIN_REPO/$NOCKCHAIN_COMMIT/crates/${BINARY}/Cargo.toml"
        CARGO_CONTENT=$(curl -s "$CARGO_URL")
        
        if [ -n "$CARGO_CONTENT" ] && ! echo "$CARGO_CONTENT" | grep -q "404: Not Found"; then
            VERSION_FROM_CARGO=$(echo "$CARGO_CONTENT" | grep '^version[[:space:]]*=' | head -1 | sed 's/.*"\([0-9.]*\)".*/\1/')
            if [ -n "$VERSION_FROM_CARGO" ]; then
                VERSION="$VERSION_FROM_CARGO"
                echo "Found version from remote Cargo.toml: $VERSION" >&2
            fi
        fi
    fi
fi

# Generate the URL following the exact pattern
ARTIFACT_NAME="${BINARY}-${CHANNEL}-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/$NOCKCHAIN_OWNER/$NOCKCHAIN_REPO/releases/download/${RELEASE_TAG}/${ARTIFACT_NAME}"

echo "Download URL: $URL" >&2

# Create temporary directory for download
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

DOWNLOAD_PATH="$TEMP_DIR/$ARTIFACT_NAME"

# Download the artifact
echo "Downloading artifact..." >&2
if ! curl -L -f -o "$DOWNLOAD_PATH" "$URL"; then
    echo "Error: Failed to download $URL" >&2
    echo "This could mean:" >&2
    echo "  1. The release doesn't exist yet" >&2
    echo "  2. The version number is incorrect" >&2
    echo "  3. The artifact wasn't uploaded" >&2
    exit 1
fi

echo "Download successful, computing hashes..." >&2

# Compute BLAKE3 hash
if command -v b3sum >/dev/null 2>&1; then
    BLAKE3_HASH=$(b3sum "$DOWNLOAD_PATH" | awk '{print $1}')
    echo "BLAKE3: $BLAKE3_HASH" >&2
else
    echo "Warning: b3sum not found, using placeholder for BLAKE3 hash" >&2
    echo "Install with: cargo install b3sum" >&2
    BLAKE3_HASH="0000000000000000000000000000000000000000000000000000000000000000"
fi

# Compute SHA-1 hash
if command -v sha1sum >/dev/null 2>&1; then
    SHA1_HASH=$(sha1sum "$DOWNLOAD_PATH" | awk '{print $1}')
    echo "SHA-1: $SHA1_HASH" >&2
elif command -v shasum >/dev/null 2>&1; then
    SHA1_HASH=$(shasum -a 1 "$DOWNLOAD_PATH" | awk '{print $1}')
    echo "SHA-1: $SHA1_HASH" >&2
else
    echo "Warning: sha1sum/shasum not found, using placeholder for SHA-1 hash" >&2
    SHA1_HASH="0000000000000000000000000000000000000000"
fi

# Create manifest directory if it doesn't exist
MANIFEST_DIR="${MANIFEST_DIR:-crates/nockup/toolchains}"
mkdir -p "$MANIFEST_DIR"

# Generate manifest file
MANIFEST_FILE="$MANIFEST_DIR/${BINARY}-${TARGET}-${CHANNEL}.toml"

cat << EOF > "$MANIFEST_FILE"
manifest-version = "1"
date = "$DATE"
commit = "$NOCKCHAIN_COMMIT"
commit_short = "$NOCKCHAIN_SHORT"
release_tag = "$RELEASE_TAG"

[pkg.$BINARY]
version = "$VERSION"
components = ["core"]

[pkg.$BINARY.target.$TARGET]
available = true
url = "$URL"
hash_blake3 = "$BLAKE3_HASH"
hash_sha1 = "$SHA1_HASH"
EOF

echo "" >&2
echo "✓ Generated manifest: $MANIFEST_FILE" >&2
echo "✓ Version: $VERSION" >&2
echo "✓ BLAKE3: $BLAKE3_HASH" >&2
echo "✓ SHA-1: $SHA1_HASH" >&2
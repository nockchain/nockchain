#!/bin/bash

CHANNEL=$1
MANIFEST_DIR=$2
OUTPUT_FILE="$CHANNEL-manifest.toml"

# Get nockup version dynamically
NOCKUP_VERSION=$(cargo metadata --format-version 1 --no-deps | jq -r '.packages[] | select(.name == "nockup") | .version')

# Start with global metadata
cat << EOF > "$OUTPUT_FILE"
manifest-version = "1"
date = "$(date +%Y-%m-%d)"

# Global package info
[pkg.nockup]
version = "$NOCKUP_VERSION"
components = ["core"]
extensions = []

# Profiles
[profiles.default]
components = ["core"]
[profiles.minimal]
components = ["core"]

EOF

# Merge all individual manifests, skipping headers and pkg definitions
find "$MANIFEST_DIR" -name "*-manifest.toml" | sort | while read manifest; do
    echo "# From $(basename "$manifest")"
    # Skip header lines and [pkg.xxx] sections, only take target entries
    awk '
        /^\[pkg\.[^.]*\.target\./ { printing = 1 }
        /^\[pkg\.[^.]*\]$/ && !/target/ { printing = 0; next }
        /^manifest-version|^date|^\s*$/ && !printing { next }
        printing || /^\[pkg\.[^.]*\.target\./
    ' "$manifest"
    echo ""
done >> "$OUTPUT_FILE"
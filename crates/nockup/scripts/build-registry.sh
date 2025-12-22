#!/bin/bash
# Build complete registry.toml from multiple workspaces

set -e

OUTPUT="registry.toml"

# Registry header
cat > "$OUTPUT" << 'EOF'
# ============================================================================
# Registry metadata
# ============================================================================
[registry]
name = "typhoon"
version = "0.1.0"
description = "TYPical HOON package registry"
url = "https://github.com/sigilante/typhoon"

[config]
default_ref = "latest"

EOF

echo "Building registry..."

# Scan Urbit workspace (if available)
if [ -d "$HOME/.nockup/cache/git/urbit/urbit/pkg/arvo" ]; then
    echo "Scanning urbit workspace..."
    python3 scan-deps-v2.py \
        --workspace urbit \
        --root-path "pkg/arvo" \
        --git-url "https://github.com/urbit/urbit" \
        --ref "409k" \
        --description "Urbit OS" \
        "$HOME/.nockup/cache/git/urbit/urbit/pkg/arvo" \
        >> "$OUTPUT"
fi

# Scan Nockchain workspace
if [ -d "../nockchain/hoon" ]; then
    echo "Scanning nockchain workspace..."
    python3 scan-deps-v2.py \
        --workspace nockchain \
        --root-path "hoon" \
        --git-url "https://github.com/nockchain/nockchain" \
        --ref "a19ad4dc" \
        --description "Nockchain standard library" \
        "../nockchain/hoon" \
        >> "$OUTPUT"
fi

echo "Registry written to $OUTPUT"
echo ""
echo "To use this registry with nockup:"
echo "  1. Copy registry.toml to sigilante/typhoon repo"
echo "  2. Tag it with a version"
echo "  3. Configure nockup to use it"

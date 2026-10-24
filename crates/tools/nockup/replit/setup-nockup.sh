#!/bin/bash
set -e

echo "🚀 Setting up Nockup development environment..."

# Increase stack size to work around TLS issues
ulimit -s unlimited 2>/dev/null || ulimit -s 65536

# Install Rust nightly if not already installed
if ! command -v rustc >/dev/null 2>&1 || ! rustup toolchain list | grep -q nightly; then
    echo "📦 Installing Rust stable toolchain..."
    rustup toolchain install stable
    rustup default stable
    echo "✅ Rust stable installed"
else
    echo "✅ Rust stable already installed"
fi

# Install nockup
echo "📦 Installing nockup..."
curl -H "Cache-Control: no-cache" -H "Pragma: no-cache" -fsSL https://raw.githubusercontent.com/nockchain/nockchain/master/crates/nockup/install.sh | bash

# Source the activation script to make nockup available immediately
if [ -f "$HOME/.nockup/activate.sh" ]; then
    source "$HOME/.nockup/activate.sh"
fi

echo "✅ Setup complete!"
echo "📦 nockup is now available in PATH"
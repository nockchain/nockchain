#!/bin/bash

set -euo pipefail

: "${HOME:=$(getent passwd "$(whoami)" | cut -d: -f6)}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# Configuration
GITHUB_REPO="nockchain/nockchain"
VERSION="unknown"
RELEASE_TAG="unknown"
CHANNEL="stable"
CONFIG_URL_MACOS="https://raw.githubusercontent.com/nockchain/nockchain/refs/heads/master/crates/nockup/default-config-aarch64-apple-darwin.toml"
CONFIG_URL_LINUX="https://raw.githubusercontent.com/nockchain/nockchain/refs/heads/master/crates/nockup/default-config-x86_64-unknown-linux-gnu.toml"
# Determine config URL based on OS
if [[ "$(uname -s)" == "Darwin" ]]; then
    CONFIG_URL="$CONFIG_URL_MACOS"
else
    CONFIG_URL="$CONFIG_URL_LINUX"
fi

# Function to print colored output (to stderr so it doesn't interfere with function returns)
print_info() {
    echo -e "${BLUE}ℹ️  $1${NC}" >&2
}

print_success() {
    echo -e "${GREEN}✅ $1${NC}" >&2
}

print_warning() {
    echo -e "${YELLOW}⚠️  $1${NC}" >&2
}

print_error() {
    echo -e "${RED}❌ $1${NC}" >&2
}

print_step() {
    echo -e "${CYAN}🚀 $1${NC}" >&2
}

# Function to check if command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Function to download file
download_file() {
    local url="$1"
    local output="$2"

    if command_exists curl; then
        curl -fsSL "$url" -o "$output"
    elif command_exists wget; then
        wget -q "$url" -O "$output"
    else
        print_error "Neither curl nor wget found. Please install one of them."
        return 1
    fi
}

# Function to create temporary directory
create_temp_dir() {
    local temp_dir
    if command_exists mktemp; then
        temp_dir=$(mktemp -d)
    else
        temp_dir="/tmp/nockup-install-$$"
        mkdir -p "$temp_dir"
    fi
    echo "$temp_dir"
}

# Function to get latest release for channel
get_latest_release() {
    local channel="$1"
    
    print_step "Fetching latest ${channel} commit from branch..."
    
    # Determine branch name based on channel
    local branch="master"  # or "nightly" for nightly channel
    if [[ "$channel" == "nightly" ]]; then
        branch="nightly"
    fi
    
    # Get latest commit SHA from the branch
    local commits_url="https://api.github.com/repos/${GITHUB_REPO}/commits/${branch}"
    local temp_commit="/tmp/nockup-commit-$$.json"
    
    if ! download_file "$commits_url" "$temp_commit"; then
        print_error "Failed to fetch latest commit from branch ${branch}"
        return 1
    fi
    
    local latest_commit_sha=""
    latest_commit_sha=$(grep -o "\"sha\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" "$temp_commit" | \
                       sed 's/"sha"[[:space:]]*:[[:space:]]*"\([^"]*\)"/\1/' | head -1) || true
    
    rm -f "$temp_commit"
    
    if [[ -z "$latest_commit_sha" ]]; then
        print_error "Could not determine latest commit SHA"
        return 1
    fi
    
    print_info "Latest commit: ${latest_commit_sha:0:7}"

    # Construct the release tag (channel is ignored in the actual release tag)
    local release_tag="build-${latest_commit_sha}"

    print_info "Looking for release: $release_tag"

    # Verify this release exists
    local releases_url="https://api.github.com/repos/${GITHUB_REPO}/releases/tags/${release_tag}"
    local temp_release="/tmp/nockup-release-$$.json"

    if ! download_file "$releases_url" "$temp_release"; then
        print_error "Release not found for tag: $release_tag"
        print_info "The build may still be in progress"
        rm -f "$temp_release"
        return 1
    fi

    # Extract version from release name
    local version=""
    version=$(grep -o "\"name\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" "$temp_release" | \
              sed 's/"name"[[:space:]]*:[[:space:]]*"\([^"]*\)"/\1/' | head -1) || true

    if [[ -z "$version" ]]; then
        version=$(echo "$release_tag" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+') || version="latest"
    fi

    rm -f "$temp_release"

    echo "$release_tag|$version"
}

# Function to detect platform and architecture
detect_platform() {
    local arch
    local os
    local target

    case "$(uname -m)" in
        x86_64|amd64)
            arch="x86_64"
            ;;
        arm64|aarch64)
            arch="aarch64"
            ;;
        *)
            print_error "Unsupported architecture: $(uname -m)"
            print_info "Supported architectures: x86_64, aarch64"
            exit 1
            ;;
    esac

    case "$(uname -s)" in
        Linux)
            os="unknown-linux-gnu"
            ;;
        Darwin)
            os="apple-darwin"
            ;;
        *)
            print_error "Unsupported operating system: $(uname -s)"
            print_info "Supported operating systems: Linux, macOS"
            exit 1
            ;;
    esac

    target="${arch}-${os}"
    echo "$target"
}

# Function to setup toolchain directory with channel manifests
setup_toolchain() {
    local toolchain_dir="$HOME/.nockup/toolchains"
    
    mkdir -p "$toolchain_dir"
    
    print_step "Setting up toolchain directory"
    print_info "Fetching latest channel manifests from GitHub releases"
    
    get_latest_manifest() {
        local channel="$1"
        local manifest_file="nockchain-manifest.toml"
        local output_file="${toolchain_dir}/channel-nockup-${channel}.toml"

        print_info "Fetching latest ${channel} manifest..."

        # Get the latest commit SHA from the master branch
        local commits_url="https://api.github.com/repos/nockchain/nockchain/commits/master"
        local temp_commit="/tmp/commit_${channel}.json"

        if ! download_file "$commits_url" "$temp_commit"; then
            print_warning "Failed to fetch latest commit from GitHub API for ${channel}"
            return 1
        fi

        local latest_commit_sha=""
        latest_commit_sha=$(grep -o "\"sha\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" "$temp_commit" | \
                           sed 's/"sha"[[:space:]]*:[[:space:]]*"\([^"]*\)"/\1/' | head -1) || true

        rm -f "$temp_commit"

        if [[ -z "$latest_commit_sha" ]]; then
            print_warning "No commit SHA found for ${channel}"
            return 1
        fi

        local manifest_url="https://github.com/nockchain/nockchain/releases/download/build-${latest_commit_sha}/${manifest_file}"
        
        print_info "Downloading from: $manifest_url"
        if download_file "$manifest_url" "$output_file"; then
            print_success "Downloaded: channel-nockup-${channel}.toml"
            return 0
        else
            print_warning "Failed to download ${channel} manifest"
            return 1
        fi
    }
    
    local channels=("stable" "nightly")
    local success_count=0
    
    for channel in "${channels[@]}"; do
        print_info "Processing channel: $channel"
        local output_file="${toolchain_dir}/channel-nockup-${channel}.toml"
        
        if [[ -f "$output_file" ]]; then
            print_info "Toolchain file already exists: channel-nockup-${channel}.toml"
            ((success_count++)) || true
            continue
        fi
        
        if get_latest_manifest "$channel"; then
            ((success_count++)) || true
        else
            if [[ "$channel" == "stable" ]]; then
                print_info "Creating minimal channel-nockup-stable.toml fallback"
                cat > "$output_file" << EOF
# Stable channel configuration for nockup
[channel]
name = "stable"
version = "$VERSION"

[binaries]
nockup = "$VERSION"
hoon = "0.1.0"
hoonc = "0.2.0"
EOF
                ((success_count++)) || true
            fi
        fi
    done
    
    print_success "Toolchain directory setup complete"
    print_info "Toolchain files location: $toolchain_dir"
}

# Function to setup config file
setup_config() {
    local config_dir="$HOME/.nockup"
    local config_file="$config_dir/config.toml"
    
    mkdir -p "$config_dir"
    
    if [[ -f "$config_file" ]]; then
        print_info "Config file already exists at: $config_file"
        return 0
    fi
    
    print_step "Downloading default config file"
    print_info "Downloading from: $CONFIG_URL"
    
    if download_file "$CONFIG_URL" "$config_file"; then
        print_success "Downloaded default config to: $config_file"
    else
        print_warning "Failed to download default config file, creating fallback"
        cat > "$config_file" << EOF
# Nockup configuration file
channel = "$CHANNEL"
architecture = "$(uname -m)"
EOF
        print_info "Created minimal config at: $config_file"
    fi
}

# Function to add to PATH
add_to_path() {
    local bin_dir="$1"
    local shell_rc=""

    if [[ -n "${BASH_VERSION:-}" ]]; then
        shell_rc="$HOME/.bashrc"
    elif [[ -n "${ZSH_VERSION:-}" ]]; then
        shell_rc="$HOME/.zshrc"
    elif [[ "$SHELL" == *"zsh"* ]]; then
        shell_rc="$HOME/.zshrc"
    elif [[ "$SHELL" == *"bash"* ]]; then
        shell_rc="$HOME/.bashrc"
    else
        shell_rc="$HOME/.profile"
    fi

    local path_entry="export PATH=\"$bin_dir:\$PATH\""
    
    if [[ -f "$shell_rc" ]] && grep -q "$path_entry" "$shell_rc" 2>/dev/null; then
        print_info "PATH already configured in $shell_rc"
    else
        if [[ -w "$(dirname "$shell_rc")" ]]; then
            touch "$shell_rc"
            
            echo "" >> "$shell_rc"
            echo "# Added by nockup installer" >> "$shell_rc"
            echo "$path_entry" >> "$shell_rc"
            
            print_success "Added $bin_dir to PATH in $shell_rc"
        else
            print_warning "Could not modify $shell_rc (permission denied)"
            print_info "Please manually add this line to your shell configuration:"
            print_info "  $path_entry"
        fi
    fi
    
    local activate_script="$HOME/.nockup/activate.sh"
    cat > "$activate_script" << 'EOF'
#!/bin/bash
# Nockup environment activation script
# Usage: source ~/.nockup/activate.sh
export PATH="$HOME/.nockup/bin:$PATH"
echo "✅ Nockup environment activated!"
echo "📦 nockup is now available in PATH"
EOF
    chmod +x "$activate_script"
    
    print_success "Created activation script: $activate_script"
}

# Function to verify binary works
verify_binary() {
    local binary_path="$1"
    
    if [[ ! -x "$binary_path" ]]; then
        print_error "Binary is not executable: $binary_path"
        return 1
    fi

    if "$binary_path" --help >/dev/null 2>&1; then
        print_success "Binary verification successful"
        return 0
    else
        print_warning "Binary verification failed, but continuing anyway"
        return 0
    fi
}

# Function to setup GPG key (Linux only)
setup_gpg_key() {
    if [[ "$(uname -s)" != "Linux" ]]; then
        return 0
    fi
    
    if ! command_exists gpg; then
        return 0
    fi
    
    local gpg_key="A6FFD2DB7D4C9710"
    
    if gpg --list-keys "$gpg_key" >/dev/null 2>&1; then
        return 0
    fi
    
    gpg --keyserver keyserver.ubuntu.com --recv-keys "$gpg_key" >/dev/null 2>&1 || true
}

# Main installation function
main() {
    print_step "Starting Nockup installation"
    print_info "This installer works on Linux and macOS systems"
    echo "" >&2
    
    # Try to get latest release
    local release_info
    set +e  # Temporarily disable exit on error
    release_info=$(get_latest_release "$CHANNEL")
    local get_release_status=$?
    set -e  # Re-enable exit on error
    
    if [[ $get_release_status -ne 0 ]] || [[ -z "$release_info" ]]; then
        print_error "Failed to fetch latest release information"
        print_info "Please check your internet connection and try again"
        exit 1
    fi
    
    RELEASE_TAG=$(echo "$release_info" | cut -d'|' -f1)
    VERSION=$(echo "$release_info" | cut -d'|' -f2)
    
    print_success "Latest ${CHANNEL} release: ${RELEASE_TAG}"
    print_success "Version: ${VERSION}"
    echo "" >&2
    
    setup_config
    setup_toolchain
    print_info "DEBUG: After setup_toolchain"
    setup_gpg_key
    print_info "DEBUG: After setup_gpg_key"

    local target
    target=$(detect_platform)
    print_info "Target platform: $target"

    local temp_dir
    temp_dir=$(create_temp_dir)
    print_info "Using temporary directory: $temp_dir"

    cleanup() {
        if [[ -n "${temp_dir:-}" ]]; then
            rm -rf "$temp_dir"
        fi
    }
    trap cleanup EXIT

    local archive_name="nockup-${target}.tar.gz"
    local download_url="https://github.com/${GITHUB_REPO}/releases/download/${RELEASE_TAG}/${archive_name}"
    local archive_path="${temp_dir}/${archive_name}"

    print_step "Downloading Nockup binary"
    print_info "URL: $download_url"

    if ! download_file "$download_url" "$archive_path"; then
        print_error "Failed to download Nockup binary"
        print_info "Please check:"
        print_info "  - Your internet connection"
        print_info "  - The release exists at: $download_url"
        exit 1
    fi

    print_success "Downloaded Nockup archive"

    print_step "Extracting Nockup binary"
    
    if ! tar -xzf "$archive_path" -C "$temp_dir"; then
        print_error "Failed to extract archive"
        exit 1
    fi

    local nockup_binary
    nockup_binary=$(find "$temp_dir" -name "nockup" -type f | head -1)
    
    if [[ -z "$nockup_binary" ]]; then
        print_error "Could not find nockup binary in extracted archive"
        print_info "Archive contents:"
        ls -la "$temp_dir" >&2
        exit 1
    fi

    print_success "Extracted Nockup binary"

    local install_dir="$HOME/.nockup/bin"
    local nockup_path="$install_dir/nockup"
    
    print_step "Installing Nockup binary"
    mkdir -p "$install_dir"
    
    cp "$nockup_binary" "$nockup_path"
    chmod +x "$nockup_path"
    
    print_success "Installed Nockup to: $nockup_path"

    verify_binary "$nockup_path"

    add_to_path "$install_dir"

    print_step "Setting channel to $CHANNEL and running installation"
    if "$nockup_path" channel set "$CHANNEL" && "$nockup_path" update; then
        print_success "Nockup installation completed successfully!"
    else
        print_error "Installation failed"
        print_info "You can try running manually:"
        print_info "  $nockup_path channel set $CHANNEL"
        print_info "  $nockup_path update"
        exit 1
    fi

    echo "" >&2
    print_success "🎉 Nockup has been successfully installed!"
    echo "" >&2
    
    echo -e "${YELLOW}═══════════════════════════════════════════════════════════════${NC}" >&2
    echo -e "${GREEN}To use nockup immediately in this terminal, run:${NC}" >&2
    echo "" >&2
    echo -e "  ${CYAN}export PATH=\"$install_dir:\$PATH\"${NC}" >&2
    echo "" >&2
    echo -e "${YELLOW}═══════════════════════════════════════════════════════════════${NC}" >&2
    echo "" >&2
    
    print_info "Next steps:"
    print_info "  1. Run the export command above, OR restart your shell"
    print_info "  2. Verify installation: nockup --help"
    print_info "  3. Create a project: nockup project init"
    echo "" >&2
}

if [[ "${BASH_SOURCE[0]:-$0}" == "${0}" ]]; then
    main "$@"
fi
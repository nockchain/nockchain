#!/bin/bash

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# Global variables for project configuration
PROJECT_NAME=""
PROJECT_TEMPLATE=""
PROJECT_AUTHOR_NAME=""
PROJECT_AUTHOR_EMAIL=""
PROJECT_GITHUB_USERNAME=""
PROJECT_DESCRIPTION=""
DRY_RUN=false
NON_INTERACTIVE=false

# Function to print colored output
print_info() {
    echo -e "${BLUE}ℹ️  $1${NC}"
}

print_success() {
    echo -e "${GREEN}✅ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

print_error() {
    echo -e "${RED}❌ $1${NC}"
}

print_step() {
    echo -e "${CYAN}🚀 $1${NC}"
}

# Function to show usage
show_usage() {
    cat << EOF
Usage: $(basename "$0") [OPTIONS]

NockApp Quick Start Helper - Create a new NockApp project interactively

Options:
  -h, --help              Show this help message
  -n, --name NAME         Project name (skips interactive prompt)
  -t, --template TEMPLATE Template to use (basic|grpc|http-static|http-server|repl|chain|rollup)
  -a, --author NAME       Author name
  -e, --email EMAIL       Author email
  -g, --github USERNAME   GitHub username
  -d, --description DESC  Project description
  --non-interactive       Run without prompts (requires all options)
  --dry-run              Show what would be created without actually creating it

Examples:
  $(basename "$0")                                    # Interactive mode
  $(basename "$0") --name my-app --template basic    # Partial interactive
  $(basename "$0") -n my-app -t basic -a "John Doe" -e john@example.com -g johndoe -d "My app" --non-interactive

Templates:
  basic       - Simple NockApp (recommended for beginners)
  grpc        - gRPC server and client
  http-static - Static file server
  http-server - Dynamic web application
  repl        - Interactive command line
  chain       - Nockchain integration (light client)
  rollup      - Rollup bundler

EOF
}

# Function to check if nockup is available
check_nockup() {
    # Add nockup to PATH if not already there
    export PATH="$HOME/.nockup/bin:$PATH"

    if ! command -v nockup >/dev/null 2>&1; then
        print_error "Nockup not found. Please run the installer first:"
        print_info "  curl -fsSL https://raw.githubusercontent.com/sigilante/nockup/master/install.sh | bash"
        exit 1
    fi

    print_success "Nockup found: $(which nockup)"
}

# Function to get user input with default
get_input() {
    local prompt="$1"
    local default="$2"
    local result

    if [[ -n "$default" ]]; then
        read -p "$prompt (default: $default): " result
        echo "${result:-$default}"
    else
        read -p "$prompt: " result
        echo "$result"
    fi
}

# Function to sanitize input (remove potentially dangerous characters)
sanitize_input() {
    local input="$1"
    # Remove newlines, carriage returns, and other control characters
    echo "$input" | tr -d '\n\r\t' | sed 's/[`$]//g'
}

# Function to validate project name
validate_project_name() {
    local name="$1"

    # Check if name is empty
    if [[ -z "$name" ]]; then
        echo "Project name cannot be empty"
        return 1
    fi

    # Check if name contains invalid characters
    if [[ ! "$name" =~ ^[a-zA-Z0-9_-]+$ ]]; then
        echo "Project name can only contain letters, numbers, hyphens, and underscores"
        return 1
    fi

    # Check if directory already exists
    if [[ -d "$name" ]]; then
        echo "Directory '$name' already exists"
        return 1
    fi

    return 0
}

# Function to validate template
validate_template() {
    local template="$1"
    local valid_templates="basic grpc http-static http-server repl chain rollup"
    
    for valid in $valid_templates; do
        if [[ "$template" == "$valid" ]]; then
            return 0
        fi
    done
    
    echo "Invalid template: $template"
    echo "Valid templates: $valid_templates"
    return 1
}

# Function to show template information
show_template_info() {
    echo ""
    print_info "Available NockApp templates:"
    echo ""
    echo "  1. basic       - Simple NockApp (recommended for beginners)"
    echo "                   Single process, minimal setup"
    echo ""
    echo "  2. grpc        - gRPC server and client"
    echo "                   May require multiple processes"
    echo ""
    echo "  3. http-static - Static file server"
    echo "                   Serve HTML, CSS, JS files"
    echo ""
    echo "  4. http-server - Dynamic web application"
    echo "                   Full web server with routing"
    echo ""
    echo "  5. repl        - Interactive command line"
    echo "                   Read-eval-print loop interface"
    echo ""
    echo "  6. chain       - Nockchain integration"
    echo "                   Connect to Nockchain network (light client)"
    echo ""
    echo "  7. rollup      - Rollup bundler"
    echo "                   Package NockApps for deployment"
    echo ""
}

# Function to get template choice
get_template() {
    local choice
    local template

    show_template_info

    while true; do
        choice=$(get_input "Choose template (1-7)" "1")

        case $choice in
            1|basic) template="basic"; break ;;
            2|grpc) template="grpc"; break ;;
            3|http-static) template="http-static"; break ;;
            4|http-server) template="http-server"; break ;;
            5|repl) template="repl"; break ;;
            6|chain) template="chain"; break ;;
            7|rollup) template="rollup"; break ;;
            *)
                print_warning "Invalid choice. Please enter 1-7 or template name."
                continue
                ;;
        esac
    done

    echo "$template"
}

# Function to get project details interactively
get_project_details_interactive() {
    print_step "Let's create your NockApp project!"
    echo ""

    # Get project name (if not already set)
    if [[ -z "$PROJECT_NAME" ]]; then
        while true; do
            PROJECT_NAME=$(get_input "Enter your project name" "my-nockapp")
            PROJECT_NAME=$(sanitize_input "$PROJECT_NAME")

            if validate_project_name "$PROJECT_NAME"; then
                break
            else
                print_warning "$(validate_project_name "$PROJECT_NAME" 2>&1)"
            fi
        done
    fi

    # Get template (if not already set)
    if [[ -z "$PROJECT_TEMPLATE" ]]; then
        PROJECT_TEMPLATE=$(get_template)
    fi

    # Get author details (if not already set)
    echo ""
    print_info "Author information (for project metadata):"
    
    if [[ -z "$PROJECT_AUTHOR_NAME" ]]; then
        PROJECT_AUTHOR_NAME=$(sanitize_input "$(get_input "Your name" "Your Name")")
    fi
    
    if [[ -z "$PROJECT_AUTHOR_EMAIL" ]]; then
        PROJECT_AUTHOR_EMAIL=$(sanitize_input "$(get_input "Your email" "you@example.com")")
    fi
    
    if [[ -z "$PROJECT_GITHUB_USERNAME" ]]; then
        PROJECT_GITHUB_USERNAME=$(sanitize_input "$(get_input "GitHub username" "yourusername")")
    fi

    # Get description (if not already set)
    if [[ -z "$PROJECT_DESCRIPTION" ]]; then
        PROJECT_DESCRIPTION=$(sanitize_input "$(get_input "Project description" "A NockApp built with Nockup")")
    fi
}

# Function to validate all required fields
validate_config() {
    local missing_fields=()
    
    [[ -z "$PROJECT_NAME" ]] && missing_fields+=("name")
    [[ -z "$PROJECT_TEMPLATE" ]] && missing_fields+=("template")
    [[ -z "$PROJECT_AUTHOR_NAME" ]] && missing_fields+=("author_name")
    [[ -z "$PROJECT_AUTHOR_EMAIL" ]] && missing_fields+=("author_email")
    [[ -z "$PROJECT_GITHUB_USERNAME" ]] && missing_fields+=("github_username")
    [[ -z "$PROJECT_DESCRIPTION" ]] && missing_fields+=("description")
    
    if [[ ${#missing_fields[@]} -gt 0 ]]; then
        print_error "Missing required fields: ${missing_fields[*]}"
        return 1
    fi
    
    # Validate template
    if ! validate_template "$PROJECT_TEMPLATE"; then
        return 1
    fi
    
    # Validate project name
    if ! validate_project_name "$PROJECT_NAME"; then
        return 1
    fi
    
    return 0
}

# Function to create manifest file
create_manifest() {
    local manifest_file="${PROJECT_NAME}.toml"

    print_step "Creating project manifest: $manifest_file"

    cat > "$manifest_file" << EOF
# NockApp Project Configuration
# Generated by Nockup Quick Start

[project]
name = "$PROJECT_DESCRIPTION"
project_name = "$PROJECT_NAME"
version = "0.1.0"
description = "$PROJECT_DESCRIPTION"
author_name = "$PROJECT_AUTHOR_NAME"
author_email = "$PROJECT_AUTHOR_EMAIL"
github_username = "$PROJECT_GITHUB_USERNAME"
license = "MIT"
keywords = ["nockapp", "nockchain", "hoon"]
nockapp_commit_hash = "336f744b6b83448ec2b86473a3dec29b15858999"
template = "$PROJECT_TEMPLATE"

# Libraries: Uncomment and modify as needed
# [libraries.sequent]
# url = "https://github.com/jackfoxy/sequent"
# commit = "0f6e6777482447d4464948896b763c080dc9e559"

# [libraries.bits]
# url = "https://github.com/urbit/urbit"
# branch = "develop"
# file = "pkg/arvo/lib/bits.hoon"

# [libraries.math]
# url = "https://github.com/urbit/numerics"
# branch = "main"
# directory = "libmath"
# commit = "7c11c48ab3f21135caa5a4e8744a9c3f828f2607"
EOF

    print_success "Created manifest: $manifest_file"
}

# Function to initialize project
initialize_project() {
    local manifest_file="${PROJECT_NAME}.toml"

    print_step "Initializing NockApp project with Nockup..."

    if nockup start "$manifest_file"; then
        print_success "Project initialized successfully!"
        return 0
    else
        print_error "Project initialization failed"
        print_info "You can try manually:"
        print_info "  nockup start $manifest_file"
        return 1
    fi
}

# Function to show next steps
show_next_steps() {
    echo ""
    print_success "🎉 Your NockApp project '$PROJECT_NAME' is ready!"
    echo ""
    print_info "📁 Project structure created in: ./$PROJECT_NAME/"
    print_info "🌙 Hoon code: ./$PROJECT_NAME/hoon/app/app.hoon"
    print_info "🦀 Rust code: ./$PROJECT_NAME/src/main.rs"
    print_info "📋 Configuration: ./$PROJECT_NAME/manifest.toml"
    echo ""
    print_info "🚀 Next steps:"
    echo "  cd $PROJECT_NAME"
    echo "  nockup build ."
    echo "  nockup run ."
    echo ""

    # Template-specific instructions
    case "$PROJECT_TEMPLATE" in
        "grpc")
            print_info "💡 gRPC template may need multiple processes"
            echo ""
            ;;
        "chain")
            print_info "💡 Chain template connects to Nockchain network"
            print_info "   Uses light client - no additional setup needed"
            echo ""
            ;;
        "http-static"|"http-server")
            print_info "💡 Web server template:"
            print_info "   Your app will be available at a local URL after running"
            echo ""
            ;;
    esac

    print_info "📚 To learn more:"
    print_info "  • Edit the Hoon kernel: $PROJECT_NAME/hoon/app/app.hoon"
    print_info "  • Customize the Rust code: $PROJECT_NAME/src/main.rs"
    print_info "  • Add libraries by editing: $PROJECT_NAME/manifest.toml"
    print_info "  • View documentation: nockup --help"
}

# Function to show dry run summary
show_dry_run_summary() {
    echo ""
    print_info "=== DRY RUN - No files will be created ==="
    echo ""
    print_info "Project Configuration:"
    print_info "  Name:         $PROJECT_NAME"
    print_info "  Template:     $PROJECT_TEMPLATE"
    print_info "  Author:       $PROJECT_AUTHOR_NAME"
    print_info "  Email:        $PROJECT_AUTHOR_EMAIL"
    print_info "  GitHub:       $PROJECT_GITHUB_USERNAME"
    print_info "  Description:  $PROJECT_DESCRIPTION"
    echo ""
    print_info "Would create:"
    print_info "  • Manifest file: ${PROJECT_NAME}.toml"
    print_info "  • Project directory: ./${PROJECT_NAME}/"
    print_info "  • Hoon code: ./${PROJECT_NAME}/hoon/app/app.hoon"
    print_info "  • Rust code: ./${PROJECT_NAME}/src/main.rs"
    echo ""
}

# Parse command line arguments
parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            -h|--help)
                show_usage
                exit 0
                ;;
            -n|--name)
                PROJECT_NAME=$(sanitize_input "$2")
                shift 2
                ;;
            -t|--template)
                PROJECT_TEMPLATE="$2"
                shift 2
                ;;
            -a|--author)
                PROJECT_AUTHOR_NAME=$(sanitize_input "$2")
                shift 2
                ;;
            -e|--email)
                PROJECT_AUTHOR_EMAIL=$(sanitize_input "$2")
                shift 2
                ;;
            -g|--github)
                PROJECT_GITHUB_USERNAME=$(sanitize_input "$2")
                shift 2
                ;;
            -d|--description)
                PROJECT_DESCRIPTION=$(sanitize_input "$2")
                shift 2
                ;;
            --non-interactive)
                NON_INTERACTIVE=true
                shift
                ;;
            --dry-run)
                DRY_RUN=true
                shift
                ;;
            *)
                print_error "Unknown option: $1"
                show_usage
                exit 1
                ;;
        esac
    done
}

# Cleanup function for error handling
cleanup_on_error() {
    local manifest_file="${PROJECT_NAME}.toml"
    
    if [[ -f "$manifest_file" ]]; then
        print_warning "Cleaning up manifest file: $manifest_file"
        rm -f "$manifest_file"
    fi
}

# Main function
main() {
    # Set up error handling
    trap cleanup_on_error ERR
    
    print_step "NockApp Quick Start Helper"
    print_info "This tool helps you create your first NockApp project"
    echo ""

    # Parse command line arguments
    parse_args "$@"

    # Check if nockup is available
    check_nockup

    # Get project details (interactive or from args)
    if [[ "$NON_INTERACTIVE" != "true" ]]; then
        get_project_details_interactive
    fi
    
    # Validate configuration
    if ! validate_config; then
        print_error "Configuration validation failed"
        if [[ "$NON_INTERACTIVE" == "true" ]]; then
            print_info "In non-interactive mode, all required fields must be provided"
            show_usage
        fi
        exit 1
    fi

    echo ""
    print_step "Creating your NockApp project..."
    print_info "Project: $PROJECT_NAME"
    print_info "Template: $PROJECT_TEMPLATE"
    print_info "Author: $PROJECT_AUTHOR_NAME"
    echo ""

    # Dry run mode
    if [[ "$DRY_RUN" == "true" ]]; then
        show_dry_run_summary
        exit 0
    fi

    # Create manifest file
    create_manifest

    # Initialize project with nockup
    if initialize_project; then
        show_next_steps
    else
        print_warning "Project creation completed with errors"
        print_info "Check the manifest file and try running nockup start manually"
        exit 1
    fi
}

# Check if we're being sourced or executed
if [[ "${BASH_SOURCE[0]:-$0}" == "${0}" ]]; then
    main "$@"
fi

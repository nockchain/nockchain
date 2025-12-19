#!/usr/bin/env python3
"""
Scan Hoon files and generate a registry.toml in the typhoon format.

Usage:
    python scan-deps-v2.py --workspace nockchain --root-path hoon \\
        --git-url https://github.com/nockchain/nockchain --ref a19ad4dc \\
        /path/to/nockchain/hoon
"""

import os
import sys
import argparse
from pathlib import Path
from collections import defaultdict
from typing import Dict, List, Set, Tuple, Optional

def get_dependencies(file_path: Path) -> List[str]:
    """
    Extract dependencies from a Hoon file.
    Returns list of dependency names (without paths, e.g., "zeke", "one")
    """
    deps = []
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                # Stop at first non-import line
                if not (line.startswith('/') or line.startswith('::')):
                    break
                if line.startswith('::'):  # Comment
                    continue

                parts = line.split()
                if not parts:
                    continue

                prefix = parts[0]
                if prefix in ('/+', '/-', '/#'):
                    # Parse: /+ foo, bar, *baz=qux
                    dep_str = ' '.join(parts[1:])
                    for d in dep_str.split(','):
                        d = d.strip()
                        # Remove * prefix
                        if d.startswith('*'):
                            d = d[1:]
                        # Handle renaming: *foo=bar -> bar
                        if '=' in d:
                            d = d.split('=')[-1].strip()
                        if d:
                            deps.append(d)
                elif prefix == '/=':
                    # Parse old clay syntax: /= name /path/to/file
                    # Example: /= common /common/v0-v1/table/compute
                    # Example: /= * /common/zeke
                    # The path is absolute from workspace root
                    if len(parts) >= 3:
                        path = parts[2]
                        # Strip leading slash
                        if path.startswith('/'):
                            path = path[1:]
                        # The dependency name is the full path from workspace root
                        # This will be resolved later based on the files dict
                        if path:
                            deps.append(path)
    except Exception as e:
        print(f"Warning: Could not read {file_path}: {e}", file=sys.stderr)

    return deps

def find_hoon_files(directory: Path) -> Dict[str, Tuple[Path, str, str]]:
    """
    Find all .hoon files in directory.
    Returns dict: package_key -> (full_path, install_path, filename)

    Package key: subdirectory structure within scan directory + filename
    Install path: full path from root_path (includes scan directory name)

    Example:
        Scanning: /repo/hoon/common/
        File at: /repo/hoon/common/ztd/eight.hoon
        Returns: "ztd/eight" -> (Path(...), "common/ztd", "eight.hoon")

        Package name will be: workspace/ztd/eight
        Install path will be: common/ztd
    """
    files = {}

    for hoon_file in directory.rglob("*.hoon"):
        filename = hoon_file.stem  # Without .hoon extension

        # Get path relative to scan directory
        rel_dir = hoon_file.parent.relative_to(directory)
        rel_dir_str = str(rel_dir).replace(os.sep, '/') if str(rel_dir) != '.' else ""

        # Package key: subdirectory path within scan dir + filename
        if rel_dir_str:
            package_key = f"{rel_dir_str}/{filename}"
        else:
            package_key = filename

        # Install path: scan directory name + subdirectory path
        scan_dir_name = directory.name
        if rel_dir_str:
            install_path = f"{scan_dir_name}/{rel_dir_str}"
        else:
            install_path = scan_dir_name

        files[package_key] = (hoon_file, install_path, hoon_file.name)

    return files

def resolve_dependency(
    dep_name: str,
    current_file_dir: str,
    files: Dict[str, Tuple[Path, str, str]]
) -> Optional[str]:
    """
    Resolve a dependency name to its full key in the files dict.

    Search order:
    1. If dep_name is a full path (contains /), try it directly
    2. Same directory as current file: {current_dir}/{dep_name}
    3. Sibling directories: {parent}/{dep_name}
    4. Root level: {dep_name}
    5. Anywhere in tree (last resort)

    Args:
        dep_name: dependency name like "types" or full path like "common/v0-v1/table/compute"
        current_file_dir: directory of the file with the import (e.g., "wallet")
        files: dict of all files

    Returns:
        Full key like "wallet/types" or None if not found
    """
    # If it's already a full path (from /= syntax), try it directly
    if '/' in dep_name:
        if dep_name in files:
            return dep_name
        # Also try matching just the basename if full path doesn't work
        basename = dep_name.split('/')[-1]
        dep_name = basename

    # Try same directory first
    if current_file_dir:
        same_dir_key = f"{current_file_dir}/{dep_name}"
        if same_dir_key in files:
            return same_dir_key

    # Try root level
    if dep_name in files:
        return dep_name

    # Try parent directory siblings
    if current_file_dir:
        parts = current_file_dir.split('/')
        if len(parts) > 1:
            parent = '/'.join(parts[:-1])
            sibling_key = f"{parent}/{dep_name}"
            if sibling_key in files:
                return sibling_key

    # Last resort: search all files for matching basename
    matches = [k for k in files.keys() if k.endswith(f"/{dep_name}") or k == dep_name]
    if len(matches) == 1:
        return matches[0]
    elif len(matches) > 1:
        print(f"Warning: Ambiguous dependency '{dep_name}' from {current_file_dir}, found: {matches}",
              file=sys.stderr)
        return matches[0]  # Return first match

    return None

def build_dependency_graph(files: Dict[str, Tuple[Path, str, str]], scan_dir: Path) -> Dict[str, List[str]]:
    """
    Build dependency graph by scanning all files.
    Returns dict: full_key -> list of resolved dependency full_keys
    """
    graph = {}

    # Get the basename of the scan directory to handle /common/ prefix stripping
    scan_dir_name = scan_dir.name  # e.g., "common"

    for full_key, (file_path, file_dir, _) in files.items():
        raw_deps = get_dependencies(file_path)

        # Resolve each dependency to its full key
        resolved_deps = []
        for dep_name in raw_deps:
            # Strip the scan directory name from the dependency path if present
            # E.g., if scanning "common/", strip "common/" from "common/v0-v1/table/compute"
            if dep_name.startswith(f"{scan_dir_name}/"):
                dep_name = dep_name[len(scan_dir_name)+1:]

            resolved = resolve_dependency(dep_name, file_dir, files)
            if resolved:
                resolved_deps.append(resolved)
            else:
                print(f"Warning: Could not resolve dependency '{dep_name}' in {full_key}",
                      file=sys.stderr)

        graph[full_key] = resolved_deps

    return graph

def generate_registry_toml(
    workspace_name: str,
    git_url: str,
    ref: str,
    description: str,
    root_path: str,
    files: Dict[str, Tuple[Path, str, str]],
    graph: Dict[str, List[str]],
    scan_dir: Path
) -> str:
    """
    Generate registry TOML in the typhoon format.

    The root_path parameter specifies where meaningful paths begin in the repo.
    For example, if root_path = "hoon", then scanning hoon/common/ should produce:
    - Package name: workspace/zeke (not workspace/common/zeke)
    - Path: common (not hoon/common)
    """
    lines = []

    # Header comment
    lines.append("# ============================================================================")
    lines.append(f"# {workspace_name} workspace packages")
    lines.append("# Generated by scan-deps-v2.py")
    lines.append("# ============================================================================")
    lines.append("")

    # Workspace definition
    lines.append(f"[workspace.{workspace_name}]")
    lines.append(f'git_url = "{git_url}"')
    lines.append(f'ref = "{ref}"')
    lines.append(f'description = "{description}"')
    lines.append(f'root_path = "{root_path}"')
    lines.append("")

    # Sort files by path for consistent output
    sorted_files = sorted(files.items(), key=lambda x: x[0])

    # Collect aliases while generating packages
    aliases_to_generate = []

    # Package definitions
    for package_key, (_, install_path, hoon_filename) in sorted_files:
        lines.append("[[package]]")
        # Package name: workspace + subdirectory path within scan dir
        lines.append(f'name = "{workspace_name}/{package_key}"')
        lines.append(f'workspace = "{workspace_name}"')
        # Path: full path from root_path (includes scan directory name)
        lines.append(f'path = "{install_path}"')
        lines.append(f'file = "{hoon_filename}"')

        # Dependencies - use package keys
        deps = graph.get(package_key, [])
        if deps:
            dep_list = ', '.join(f'"{workspace_name}/{d}"' for d in deps)
            lines.append(f'dependencies = [{dep_list}]')
        else:
            lines.append('dependencies = []')

        lines.append("")

        # Collect aliases for special cases
        # For ztd/one through ztd/eight, create short aliases
        filename_stem = hoon_filename.replace('.hoon', '')
        if package_key.startswith('common/ztd/') and filename_stem in ['one', 'two', 'four', 'five', 'six', 'seven', 'eight']:
            aliases_to_generate.append((f'{workspace_name}/{filename_stem}', f'{workspace_name}/{package_key}'))

    # Special-case aliases
    special_aliases = {
        'zeke': 'common/zeke',
        'zoon': 'common/zoon',
        'zose': 'common/zose',
    }
    for alias_name, target_path in special_aliases.items():
        full_target_key = f"{workspace_name}/{target_path}"
        if full_target_key in [f"{workspace_name}/{k}" for k in files.keys()]:
            aliases_to_generate.append((f"{workspace_name}/{alias_name}", full_target_key))

    # Generate alias sections
    if aliases_to_generate:
        lines.append("# ============================================================================")
        lines.append("# Aliases")
        lines.append("# ============================================================================")
        lines.append("")
        for alias_name, target_name in aliases_to_generate:
            lines.append("[[alias]]")
            lines.append(f'name = "{alias_name}"')
            lines.append(f'target = "{target_name}"')
            lines.append("")

    return '\n'.join(lines)

def main():
    parser = argparse.ArgumentParser(
        description='Scan Hoon files and generate registry TOML',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Example:
  python scan-deps-v2.py --workspace nockchain --root-path hoon \\
      --git-url https://github.com/nockchain/nockchain --ref a19ad4dc \\
      --description "Nockchain standard library" \\
      /path/to/nockchain/hoon
""")

    parser.add_argument('directory', type=Path,
                        help='Directory to scan for .hoon files')
    parser.add_argument('--workspace', required=True,
                        help='Workspace name (e.g., "nockchain")')
    parser.add_argument('--git-url', required=True,
                        help='Git repository URL')
    parser.add_argument('--ref', required=True,
                        help='Git ref (tag or commit hash)')
    parser.add_argument('--root-path', required=True,
                        help='Root path in repo (e.g., "hoon", "pkg/arvo")')
    parser.add_argument('--description', default='',
                        help='Workspace description')
    parser.add_argument('--output', '-o', type=Path,
                        help='Output file (default: stdout)')

    args = parser.parse_args()

    if not args.directory.is_dir():
        print(f"Error: {args.directory} is not a directory", file=sys.stderr)
        sys.exit(1)

    # Find all Hoon files
    files = find_hoon_files(args.directory)
    print(f"Found {len(files)} Hoon files", file=sys.stderr)

    # Build dependency graph
    graph = build_dependency_graph(files, args.directory)

    # Generate TOML
    toml_output = generate_registry_toml(
        workspace_name=args.workspace,
        git_url=args.git_url,
        ref=args.ref,
        description=args.description,
        root_path=args.root_path,
        files=files,
        graph=graph,
        scan_dir=args.directory
    )

    # Output
    if args.output:
        args.output.write_text(toml_output)
        print(f"Wrote registry to {args.output}", file=sys.stderr)
    else:
        print(toml_output)

if __name__ == "__main__":
    main()

{
  description = "Dev shell with all tooling to build nockchain via Cargo and Bazel";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-analyzer-src.follows = "";
    };

    foundry = {
      url = "github:shazow/foundry.nix/monthly";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, fenix, foundry, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            fenix.overlays.default
            foundry.overlay
            (final: prev: {
              cargo-sort = prev.rustPlatform.buildRustPackage rec {
                pname = "cargo-sort";
                version = "2.0.2";
                src = prev.fetchCrate {
                  inherit pname version;
                  sha256 = "1i6zy2yikf7vrqawxk5v4i344limb4p9l8k62vxahghjsn8dmwjk";
                };
                cargoHash = "sha256-FoFzBf24mNDTRBfFyTEr9Q7sJjUhs0X/XWRGEoierQ4=";
                doCheck = false;
              };
              cargo-audit = prev.rustPlatform.buildRustPackage rec {
                pname = "cargo-audit";
                version = "0.22.0";
                src = prev.fetchCrate {
                  inherit pname version;
                  sha256 = "sha256-Ha2yVyu9331NaqiW91NEwCTIeW+3XPiqZzmatN5KOws=";
                };
                cargoHash = "sha256-f8nrW1l7UA8sixwqXBD1jCJi9qyKC5tNl/dWwCt41Lk=";
                doCheck = false;
              };
            })
          ];
        };
        lib = pkgs.lib;

        # Use the specific nightly date from rust-toolchain.toml (nightly-2025-11-26)
        rustToolchainManifest = {
          channel = "nightly";
          date = "2025-11-26";
          sha256 = "sha256-bYq1DZv2iwLeZAQwA1nqJQgx7p1M/srnZyr1FYJ3+GU=";
        };
        rustToolchainBase = (fenix.packages.${system}.toolchainOf rustToolchainManifest).withComponents [
          "cargo"
          "clippy"
          "rust-src"
          "rustc"
          "rustfmt"
          "llvm-tools-preview"
          "miri"
        ];
        rustToolchainTargetStd =
          (fenix.packages.${system}.targets."x86_64-unknown-linux-gnu".toolchainOf rustToolchainManifest).rust-std;
        rustToolchain = fenix.packages.${system}.combine [
          rustToolchainBase
          rustToolchainTargetStd
        ];

        # Build cargo-deny with our nightly toolchain since 0.18.9 requires rustc 1.88+
        cargoDeny = pkgs.rustPlatform.buildRustPackage rec {
          pname = "cargo-deny";
          version = "0.18.9";
          src = pkgs.fetchCrate {
            inherit pname version;
            sha256 = "sha256-WnIkb4OXutgufNWpFooKQiJ5TNhamtTsFJu8bWyWeR4=";
          };
          cargoHash = "sha256-2u1DQtvjRfwbCXnX70M7drrMEvNsrVxsbikgrnNOkUE=";
          nativeBuildInputs = [ pkgs.pkg-config rustToolchain ];
          buildInputs = [ pkgs.zstd ];
          # Override the rust toolchain used for building
          CARGO = "${rustToolchain}/bin/cargo";
          RUSTC = "${rustToolchain}/bin/rustc";
          doCheck = false;
        };

        llvmPkgs = pkgs.llvmPackages_latest;
        origClangPrefix = lib.strings.removeSuffix "\n" (builtins.readFile "${llvmPkgs.clang}/nix-support/orig-cc");

        # Use the unwrapped clang, but inject platform-specific flags:
        # - On Darwin: inject Apple SDK sysroot so headers like <stdalign.h> are discoverable
        # - On Linux: inject dynamic linker path so binaries linked with lld have proper INTERP
        #   (needed because -fuse-ld=<full-path> bypasses the normal Nix wrapper that adds this)
        clangWithSysroot = pkgs.writeShellScriptBin "clang-with-sysroot" (''
          set -euo pipefail
          extra_args=()
          if [ -n "''${LIBCLANG_PATH:-}" ] && [ -d "''${LIBCLANG_PATH%/}/clang" ]; then
            _clang_ver="$(${pkgs.coreutils}/bin/ls -1 "''${LIBCLANG_PATH%/}/clang" | ${pkgs.coreutils}/bin/head -n 1)"
            extra_args+=(-resource-dir "''${LIBCLANG_PATH%/}/clang/$_clang_ver")
          fi
        '' + lib.optionalString pkgs.stdenv.isLinux ''
          # On Linux, when using -fuse-ld=<full-path-to-lld>, clang bypasses the normal
          # wrapper logic that adds --dynamic-linker. We need to add it explicitly.
          # Only add for link operations (not compile-only: -c, -S, -E, -M flags).
          # Also add RPATH so binaries can find glibc/libgcc at runtime (needed for
          # test binaries compiled by build.rs scripts like aws-lc-sys).
          _is_link=true
          for arg in "$@"; do
            case "$arg" in
              -c|-S|-E|-M|-MM) _is_link=false; break ;;
            esac
          done
          if $_is_link; then
            extra_args+=(-Wl,--dynamic-linker=${pkgs.stdenv.cc.libc}/lib/ld-linux-x86-64.so.2)
            extra_args+=(-L${pkgs.stdenv.cc.cc.lib}/lib)
            extra_args+=(-Wl,-rpath=${pkgs.stdenv.cc.libc}/lib)
            extra_args+=(-Wl,-rpath=${pkgs.stdenv.cc.cc.lib}/lib)
          fi
        '' + ''
          if [ -n "''${SDKROOT:-}" ]; then
            exec ${origClangPrefix}/bin/clang "''${extra_args[@]}" -isysroot "$SDKROOT" "$@"
          fi
          exec ${origClangPrefix}/bin/clang "''${extra_args[@]}" "$@"
        '');
        clangxxWithSysroot = pkgs.writeShellScriptBin "clangxx-with-sysroot" (''
          set -euo pipefail
          extra_args=()
          if [ -n "''${LIBCLANG_PATH:-}" ] && [ -d "''${LIBCLANG_PATH%/}/clang" ]; then
            _clang_ver="$(${pkgs.coreutils}/bin/ls -1 "''${LIBCLANG_PATH%/}/clang" | ${pkgs.coreutils}/bin/head -n 1)"
            extra_args+=(-resource-dir "''${LIBCLANG_PATH%/}/clang/$_clang_ver")
          fi
        '' + lib.optionalString pkgs.stdenv.isLinux ''
          # On Linux, when using -fuse-ld=<full-path-to-lld>, clang bypasses the normal
          # wrapper logic that adds --dynamic-linker. We need to add it explicitly.
          # Only add for link operations (not compile-only: -c, -S, -E, -M flags).
          # Also add RPATH so binaries can find glibc/libgcc at runtime (needed for
          # test binaries compiled by build.rs scripts like aws-lc-sys).
          _is_link=true
          for arg in "$@"; do
            case "$arg" in
              -c|-S|-E|-M|-MM) _is_link=false; break ;;
            esac
          done
          if $_is_link; then
	    extra_args+=(-Wl,--dynamic-linker=${pkgs.stdenv.cc.libc}/lib/ld-linux-x86-64.so.2)
            extra_args+=(-L${pkgs.stdenv.cc.cc.lib}/lib)
            extra_args+=(-Wl,-rpath=${pkgs.stdenv.cc.libc}/lib)
            extra_args+=(-Wl,-rpath=${pkgs.stdenv.cc.cc.lib}/lib)
          fi
        '' + ''
          if [ -n "''${SDKROOT:-}" ]; then
            exec ${origClangPrefix}/bin/clang++ "''${extra_args[@]}" -isysroot "$SDKROOT" "$@"
          fi
          exec ${origClangPrefix}/bin/clang++ "''${extra_args[@]}" "$@"
        '');

        cargoTools = with pkgs; [
          rustToolchain
          cargoDeny
          cargo-audit
          cargo-udeps
          cargo-sort
          cargo-pgo
          cargo-zigbuild
          cargo-nextest
          rust-script
        ];

        # Bazelisk upstream recommends invoking it via a `bazel` symlink/rename so it
        # transparently selects the pinned Bazel version from `.bazelversion`.
        # Nixpkgs' bazelisk package ships as `bazelisk`, so we provide `bazel`.
        bazelWithBazelisk = pkgs.symlinkJoin {
          name = "bazel-with-bazelisk";
          paths = [ pkgs.bazelisk ];
          postBuild = ''
            ln -s bazelisk $out/bin/bazel
          '';
        };

        buildTools =
          (with pkgs; [
          bazelWithBazelisk
          buf
          yq-go  # mikefarah/yq (not python3Packages.yq which is a jq wrapper)
          jq
          pkg-config
          cmake
          openssl
          zlib
          protobuf
          llvmPkgs.clang
          clangWithSysroot
          clangxxWithSysroot
          llvmPkgs.lld
          llvmPkgs.llvm
          llvmPkgs.libclang
          zig
          nodejs
          python3
          git
          curl
          wget
          tmux
          tree
          buildifier
          # For jemalloc build (autotools needed by tikv-jemalloc-sys)
          autoconf
          automake
          # For testing gRPC APIs on the cheap-n-cheerful
          grpcurl
          sqlite
        ])
        # On macOS, Bazel's C++ toolchain expects Apple `libtool` semantics for archiving.
        # If GNU libtool is first on PATH, Bazel's auto-configured toolchain will pick it and fail.
        ++ lib.optionals pkgs.stdenv.isDarwin [ pkgs.darwin.cctools ]
        ++ (with pkgs; [
          libtool
          # Foundry (forge, cast, anvil) for Solidity contracts
          foundry-bin
          # nice tools
          fd
          ripgrep
        ]);
      in
      {
        devShells.default = pkgs.mkShell {
          packages = cargoTools ++ buildTools;

          shellHook = ''
            # On macOS, Bazel/rules_rust may try to call `xcrun --show-sdk-path` to locate the SDK.
            # In nix develop, PATH may not include Xcode's tools, so we set SDKROOT explicitly via
            # /usr/bin/xcrun (and `.bazelrc` is configured to forward SDKROOT into sandboxed actions).
            ${lib.optionalString pkgs.stdenv.isDarwin ''
            if [ -x /usr/bin/xcrun ]; then
              export SDKROOT="$(/usr/bin/xcrun --sdk macosx --show-sdk-path)"
            fi
            export MACOSX_DEPLOYMENT_TARGET=${pkgs.stdenv.hostPlatform.darwinMinVersion or "11.0"}
            export LIBICONV_LIB="${pkgs.libiconv}/lib"
            ''}
            export CC=${clangWithSysroot}/bin/clang-with-sysroot
            export CXX=${clangxxWithSysroot}/bin/clangxx-with-sysroot
            export AR=${llvmPkgs.llvm}/bin/llvm-ar
            export RANLIB=${llvmPkgs.llvm}/bin/llvm-ranlib
            # Only export LD on Linux. On macOS, cc_configure constructs -fuse-ld=<full-path>
            # which clang rejects ("invalid linker name"). Let macOS use the default linker.
            ${lib.optionalString pkgs.stdenv.isLinux ''
            export LD=${llvmPkgs.lld}/bin/ld.lld
            ''}
            export LIBCLANG_PATH=${llvmPkgs.libclang.lib}/lib
            export FOUNDRY_DISABLE_NIGHTLY_WARNING=1
            # Zig links can exceed macOS' default fd limit.
            ulimit -n 8192 || true
            # Ensure Nix Rust toolchain is used even if user PATH has rustup shims.
            export PATH="${rustToolchain}/bin:$PATH"
            export CARGO="${rustToolchain}/bin/cargo"
            export RUSTC="${rustToolchain}/bin/rustc"
            export RUSTDOC="${rustToolchain}/bin/rustdoc"
            # Add libstdc++ to library paths for lld linker
            # LIBRARY_PATH is for compile-time linking, LD_LIBRARY_PATH is for runtime loading
            # NOTE: We do NOT append to parent shell's $LIBRARY_PATH/$LD_LIBRARY_PATH to ensure
            # deterministic values for Bazel cache keys. The Nix shell provides all needed paths.
            export LIBRARY_PATH="${pkgs.stdenv.cc.cc.lib}/lib:${pkgs.zlib}/lib"
            export LD_LIBRARY_PATH="${pkgs.stdenv.cc.cc.lib}/lib:${pkgs.zlib}/lib"
            ${lib.optionalString pkgs.stdenv.isDarwin ''
            # Ensure libc++ is discoverable for Rust tools that link via lld.
            # These append to the LIBRARY_PATH/LD_LIBRARY_PATH we just set above (not parent shell).
            export LIBRARY_PATH="${llvmPkgs.libcxx}/lib:$LIBRARY_PATH"
            export LD_LIBRARY_PATH="${llvmPkgs.libcxx}/lib:$LD_LIBRARY_PATH"
            ''}
            # C++ stdlib headers for C/C++ builds inside Bazel sandboxes.
            # On macOS we use libc++ (from the Nix LLVM toolchain). On Linux, stdenv provides libstdc++.
            # NOTE: We do NOT append to parent shell's $CPLUS_INCLUDE_PATH for deterministic Bazel cache keys.
            ${lib.optionalString pkgs.stdenv.isDarwin ''
            export CPLUS_INCLUDE_PATH="${llvmPkgs.libcxx.dev}/include/c++/v1"
            ''}
            ${lib.optionalString pkgs.stdenv.isLinux ''
            # Note: gcc version string is e.g. "14-20241116", not just "14"
            export CPLUS_INCLUDE_PATH="${pkgs.stdenv.cc.cc}/include/c++/${pkgs.stdenv.cc.cc.version}:${pkgs.stdenv.cc.cc}/include/c++/${pkgs.stdenv.cc.cc.version}/${pkgs.stdenv.hostPlatform.config}"
            ''}

          '';
        };
      });
}

#!/usr/bin/env bash
# Type-checks the platform-neutral crates for Android so a dependency that
# cannot build there is caught early. Android itself is post-MVP (spec §10).
#
# Needs the Android NDK (ring compiles C with its clang). Set ANDROID_NDK_HOME,
# or ANDROID_NDK_LATEST_HOME as on GitHub runners.
set -euo pipefail

CRATES=(tgx-transport)
TARGETS=(aarch64-linux-android armv7-linux-androideabi x86_64-linux-android)
API=26

ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}"
if [[ -z "$ndk" || ! -d "$ndk" ]]; then
    echo "error: set ANDROID_NDK_HOME to an Android NDK directory" >&2
    exit 1
fi

case "$(uname -s)" in
    Linux*) host=linux-x86_64; ext="" ;;
    Darwin*) host=darwin-x86_64; ext="" ;;
    MINGW* | MSYS* | CYGWIN*) host=windows-x86_64; ext=".cmd" ;;
    *) echo "error: unsupported host $(uname -s)" >&2; exit 1 ;;
esac
bin="$ndk/toolchains/llvm/prebuilt/$host/bin"

pkgs=()
for c in "${CRATES[@]}"; do pkgs+=(-p "$c"); done

for target in "${TARGETS[@]}"; do
    # The NDK's clang wrapper is named armv7a-, not armv7-.
    clang_triple="${target/armv7-linux-androideabi/armv7a-linux-androideabi}"
    var="${target//-/_}"
    export "CC_$var=$bin/$clang_triple$API-clang$ext"
    export "AR_$var=$bin/llvm-ar${ext:+.exe}"
    rustup target add "$target" >/dev/null 2>&1
    echo "== cargo check --target $target"
    cargo check --locked "${pkgs[@]}" --target "$target"
done
echo "android check ok"

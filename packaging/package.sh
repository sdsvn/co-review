#!/usr/bin/env bash
# Builds Co-Review (the app and the `co-review` command) and packages it for this machine, as a release asset:
#
#   macOS (Apple silicon)  Co-Review-mac-arm64.zip         Co-Review.app
#   Linux (x64, arm64)     Co-Review-linux-<arch>.tar.gz   Co-Review/ (install.sh puts it in ~/.local/share/co-review)
#   Windows (x64)          Co-Review-win-x64.exe           an installer (Inno Setup), per user, no admin rights
#
# The layout, which install.sh, the `co-review` command (crates/review_cli/src/app.rs) and the app's updater
# (lapce-app/src/update.rs) rely on:
#
#   macOS                                   Linux / Windows
#   Co-Review.app/Contents/                 Co-Review/
#     MacOS/Co-Review          the app        co-review-app | Co-Review.exe
#     Resources/co-review.icns
#     Resources/app/                          resources/app/
#       version                the app's version (Cargo.toml's workspace.package)
#       bin/co-review[.exe]    the `co-review` command (install-cli.sh links it into PATH)
#       bin/install-cli.sh
#       plugin/ .claude-plugin/ the Claude Code plugin and its marketplace (Install Agent Skills…)
#       integrations/{pi,omp,shared}  the Pi and Oh My Pi packages (co-review setup pi|omp)
#       icons/co-review.png
#       LICENSE NOTICE
#
# The command lives under resources/app/bin, not beside the app: case-insensitive file systems (macOS, Windows) can't
# hold `Co-Review` and `co-review` in one directory.
#
# Environment:
#   PROFILE     cargo profile (default: release; the release workflow uses release-lto)
#   OUT         where the asset goes (default: target/package)
#   SKIP_BUILD  1 to package binaries already built with PROFILE
#   CARGO_TARGET_DIR  as for cargo
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -n 1)
[ -n "$version" ] || { echo "package: no version in Cargo.toml's [workspace.package]" >&2; exit 1; }
profile=${PROFILE:-release}
out=${OUT:-$root/target/package}
target_dir=${CARGO_TARGET_DIR:-$root/target}
case "$profile" in
dev) build_dir="$target_dir/debug" ;;
*) build_dir="$target_dir/$profile" ;;
esac

arch=$(uname -m)
case "$arch" in
arm64 | aarch64) arch=arm64 ;;
x86_64 | amd64) arch=x64 ;;
*) echo "package: unsupported CPU $arch" >&2; exit 1 ;;
esac
case "$(uname -s)" in
Darwin) os=mac ;;
Linux) os=linux ;;
MINGW* | MSYS* | CYGWIN*) os=win ;;
*) echo "package: unsupported system $(uname -s)" >&2; exit 1 ;;
esac
exe=""
[ "$os" = win ] && exe=.exe

if [ "${SKIP_BUILD:-}" != 1 ]; then
    # A release tag makes a stable build (lapce-core/build.rs): its own config folder and Co-Review's update feed.
    RELEASE_TAG_NAME="v$version" cargo build --profile "$profile" -p lapce --bin lapce -p review_cli --bin co-review
fi

stage="$out/stage-$os-$arch"
rm -rf "$stage"
mkdir -p "$stage"
case "$os" in
mac)
    app="$stage/Co-Review.app"
    resources="$app/Contents/Resources/app"
    app_executable="$app/Contents/MacOS/Co-Review"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
    printf 'APPL????' > "$app/Contents/PkgInfo"
    cp packaging/icons/co-review.icns "$app/Contents/Resources/"
    ;;
linux)
    app="$stage/Co-Review"
    resources="$app/resources/app"
    app_executable="$app/co-review-app"
    ;;
win)
    app="$stage/Co-Review"
    resources="$app/resources/app"
    app_executable="$app/Co-Review.exe"
    ;;
esac
mkdir -p "$resources/bin" "$resources/icons"
cp "$build_dir/lapce$exe" "$app_executable"
cp "$build_dir/co-review$exe" "$resources/bin/co-review$exe"
cp bin/install-cli.sh "$resources/bin/"
printf '%s\n' "$version" > "$resources/version"
cp packaging/icons/co-review.png "$resources/icons/"
cp LICENSE NOTICE "$resources/"

# Copies the tracked files under $1 to $2, a symbolic link as a copy of what it points to (Windows checkouts have no
# links, and nothing in the app needs them as links).
copy_tracked() {
    local from="$1" to="$2" mode object stage_number path relative target
    while IFS=$'\t' read -r -d '' meta path; do
        read -r mode object stage_number <<<"$meta"
        relative="${path#"$from"}"
        relative="${relative#/}"
        mkdir -p "$(dirname "$to/$relative")"
        if [ "$mode" = 120000 ]; then
            target=$(git cat-file -p "$object")
            target=$(cd "$(dirname "$path")/$target" && pwd -P)
            copy_tracked "${target#"$(pwd -P)/"}" "$to/$relative"
        else
            cp "$path" "$to/$relative"
            if [ "$mode" = 100755 ]; then chmod +x "$to/$relative"; fi
        fi
    done < <(git ls-files -s -z -- "$from")
}
for part in plugin .claude-plugin integrations/pi integrations/omp integrations/shared; do
    copy_tracked "$part" "$resources/$part"
done

mkdir -p "$out"
case "$os" in
mac)
    # Ad hoc signatures, so Apple silicon runs it (install.sh signs it again on install); the command first, as the
    # bundle's signature seals it as a resource.
    codesign --force --sign - "$resources/bin/co-review"
    codesign --force --deep --sign - "$app"
    asset="$out/Co-Review-mac-$arch.zip"
    rm -f "$asset"
    ditto -c -k --sequesterRsrc --keepParent "$app" "$asset"
    ;;
linux)
    asset="$out/Co-Review-linux-$arch.tar.gz"
    tar -C "$stage" -czf "$asset" Co-Review
    ;;
win)
    asset="$out/Co-Review-win-$arch.exe"
    iscc=$(command -v iscc || command -v ISCC || echo "/c/Program Files (x86)/Inno Setup 6/ISCC.exe")
    "$iscc" /Q "/DVersion=$version" "/DSource=$(cygpath -w "$app")" "/DOutput=$(cygpath -w "$out")" \
        "/DIcon=$(cygpath -w "$root/packaging/icons/co-review.ico")" "$(cygpath -w "$root/packaging/windows/co-review.iss")"
    ;;
esac
echo "packaged Co-Review $version: $asset"

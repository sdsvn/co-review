#!/usr/bin/env bash
# Installs (or updates) Co-Review and the `co-review` command from the latest GitHub release.
#
#   curl -fsSL https://sdsvn.github.io/co-review/install.sh | bash
#
# macOS (Apple silicon): Co-Review.app into /Applications (or ~/Applications when that isn't writable).
# Linux (x64, arm64):    the app into ~/.local/share/co-review, with a desktop entry.
# Both:                  the `co-review` and `co-review-server` commands, into /usr/local/bin when writable,
#                        else ~/.local/bin.
#
# Environment:
#   CO_REVIEW_VERSION      a release tag to install, e.g. v0.2.0 (default: the latest release)
#   CO_REVIEW_APPS_DIR     where the macOS app goes (default: /Applications, else ~/Applications)
#   CO_REVIEW_INSTALL_DIR  where the Linux app goes (default: ~/.local/share/co-review)
#   PREFIX                 where the commands go: $PREFIX/bin
#   CO_REVIEW_DOWNLOAD_URL where the release files are (default: the GitHub release)
set -euo pipefail

repo="sdsvn/co-review"
version="${CO_REVIEW_VERSION:-latest}"
if [ -n "${CO_REVIEW_DOWNLOAD_URL:-}" ]; then
    base="$CO_REVIEW_DOWNLOAD_URL"
elif [ "$version" = latest ]; then
    base="https://github.com/$repo/releases/latest/download"
else
    base="https://github.com/$repo/releases/download/$version"
fi

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf 'co-review install: %s\n' "$*" >&2; exit 1; }

os=$(uname -s)
arch=$(uname -m)
case "$arch" in
    arm64 | aarch64) arch=arm64 ;;
    x86_64 | amd64) arch=x64 ;;
    *) fail "unsupported CPU: $arch" ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch() {
    say "Downloading $1"
    curl -fL --progress-bar -o "$tmp/$1" "$base/$1" || fail "could not download $base/$1"
}

case "$os" in
Darwin)
    [ "$arch" = arm64 ] || fail "only Apple silicon Macs have a prebuilt app; build from source: https://sdsvn.github.io/co-review/docs/start/install/"
    fetch "Co-Review-mac-arm64.zip"
    ditto -x -k "$tmp/Co-Review-mac-arm64.zip" "$tmp/app"
    apps="${CO_REVIEW_APPS_DIR:-}"
    if [ -z "$apps" ]; then
        if [ -w /Applications ]; then apps=/Applications; else apps="$HOME/Applications"; fi
    fi
    mkdir -p "$apps"
    if pgrep -f "$apps/Co-Review.app/Contents/MacOS/Co-Review" >/dev/null 2>&1; then
        say "Co-Review is running; quit it to use the new version."
    fi
    rm -rf "$apps/Co-Review.app"
    mv "$tmp/app/Co-Review.app" "$apps/"
    # The release is not notarized: sign it ad hoc so Apple silicon runs it, and clear any quarantine flag.
    codesign --force --deep --sign - "$apps/Co-Review.app" >/dev/null 2>&1 || true
    xattr -dr com.apple.quarantine "$apps/Co-Review.app" 2>/dev/null || true
    say "Installed $apps/Co-Review.app"
    "$apps/Co-Review.app/Contents/Resources/app/bin/install-cli.sh"
    ;;
Linux)
    fetch "Co-Review-linux-$arch.tar.gz"
    dir="${CO_REVIEW_INSTALL_DIR:-$HOME/.local/share/co-review}"
    mkdir -p "$tmp/app"
    tar -xzf "$tmp/Co-Review-linux-$arch.tar.gz" -C "$tmp/app"
    root=$(find "$tmp/app" -mindepth 1 -maxdepth 1 -type d | head -n 1)
    [ -x "$root/co-review" ] || fail "unexpected archive layout"
    rm -rf "$dir"
    mkdir -p "$(dirname "$dir")"
    mv "$root" "$dir"
    say "Installed $dir"
    apps_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
    mkdir -p "$apps_dir"
    cat > "$apps_dir/co-review.desktop" <<EOF
[Desktop Entry]
Name=Co-Review
Comment=Review code and designs with your coding agent
Exec="$dir/co-review" %F
Icon=$dir/resources/app/icons/icon.png
Terminal=false
Type=Application
Categories=Development;
EOF
    "$dir/resources/app/bin/install-cli.sh"
    ;;
*)
    fail "unsupported system: $os (on Windows, use the installer from https://github.com/$repo/releases/latest)"
    ;;
esac

cat <<'EOF'

Next:
  co-review ~/path/to/repo                      open a repository
  In Claude Code:
    /plugin marketplace add sdsvn/co-review
    /plugin install co-review@co-review
  Pi:           co-review setup pi
  Oh My Pi:     co-review setup omp
  Other agents: https://sdsvn.github.io/co-review/#setup
EOF

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
#   CO_REVIEW_FORCE=1      reinstall even when that version is already installed
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

# Where the app is (or goes), so an install of the version already there can be skipped.
case "$os" in
Darwin)
    apps="${CO_REVIEW_APPS_DIR:-}"
    if [ -z "$apps" ]; then
        if [ -w /Applications ]; then apps=/Applications; else apps="$HOME/Applications"; fi
    fi
    app_res="$apps/Co-Review.app/Contents/Resources/app"
    ;;
Linux)
    dir="${CO_REVIEW_INSTALL_DIR:-$HOME/.local/share/co-review}"
    app_res="$dir/resources/app"
    ;;
*)
    app_res=""
    ;;
esac

# The installed version (the app's package.json) and the one this would install (the release tag; the latest
# release's tag is where GitHub redirects /releases/latest). Unknown for a custom download URL: then it installs.
installed=""
if [ -n "$app_res" ] && [ -f "$app_res/package.json" ]; then
    installed=$(sed -n 's/^  "version": "\(.*\)",*$/\1/p' "$app_res/package.json" | head -n 1)
fi
wanted=""
if [ -z "${CO_REVIEW_DOWNLOAD_URL:-}" ]; then
    if [ "$version" = latest ]; then
        wanted=$(curl -fsSI "https://github.com/$repo/releases/latest" 2>/dev/null \
            | sed -n 's|^[Ll]ocation: .*/releases/tag/\([^[:space:]]*\).*|\1|p' | tail -n 1)
    else
        wanted="$version"
    fi
    wanted="${wanted#v}"
fi
if [ -n "$installed" ] && [ "$installed" = "$wanted" ] && [ "${CO_REVIEW_FORCE:-}" != 1 ]; then
    say "Co-Review $installed is already installed and up to date (CO_REVIEW_FORCE=1 reinstalls it)."
    # The commands may be missing (or point at an old place): installing them is quick and idempotent.
    "$app_res/bin/install-cli.sh"
    exit 0
fi
if [ -n "$installed" ]; then
    say "Updating Co-Review $installed to ${wanted:-the release at $base}"
fi

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
    mkdir -p "$apps"
    # Replacing the app's files under a running app leaves it alive without its backend (macOS kills the helper
    # processes, not the app), holding the single-instance lock: nothing can start until it is quit. Quit it.
    # The app's main process only: the agents' `co-review mcp` run on the app's executable too (as Node), and
    # stopping them would cut every agent session off Co-Review; they start the new app when next needed.
    # Asked to quit first (windows close and agents are told, as when the reviewer quits), then SIGTERM, then
    # SIGKILL: one without its backend may not answer the first two.
    running="$apps/Co-Review.app/Contents/MacOS/Co-Review"
    app_pids() {
        for pid in $(pgrep -f "$running"); do
            case "$(ps -o command= -p "$pid" 2>/dev/null)" in
            *bin/co-review.mjs*) ;;
            ?*) echo "$pid" ;;
            esac
        done
    }
    quit_wait() {
        for _ in $(seq 1 "$1"); do
            [ -z "$(app_pids)" ] && return 0
            sleep 0.2
        done
        return 1
    }
    pids=$(app_pids)
    if [ -n "$pids" ]; then
        say "Quitting the running Co-Review (process $(echo $pids)) for the update (reviews are saved; it starts again on the next co-review call)"
        # Bounded: an app without its backend may never answer the request.
        osascript -e 'with timeout of 5 seconds' -e 'quit app "Co-Review"' -e 'end timeout' >/dev/null 2>&1 || true
        if ! quit_wait 25; then
            say "Co-Review did not quit when asked; stopping it"
            kill -TERM $(app_pids) 2>/dev/null || true
            if ! quit_wait 25; then
                say "Co-Review did not stop; killing it"
                kill -KILL $(app_pids) 2>/dev/null || true
                quit_wait 10 || fail "could not stop the running Co-Review; quit it and run this again"
            fi
        fi
        say "Co-Review quit"
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
    mkdir -p "$tmp/app"
    tar -xzf "$tmp/Co-Review-linux-$arch.tar.gz" -C "$tmp/app"
    root=$(find "$tmp/app" -mindepth 1 -maxdepth 1 -type d | head -n 1)
    [ -x "$root/co-review" ] || fail "unexpected archive layout"
    if pgrep -f "$dir/co-review" >/dev/null 2>&1; then
        say "Quitting the running Co-Review for the update (reviews are saved; it starts again on the next co-review call)"
        pkill -TERM -f "$dir/co-review" || true
        for _ in $(seq 1 50); do
            pgrep -f "$dir/co-review" >/dev/null 2>&1 || break
            sleep 0.2
        done
        pkill -KILL -f "$dir/co-review" 2>/dev/null || true
    fi
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

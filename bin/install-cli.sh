#!/bin/sh
# Installs the `co-review` command: a link to the native command that ships with the app (or was built in a checkout).
#
#   From the macOS app:  "/Applications/Co-Review.app/Contents/Resources/app/bin/install-cli.sh"
#   From the Linux app:  ~/.local/share/co-review/resources/app/bin/install-cli.sh
#   From a checkout:     make install-cli (after make build)
#
# Target: $PREFIX/bin (default /usr/local/bin when writable, else ~/.local/bin). The Windows installer puts the
# command's folder on PATH instead.
set -e
here=$(cd "$(dirname "$0")" && pwd -P)

if [ -x "$here/co-review" ]; then
    command="$here/co-review"
else
    # A checkout: the command `make build` (or `cargo build`) built.
    root=$(dirname "$here")
    for candidate in "$root/target/debug/co-review" "$root/target/release/co-review"; do
        if [ -x "$candidate" ]; then
            command="$candidate"
            break
        fi
    done
    [ -n "${command:-}" ] || { echo "co-review: build it first (make build)" >&2; exit 1; }
fi

if [ -n "$PREFIX" ]; then
    dir="$PREFIX/bin"
elif [ -w /usr/local/bin ]; then
    dir=/usr/local/bin
else
    dir="$HOME/.local/bin"
fi
mkdir -p "$dir"
# Replaces the Node shim earlier versions wrote (a file, not a link).
rm -f "$dir/co-review"
ln -s "$command" "$dir/co-review"
echo "installed $dir/co-review -> $command"
# The review server command is gone with the browser app; remove the shim an earlier version wrote.
if [ -f "$dir/co-review-server" ] && grep -q 'review-server.mjs' "$dir/co-review-server" 2>/dev/null; then
    rm -f "$dir/co-review-server"
    echo "removed $dir/co-review-server (the review server is no longer part of Co-Review)"
fi
case ":$PATH:" in
*":$dir:"*) ;;
*) echo "note: $dir is not on your PATH; add it, e.g. echo 'export PATH=\"$dir:\$PATH\"' >> ~/.zshrc" ;;
esac

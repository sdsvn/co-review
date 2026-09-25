#!/bin/sh
# Installs the `co-review` command, and `co-review-server` (review server mode, see bin/review-server.mjs).
#
#   From the desktop app:  "/Applications/Co-Review.app/Contents/Resources/app/bin/install-cli.sh"
#   From a checkout:       make install-cli
#
# The command runs on the desktop app's own runtime when installed from the app (no Node.js needed),
# otherwise on `node` with the checkout's browser app. Target: $PREFIX/bin (default /usr/local/bin
# when writable, else ~/.local/bin).
set -e
here=$(cd "$(dirname "$0")" && pwd -P)

case "$here" in
*.app/Contents/Resources/app/bin)
    app=${here%/Contents/Resources/app/bin}
    name=$(basename "$app" .app)
    runner="ELECTRON_RUN_AS_NODE=1 exec \"$app/Contents/MacOS/$name\""
    ;;
*)
    command -v node >/dev/null || { echo "co-review: node is required for a checkout install" >&2; exit 1; }
    runner="exec \"$(command -v node)\""
    ;;
esac

if [ -n "$PREFIX" ]; then
    dir="$PREFIX/bin"
elif [ -w /usr/local/bin ]; then
    dir=/usr/local/bin
else
    dir="$HOME/.local/bin"
fi
mkdir -p "$dir"
for cmd in co-review:co-review.mjs co-review-server:review-server.mjs; do
    cat > "$dir/${cmd%%:*}" <<EOF
#!/bin/sh
$runner "$here/${cmd#*:}" "\$@"
EOF
    chmod +x "$dir/${cmd%%:*}"
    echo "installed $dir/${cmd%%:*}"
done
case ":$PATH:" in
*":$dir:"*) ;;
*) echo "note: $dir is not on your PATH; add it, e.g. echo 'export PATH=\"$dir:\$PATH\"' >> ~/.zshrc" ;;
esac

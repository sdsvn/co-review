# Running and packaging

This page is for running Co-Review from a clone of the repository, and for building the desktop app and its
installers. To just use Co-Review, the [install script](../README.md#install) is quicker.

## Requirements

- Node.js 20 or later (tested with 24), npm and Git.
- A C/C++ toolchain for native modules: the Xcode command-line tools on macOS, `build-essential` on Linux.
- Network access to npm, Open VSX (plugins) and GitHub (Electron) for the first install.

Then install the dependencies:

```bash
# npm install, plus the VS Code built-in plugins (Git, languages) into plugins/
make install

# list every target
make help
```

## Browser app

Build Co-Review and open a repository in your browser:

```bash
# builds, starts on http://127.0.0.1:3000 in the background, opens the browser
make start REPO=/path/to/repo

# the same, on another port
make start REPO=/path/to/repo PORT=4000
```

`make start` runs `bin/co-review.mjs`. It starts the server in the background, or reuses one that is already
running, and opens `http://127.0.0.1:<port>/#<repo>`. The server log is `~/.co-review/server.log`.

One server serves any number of repositories. To open another one:

```bash
node bin/co-review.mjs /other/repo
```

To stop the server:

```bash
kill $(jq .pid ~/.co-review/server.json)
```

(or `pkill -f applications/browser/lib/backend/main.js`).

**Development.** `make dev` recompiles the extension and rebundles the frontend whenever a file changes. Reload
the browser to see frontend changes; restart the server for backend changes.

## Desktop app

```bash
# build and run the Electron app
make desktop REPO=/path/to/repo

# installers for this OS, in applications/electron/dist
make package

# the unpacked app only (faster)
make package-dir
```

What `make package` produces (configured in `applications/electron/electron-builder.yml`):

| OS | Output |
|---|---|
| macOS | `dmg` and `zip` |
| Linux | `AppImage` and `deb` |
| Windows | `nsis` installer |

**Signing.** Builds are unsigned, so on macOS open an unsigned build with right-click → Open the first time. To
sign on macOS, set `CSC_LINK` and `CSC_KEY_PASSWORD` and remove `identity: null`. To notarize, also set `APPLE_ID`,
`APPLE_APP_SPECIFIC_PASSWORD` and `APPLE_TEAM_ID`.

**What's in the package:**

- The built frontend and backend (`lib/`, `src-gen/`) and the production `node_modules`.
- The VS Code built-in plugins (`plugins/`, loaded by `scripts/electron-main.js`). Built-ins that aren't needed
  (debugger, notebooks, Emmet, extra themes, …) are listed under `theiaPluginsExcludeIds` in the root
  `package.json`.
- The agent integrations the app installs for you: the Claude Code plugin (`plugin/`, `.claude-plugin/`), the Pi
  and Oh My Pi packages (`integrations/pi`, `integrations/omp`, sharing `integrations/shared`) and the `co-review` command (`bin/`).

### Releases

`.github/workflows/release.yml` builds the app for macOS (arm64), Linux (x64, arm64) and Windows (x64). For a `v*`
tag, it also publishes a GitHub release.

To release, tag and push. The workflow takes the version from the tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

To get the builds as artifacts without releasing, run the workflow by hand: **Actions → Release → Run workflow**.

Asset names carry no version (`Co-Review-mac-arm64.zip`, `Co-Review-linux-x64.tar.gz`, `Co-Review-win-x64.exe`,
…). That way `install.sh` can always download `releases/latest/download/<name>`:

```bash
curl -fsSL https://sdsvn.github.io/co-review/install.sh | bash
```

### Installing on macOS

```bash
# package, copy Co-Review.app to /Applications (or ~/Applications),
# and install the co-review command
make install-app
```

**The `co-review` command** is a small shim that `bin/install-cli.sh` writes to `/usr/local/bin` when it is
writable, otherwise to `~/.local/bin`. Set `PREFIX=…` to choose another place.

- `co-review [dir]` opens `dir` in the desktop app. If the app is already running, it opens a new window.
- `co-review mcp [dir]` connects an agent (stdio MCP) to the app's backend. It starts the app if needed and finds
  it through `~/.co-review/server.json`.

Installed from the app, the shim runs `bin/co-review.mjs` inside the app bundle, on the app's own Electron
(`ELECTRON_RUN_AS_NODE=1`). Installed from a checkout (`make install-cli`), it runs on `node` and starts the
browser app instead.

**Security.** The desktop backend only accepts its own windows (Theia's per-launch token). Two endpoints are
exempt, `/mcp` and the phone view (`src/electron-node`), and they only accept local connections. `/mcp` also
refuses browser requests from other origins.

### Environment and language servers

**Environment.** An app opened from Finder or the Dock gets a minimal environment on macOS, without your shell's
`PATH`. So at startup, `scripts/electron-main.js` reads your login shell's environment (`$SHELL -ilc env`). That
way `gopls`, other language servers and ACP agents are found just as in a terminal.

- Launched from a terminal (`co-review`, `make desktop`), the app keeps that terminal's environment.
- Set `CO_REVIEW_NO_SHELL_ENV=1` to skip reading the shell environment.

**Language servers.** Go to definition, find references and the like come from VS Code extensions. They are listed
in `theiaPlugins` in the root `package.json` and downloaded into `plugins/` by `make plugins`.

- Included: the built-ins (TypeScript/JavaScript, JSON, Markdown, PHP) and `golang.Go`, which needs `gopls`.
- For other languages, add an [Open VSX](https://open-vsx.org) extension to that list.
- Language servers only start in trusted workspaces.

### Native modules

`node-pty`, `drivelist`, `keytar` and `native-keymap` are compiled against Node for the browser app, and against
Electron for the desktop app. `make browser` and `make desktop-build` switch between the two (`theia
rebuild:browser|electron`, with the originals cached in `.browser_modules/`). So build the target you run last.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `CO_REVIEW_HOME` | `~/.co-review` | Review storage, server registry (`server.json`), server log |
| `CO_REVIEW_PORT` | `3000` | Port for `co-review` / `co-review mcp` when starting the browser app |
| `CO_REVIEW_ALLOWED_HOSTS` | — | Comma-separated extra host names allowed to open the phone view (`/m/`) |

Agents launched over ACP inherit the environment of the Co-Review process. So start Co-Review from a shell where
the agent is logged in and configured.

## Phone view

`/m/` on the running app (browser app or desktop backend) is a small page for reviewing away from your desk. You
can install it on your phone's home screen. It has:

- the review list, and the threads with their replies;
- Resolve, and Accept / Dismiss for findings and suggestions;
- Submit review.

It updates live (long polling). Only local connections are allowed, so to reach it from a phone:

1. Expose the port on your [Tailscale](https://tailscale.com) network:

   ```bash
   tailscale serve --bg 3000
   ```

2. Start Co-Review with that host name allowed:

   ```bash
   CO_REVIEW_ALLOWED_HOSTS=my-mac.tailnet-name.ts.net make start
   ```

3. On the phone, open `https://my-mac.tailnet-name.ts.net/m/` and choose *Add to Home Screen*.

# Running and packaging

Requirements: Node.js ≥ 20 (tested with 24), npm, Git, a C/C++ toolchain for native modules
(Xcode command-line tools on macOS, `build-essential` on Linux), and network access to npm,
Open VSX (plugins) and GitHub (Electron) for the first install.

```bash
make install        # npm install + VS Code built-in plugins (Git, languages) into plugins/
make help           # all targets
```

## Browser app

```bash
make start REPO=/path/to/repo            # builds, starts on http://127.0.0.1:3000 in the background, opens the browser
make start REPO=/path/to/repo PORT=4000
```

`make start` runs `bin/co-review.mjs`, which starts the server detached (log:
`~/.co-review/server.log`) or reuses a running one, and opens `http://127.0.0.1:<port>/#<repo>`.
One server serves any repository: open another one with `node bin/co-review.mjs /other/repo`.
Stop it with `kill $(jq .pid ~/.co-review/server.json)` (or `pkill -f applications/browser/lib/backend/main.js`).

Development: `make dev` recompiles the extension and rebundles the frontend on change; reload
the browser. Backend changes need a restart of the server.

## Desktop app

```bash
make desktop REPO=/path/to/repo          # build and run the Electron app
make package                             # installers for this OS in applications/electron/dist
make package-dir                         # unpacked app only (faster)
```

`make package` produces `dmg` + `zip` on macOS, `AppImage` + `deb` on Linux, and an `nsis`
installer on Windows (configured in `applications/electron/electron-builder.yml`). Builds are
unsigned; to sign on macOS set `CSC_LINK`/`CSC_KEY_PASSWORD` (and remove `identity: null`), and
set `APPLE_ID`/`APPLE_APP_SPECIFIC_PASSWORD`/`APPLE_TEAM_ID` to notarize. Unsigned macOS
builds must be opened with right-click → Open the first time.

The package also bundles the agent integrations that the app installs for you: the Claude Code plugin (`plugin/`,
`.claude-plugin/`), the Pi package (`integrations/pi`) and the `co-review` CLI (`bin/`).

The package contains the built frontend and backend (`lib/`, `src-gen/`), the production
`node_modules` and the VS Code built-in plugins (`plugins/`, picked up by
`scripts/electron-main.js`). Excluded built-ins (debugger, notebooks, Emmet, extra themes, …)
are listed under `theiaPluginsExcludeIds` in the root `package.json`.

### Installing on macOS

```bash
make install-app                         # package, copy Co-Review.app to /Applications (or ~/Applications), install `co-review`
```

`co-review` is a small shim written by `bin/install-cli.sh` into `/usr/local/bin` when writable, else `~/.local/bin`
(override with `PREFIX=…`). Installed from the app, it runs `bin/co-review.mjs` inside the bundle on the app's
Electron (`ELECTRON_RUN_AS_NODE=1`): `co-review [dir]` opens `dir` in the desktop app (a running app opens a new
window), and `co-review mcp [dir]` bridges stdio MCP to the app's backend, launching the app if needed and finding
it through `~/.co-review/server.json`. From a checkout (`make install-cli`) the shim runs on `node` and starts the
browser app instead.

The desktop backend only admits its own windows (Theia's per-launch token); `/mcp` and the phone view are exempt
(`src/electron-node`) and keep their own local-host guards; `/mcp` also refuses browser requests from other origins.

### Environment and language servers

Opened from Finder or the Dock, a macOS app gets launchd's minimal environment. `scripts/electron-main.js` therefore
reads the login shell's environment (`$SHELL -ilc env`) at startup, so `gopls`, other language servers and ACP agents
are found the way they are in a terminal. Launched from a terminal (`co-review`, `make desktop`), the app keeps that
terminal's environment. Set `CO_REVIEW_NO_SHELL_ENV=1` to skip it.

Code navigation comes from VS Code extensions listed in `theiaPlugins` (root `package.json`) and downloaded into
`plugins/` by `make plugins`: the built-ins (TypeScript/JavaScript, JSON, Markdown, PHP) and `golang.Go` (needs
`gopls`). Add an [Open VSX](https://open-vsx.org) extension there for other languages. Language servers only start
in trusted workspaces.

### Native modules

`node-pty`, `drivelist`, `keytar` and `native-keymap` are compiled against Node for the browser
app and against Electron for the desktop app. `make browser` / `make desktop-build` switch them
(`theia rebuild:browser|electron`, originals cached in `.browser_modules/`), so build the target
you run last.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `CO_REVIEW_HOME` | `~/.co-review` | Review storage, server registry (`server.json`), server log |
| `CO_REVIEW_PORT` | `3000` | Port for `co-review` / `co-review mcp` when starting the browser app |
| `CO_REVIEW_ALLOWED_HOSTS` | — | Comma-separated extra host names allowed to open the phone view (`/m/`) |

Agents launched over ACP inherit the environment of the Co-Review process — start Co-Review from
a shell where the agent is logged in / configured.

## Phone view

`/m/` on the running app (browser app or desktop backend) is a small installable page for reviewing away from the desk:
the review list, threads with replies, Resolve, Accept / Dismiss for findings and suggestions, and Submit review. It
updates live (long polling). Only local connections are allowed; to reach it from a phone, expose the port on your
tailnet and allow that host name:

```bash
tailscale serve --bg 3000
```

```bash
CO_REVIEW_ALLOWED_HOSTS=my-mac.tailnet-name.ts.net make start
```

Then open `https://my-mac.tailnet-name.ts.net/m/` on the phone and *Add to Home Screen*.

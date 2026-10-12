# Building and running

This page is for building Co-Review from a clone of the repository, packaging it, and configuring it. To just use
Co-Review, the [install script](../README.md#install) is quicker.

## Requirements

- Rust: `rustup` installs the version in `rust-toolchain.toml` on the first build.
- Git.
- Node.js 20 or later, only for `make prompts`, the version check and the website.
- macOS: the Command Line Tools (`xcode-select --install`). No Xcode, no cmake.
- Linux: the libraries Lapce needs. On Ubuntu, `sudo make -f Makefile.lapce ubuntu-deps`; for other distributions,
  see [Lapce's build guide](lapce/building-from-source.md).
- Windows: MSVC (the Visual Studio Build Tools), and Inno Setup 6 to build the installer.

## Build and run

```bash
# the app and the `co-review` command (debug build, in target/debug)
make build

# run the app on a repository (default: this checkout)
make run REPO=/path/to/repo

# the `co-review` command built here, linked into PATH
make install-cli

# list every target
make help
```

`make run` opens the repository through the `co-review` command, as an agent or the installed app would. A debug
build keeps its own config folder, so it never shares settings with an installed Co-Review.

**Tests.** `make test` checks that every version matches and that the prompt copies are current, then runs the review
crates' tests (`cargo test`). `make lint` runs Clippy. CI (`.github/workflows/rust.yml`) does the same on macOS,
Linux and Windows.

## Package

```bash
# this OS's release asset, in target/package
make package

# package it and install it, as install.sh does
make install-app
```

`make package` runs [`packaging/package.sh`](../packaging/package.sh):

| OS | Asset | What's in it |
|---|---|---|
| macOS (Apple silicon) | `Co-Review-mac-arm64.zip` | `Co-Review.app` |
| Linux (x64, arm64) | `Co-Review-linux-x64.tar.gz`, `Co-Review-linux-arm64.tar.gz` | a `Co-Review/` folder |
| Windows (x64) | `Co-Review-win-x64.exe` | an installer (Inno Setup) for your user only, no admin rights |

Inside, the app's own files sit in `resources/app` (`Co-Review.app/Contents/Resources/app` on macOS):

- `version`: the app's version.
- `bin/co-review`: the `co-review` command. `bin/install-cli.sh` links it into PATH.
- `plugin/` and `.claude-plugin/`: the Claude Code plugin and its marketplace.
- `integrations/`: the Pi and Oh My Pi packages (`co-review setup pi|omp`).

The app itself is `Co-Review.app/Contents/MacOS/Co-Review` on macOS, `Co-Review/co-review-app` on Linux, and
`Co-Review.exe` on Windows.

**Signing.** Builds are not notarized. They are signed ad hoc, and `install.sh` signs the macOS app again when it
installs it, so it opens without a warning. If you unzip a release yourself, right-click the app and choose **Open**
the first time.

### Releases

`.github/workflows/release.yml` builds the four assets. For a `v*` tag, it also publishes them as a GitHub release.
`install.sh` downloads from that release, and the app checks it for updates.

To release, bump the version where it is recorded:

- the app: `[workspace.package]` in `Cargo.toml`;
- the Claude Code plugin manifests;
- the Pi and Oh My Pi packages.

Then tag and push. `make check-versions` (part of `make test`) fails when any of them differs from `Cargo.toml`. The
workflow also fails when the tag differs.

```bash
git tag v0.7.0 && git push origin v0.7.0
```

To get the builds as artifacts without releasing, run the workflow by hand: **Actions → Release → Run workflow**.

Asset names carry no version, so `install.sh` can always download `releases/latest/download/<name>`:

```bash
curl -fsSL https://sdsvn.github.io/co-review/install.sh | bash
```

After installing (or finding that version already installed), `install.sh` brings the agent integrations that are
already installed to the app's version:

- the Claude Code plugin: `claude plugin marketplace update co-review`, then `claude plugin update co-review@co-review`;
- the Oh My Pi and Pi packages, when they come from this app: `omp install` / `pi install` of its `integrations/`
  path, which refreshes the version they recorded.

It skips a harness or plugin that isn't installed, and never fails the install over them.

An update never leaves an old Co-Review running, so an old version never serves the new command and agents. It is
stopped in three ways:

- `install.sh` quits the running app before replacing it.
- The running app quits itself once the version on disk is not its own.
- The `co-review` command stops a running app of another version, or one without a working backend, before starting
  one.

Reviews are saved; the next `co-review` call starts the new version.

## The `co-review` command

`install.sh` links the command into `/usr/local/bin` when it is writable, otherwise into `~/.local/bin` (set
`PREFIX=…` to choose). The Windows installer puts its folder on your `PATH`. On macOS, the app's command palette also
has **Install the co-review Command in PATH**.

- `co-review [dir]` opens `dir` in Co-Review, or focuses its window if it is already open.
- `co-review mcp [dir]` connects an agent (stdio MCP) to Co-Review, found through `~/.co-review/server.json`.
  - It starts Co-Review in the background on the agent's first tool call, not when the agent session starts. No window
    opens until the agent shows a review (`open_review`), or you open the app.
  - A window the agent showed closes when the agent is done with the review (you approve it, or its session ends). It
    says why and closes after 10 seconds unless you keep it open. With no window left, the app quits.
  - When you close a window yourself, the agent is told.
  - If Co-Review goes away mid-session, the agent's call fails instead of waiting, and its next call starts Co-Review
    again.
- `co-review status [dir]` prints this Co-Review's version and each installed agent integration's (the Claude Code
  plugin, the Oh My Pi and Pi packages). It flags any that differ with the command that updates it, then lists the open
  reviews of `dir`. It never starts Co-Review.
- `co-review setup <pi|omp>` installs the Pi or Oh My Pi package that ships with the app.
- `co-review logs` is for when something hangs or fails. It says whether Co-Review answers (and how fast), then prints
  the recent debug log of every Co-Review process, merged by time (`--lines <n>`, default 80). The logs are in
  `~/.co-review/logs`: `main.log` (the app and its windows), `backend.log` (the review core) and `cli.log` (the
  `co-review` command and each agent's `co-review mcp` bridge).
- `co-review version` (`-v`) prints the version; `co-review help` (`-h`) lists the commands and options.

**Security.** The MCP endpoint (`/mcp`) only accepts local connections, and refuses browser requests from other
origins.

## Environment and language servers

**Environment.** An app opened from Finder, the Dock or a desktop menu gets a minimal environment, without your shell's
`PATH`. So at startup Co-Review reads your login shell's environment. That way language servers and ACP agents are
found just as in a terminal. Launched from a terminal (`co-review`, `make run`), the app keeps that terminal's
environment.

Agents launched over ACP inherit the environment of the Co-Review process. So start Co-Review from a shell where the
agent is logged in and configured.

**Language servers.** Go to definition, find references, hover and the like come from Lapce plugins: open **Plugins**
in the side bar and install the one for your language (for example Rust, Go or TypeScript). A plugin may need its
language server on your `PATH` (`gopls` for Go, for instance). Tree-sitter syntax highlighting needs no plugin.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `CO_REVIEW_HOME` | `~/.co-review` | Review storage, server registry (`server.json`), logs, worktrees |
| `CO_REVIEW_PORT` | a running Co-Review's, else `3000` | Port of the MCP server for `co-review` / `co-review mcp` when they start Co-Review |

Themes, fonts and keybindings are Lapce's settings: **Open Settings** and **Change Color Theme** in the command
palette.

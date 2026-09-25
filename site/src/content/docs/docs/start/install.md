---
title: Install
description: Install the Co-Review desktop app and the co-review command.
sidebar:
  order: 2
---

## macOS and Linux

```bash
curl -fsSL https://sdsvn.github.io/co-review/install.sh | bash
```

This installs the latest release: the desktop app (macOS on Apple silicon: `/Applications`, or `~/Applications`;
Linux x64 and arm64: `~/.local/share/co-review`, with a desktop entry) and the `co-review` and `co-review-server`
commands (`/usr/local/bin` when writable, else `~/.local/bin`). Run it again to update. The script is
[install.sh](https://github.com/sdsvn/co-review/blob/main/install.sh); read it before piping it to a shell if you
prefer.

| Variable | Does |
|---|---|
| `CO_REVIEW_VERSION=v0.2.0` | Installs that release instead of the latest |
| `CO_REVIEW_APPS_DIR` | Where the macOS app goes |
| `CO_REVIEW_INSTALL_DIR` | Where the Linux app goes |
| `PREFIX` | Where the commands go (`$PREFIX/bin`) |

```bash
co-review ~/src/my-service     # open a repository
co-review mcp                  # MCP server for agent harnesses; starts the app if needed
```

The commands run on the app's own runtime, so Node.js isn't needed.

:::note
Releases aren't notarized. The script signs the macOS app ad hoc so it opens without a warning; if you download
the `.dmg` yourself instead, open the app with right-click → **Open** the first time.
:::

## Windows

Download `Co-Review-win-x64.exe` from the [latest release](https://github.com/sdsvn/co-review/releases/latest). Then
set up your agent with **Review: Add Co-Review to an Agent Harness (MCP)…** inside the app.

## From source (any OS)

Requires Node.js 20+, Git and a C/C++ toolchain (Xcode command-line tools on macOS, `build-essential` on Linux).

```bash
make install
make start REPO=/path/to/repo      # browser app on http://127.0.0.1:3000
make desktop REPO=/path/to/repo    # desktop app
make install-cli                   # `co-review` command running from this checkout
```

`make package` builds installers for the current OS: `dmg`/`zip` on macOS, `AppImage`/`deb` on Linux, `nsis` on
Windows. See [Running and packaging](/co-review/docs/guides/running-and-packaging/).

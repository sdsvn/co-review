---
title: Install
description: Install the Co-Review desktop app and the co-review command.
sidebar:
  order: 2
---

## macOS: app and command line

```bash
git clone https://github.com/sdsvn/co-review && cd co-review
make install        # dependencies and bundled VS Code extensions (first time)
make install-app    # Co-Review.app into /Applications, plus the `co-review` command
```

Or build a disk image with `make package` and drag **Co-Review** to Applications. Then install the command from
inside the app (**Review: Install the co-review Command**), or with:

```bash
/Applications/Co-Review.app/Contents/Resources/app/bin/install-cli.sh
```

:::note
Local builds are unsigned: the first time, open the app with right-click → **Open**.
:::

```bash
co-review ~/src/my-service     # open a repository
co-review mcp                  # MCP server for agent harnesses; starts the app if needed
```

The command runs on the app's own runtime, so Node.js isn't needed.

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

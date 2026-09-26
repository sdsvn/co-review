---
title: Install
description: Install the Co-Review desktop app and the co-review command.
sidebar:
  order: 2
---

## macOS and Linux

Run the one-line installer to get the latest release.

```bash
curl -fsSL https://sdsvn.github.io/co-review/install.sh | bash
```

This installs two things:

- **The desktop app.**
  On macOS (Apple silicon) it goes to `/Applications` or `~/Applications`.
  On Linux (x64 and arm64) it goes to `~/.local/share/co-review`, with a desktop entry.
- **The `co-review` and `co-review-server` commands.**
  They go to `/usr/local/bin` when writable, otherwise `~/.local/bin`.

Run the same command again to update.
The script is [install.sh](https://github.com/sdsvn/co-review/blob/main/install.sh) -- read it before piping it to a shell if you prefer.

You can override the defaults with environment variables:

| Variable | Effect |
|---|---|
| `CO_REVIEW_VERSION=v0.2.0` | Install that release instead of the latest |
| `CO_REVIEW_APPS_DIR` | Where the macOS app goes |
| `CO_REVIEW_INSTALL_DIR` | Where the Linux app goes |
| `PREFIX` | Where the commands go (`$PREFIX/bin`) |

Once installed, try it:

```bash
# Open a repository
co-review ~/src/my-service

# Start the MCP server (launches the app if needed)
co-review mcp
```

The commands run on the app's own runtime, so Node.js is not needed.

:::note
Releases are not notarized.
The script signs the macOS app ad hoc so it opens without a warning.
If you download the `.dmg` yourself instead, open the app with right-click then **Open** the first time.
:::

## Windows

1. Download `Co-Review-win-x64.exe` from the [latest release](https://github.com/sdsvn/co-review/releases/latest).
2. Set up your agent with **Review: Add Co-Review to an Agent Harness (MCP)...** inside the app.

## From source (any OS)

Building from source requires Node.js 20+, Git, and a C/C++ toolchain (Xcode command-line tools on macOS, `build-essential` on Linux).

```bash
make install

# Browser app on http://127.0.0.1:3000
make start REPO=/path/to/repo

# Desktop app
make desktop REPO=/path/to/repo

# Install the co-review command from this checkout
make install-cli
```

`make package` builds installers for the current OS:

- `dmg` / `zip` on macOS
- `AppImage` / `deb` on Linux
- `nsis` on Windows

See [Running and packaging](/co-review/docs/guides/running-and-packaging/).

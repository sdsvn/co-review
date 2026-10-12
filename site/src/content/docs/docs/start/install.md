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
- **The `co-review` command**, a native binary that ships with the app.
  It is linked into `/usr/local/bin` when writable, otherwise `~/.local/bin`.

Run the same command again to update. When the latest release is already installed, it downloads nothing and only
re-links the command.
The script is [install.sh](https://github.com/sdsvn/co-review/blob/main/install.sh) -- read it before piping it to a shell if you prefer.

You can override the defaults with environment variables:

| Variable | Effect |
|---|---|
| `CO_REVIEW_VERSION=v0.2.0` | Install that release instead of the latest |
| `CO_REVIEW_APPS_DIR` | Where the macOS app goes |
| `CO_REVIEW_INSTALL_DIR` | Where the Linux app goes |
| `PREFIX` | Where the command goes (`$PREFIX/bin`) |
| `CO_REVIEW_FORCE=1` | Reinstall even when that version is already installed |

Once installed, try it:

```bash
# Open a repository
co-review ~/src/my-service

# Start the MCP server (launches the app if needed)
co-review mcp
```

The command is a native binary, so Node.js is not needed.

:::note
Releases are not notarized.
The script signs the macOS app ad hoc so it opens without a warning.
If you download `Co-Review-mac-arm64.zip` yourself instead, open the app with right-click then **Open** the first time.
:::

## Windows

1. Download `Co-Review-win-x64.exe` from the [latest release](https://github.com/sdsvn/co-review/releases/latest) and run it.
   It installs Co-Review for your user only (no admin rights) and puts the `co-review` command on your `PATH`.
2. Set up your agent: see [Connect your agent](/co-review/docs/guides/connect-your-agent/).

## From source (any OS)

Building from source requires Rust (`rustup` installs the version the repository pins), Git, and the platform's
build tools: the Command Line Tools on macOS, the libraries in `Makefile.lapce` on Linux
(`sudo make -f Makefile.lapce ubuntu-deps`), MSVC on Windows.

```bash
# The app and the co-review command (debug build)
make build

# Run the app on a repository
make run REPO=/path/to/repo

# Install the co-review command from this checkout
make install-cli
```

`make package` builds this OS's release asset in `target/package`:

- `Co-Review-mac-arm64.zip` on macOS
- `Co-Review-linux-x64.tar.gz` or `Co-Review-linux-arm64.tar.gz` on Linux
- `Co-Review-win-x64.exe` (Inno Setup) on Windows

See [Building and running](/co-review/docs/guides/running-and-packaging/).

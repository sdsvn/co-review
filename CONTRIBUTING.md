# Contributing to Co-Review

Co-Review is a fork of [Lapce](https://lapce.dev). Lapce's own contributing guide is in
[docs/lapce/CONTRIBUTING.md](docs/lapce/CONTRIBUTING.md); editor bugs that aren't about reviewing are best fixed
upstream.

## Questions, bugs and ideas

Open an [issue](https://github.com/sdsvn/co-review/issues). For a bug, say what you did, what you expected and what
happened, and add the relevant lines of `co-review logs`.

## Building

You need Rust (`rustup` installs the version in `rust-toolchain.toml`), Git, and Node.js for `make prompts` and the
website. On macOS, the Command Line Tools; on Linux, `sudo make -f Makefile.lapce ubuntu-deps`; on Windows, MSVC.

```bash
make build                    # the app and the `co-review` command (debug)
make run REPO=/path/to/repo   # run the app on a repository
make test                     # version and prompt checks, and the review crates' tests
make lint                     # clippy
```

[Building and running](docs/running-and-packaging.md) has packaging, installing and configuration.

## Pull requests

- Read [INSTRUCTIONS.md](INSTRUCTIONS.md) first: where new code goes, how edits to Lapce's files are marked
  (`// co-review:`) and listed in [docs/co-review/fork-surface.md](docs/co-review/fork-surface.md), and the tests.
- New behaviour starts as a change in [`openspec/`](openspec/).
- Agent instructions are written once in [`prompts/`](prompts/README.md). Edit them there and run `make prompts`;
  never edit a generated copy.
- Run `cargo fmt --all`, `make lint` and `make test` before you open the pull request.
- Update the docs (`README.md`, `docs/`, `llms.txt`) when what users see changes.

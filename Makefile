# Co-Review — build, run and package (Lapce's own Makefile is Makefile.lapce).
#
#   make build          the app and the `co-review` command (debug build, target/debug)
#   make run            run the app on $(REPO), through the `co-review` command
#   make test           version check, prompt check, and the review crates' tests
#   make package        the release asset for this OS in target/package (Co-Review-mac-arm64.zip, …)
#   make install-app    package and install it with install.sh (the app + the `co-review` command)
#   make install-cli    the `co-review` command built in this checkout, into PATH
#   make prompts        copy the shared agent prompts (prompts/) to the skills, commands, agents, app and docs

REPO    ?= $(CURDIR)
# cargo profile for `make package` (the release workflow uses release-lto)
PROFILE ?= release
PACKAGE := $(CURDIR)/target/package

# Lapce's crates; the rest of the workspace is Co-Review's.
LAPCE_CRATES := --exclude lapce --exclude lapce-app --exclude lapce-core --exclude lapce-proxy --exclude lapce-rpc

.PHONY: help build run start desktop test lint prompts check-prompts check-versions package install-app install-cli mcp-claude clean

help: ## Show targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  \033[1m%-14s\033[0m %s\n", $$1, $$2}'

build: ## Build the app and the `co-review` command (debug)
	cargo build -p lapce --bin lapce -p review_cli --bin co-review

run: build ## Run the app on a repository (REPO=/path/to/repo; default: this checkout)
	target/debug/co-review $(REPO)

start desktop: run

prompts: ## Copy the shared agent prompts in prompts/ everywhere agents read them
	node bin/gen-prompts.mjs

check-prompts: ## Fail if a copy of the shared agent prompts is out of date
	node bin/gen-prompts.mjs --check

check-versions: ## Fail if the plugin manifests or the Pi/omp packages differ from the app's version (Cargo.toml)
	node bin/check-versions.mjs

test: check-versions check-prompts ## Run the checks and the review crates' tests
	cargo test --workspace $(LAPCE_CRATES)

lint: ## Clippy: the review crates with warnings as errors, Lapce's as upstream does
	cargo clippy --workspace $(LAPCE_CRATES) --all-targets -- -D warnings
	cargo clippy -p lapce -p lapce-app -p lapce-core -p lapce-proxy -p lapce-rpc

package: check-versions ## Package the app for this OS into target/package (PROFILE=release)
	PROFILE=$(PROFILE) OUT=$(PACKAGE) packaging/package.sh

install-app: package ## Install the packaged app (macOS: /Applications or ~/Applications; Linux: ~/.local/share/co-review) and the `co-review` command
	@# install.sh quits a running Co-Review before replacing it, links the command and refreshes the integrations.
	CO_REVIEW_DOWNLOAD_URL=file://$(PACKAGE) CO_REVIEW_FORCE=1 ./install.sh

install-cli: build ## Install the `co-review` command built in this checkout (target/debug) into PATH
	bin/install-cli.sh

mcp-claude: build ## Register this checkout's `co-review` as an MCP server in Claude Code (user scope)
	claude mcp add -s user co-review -- $(CURDIR)/target/debug/co-review mcp

clean: ## Remove the build output
	cargo clean

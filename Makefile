# Co-Review — build, run and package.
#
#   make install        dependencies + VS Code built-in plugins (first time)
#   make start          browser app on http://localhost:$(PORT), reviewing $(REPO)
#   make desktop        desktop (Electron) app, reviewing $(REPO)
#   make package        installable desktop app (dmg/zip, AppImage/deb or nsis) in applications/electron/dist
#   make install-app    macOS: build and install Co-Review.app + the `co-review` command
#   make install-cli    the `co-review` command, running from this checkout
#
# Native modules (node-pty, drivelist, …) are compiled either for Node (browser app) or for
# Electron (desktop app); each target switches them as needed, so the two can be built alternately.

PORT  ?= 3000
REPO  ?= $(CURDIR)
MODE  ?= production

THEIA := $(CURDIR)/node_modules/.bin/theia
BROWSER := applications/browser
ELECTRON := applications/electron

.PHONY: help install plugins extension browser start dev desktop-build desktop package package-dir install-app install-cli mcp-claude clean distclean

help: ## Show targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  \033[1m%-14s\033[0m %s\n", $$1, $$2}'

install: ## Install npm dependencies and download the VS Code built-in plugins
	npm install
	$(MAKE) plugins

plugins: ## (Re)download the VS Code built-in plugins from Open VSX into plugins/
	npm run download:plugins

extension: ## Compile the review extension (TypeScript)
	npm run -w @co-review/review build

browser: extension ## Build the browser app
	cd $(BROWSER) && $(THEIA) rebuild:browser --cacheRoot ../.. && $(THEIA) build --mode $(MODE)

start: browser ## Run the browser app (PORT=3000 REPO=/path/to/repo)
	node bin/co-review.mjs --port $(PORT) $(REPO)
	@echo "Co-Review is running in the background on http://127.0.0.1:$(PORT) (log: ~/.co-review/server.log)"

dev: ## Rebuild the extension on change; reload the browser to pick up frontend changes
	npm run -w @co-review/review watch & (cd $(BROWSER) && $(THEIA) build --watch --mode development)

desktop-build: extension ## Build the desktop (Electron) app
	cd $(ELECTRON) && $(THEIA) rebuild:electron --cacheRoot ../.. && $(THEIA) build --mode $(MODE)

desktop: desktop-build ## Run the desktop app (REPO=/path/to/repo)
	cd $(ELECTRON) && ../../node_modules/.bin/electron . $(REPO)

package: desktop-build ## Package the desktop app for this OS into applications/electron/dist
	rm -rf $(ELECTRON)/plugins && cp -R plugins $(ELECTRON)/plugins
	cd $(ELECTRON) && ../../node_modules/.bin/electron-builder --config electron-builder.yml

package-dir: desktop-build ## Package the desktop app unpacked (faster; for testing)
	rm -rf $(ELECTRON)/plugins && cp -R plugins $(ELECTRON)/plugins
	cd $(ELECTRON) && ../../node_modules/.bin/electron-builder --config electron-builder.yml --dir

APPS ?= $(shell [ -w /Applications ] && echo /Applications || echo $(HOME)/Applications)
MAC_APP = $(ELECTRON)/dist/mac-$(shell uname -m | sed 's/x86_64/x64/')/Co-Review.app

install-app: package ## macOS: install Co-Review.app into /Applications (or ~/Applications) and the `co-review` command
	@test "$$(uname)" = Darwin || { echo "make install-app is macOS-only; use make install-cli"; exit 1; }
	mkdir -p "$(APPS)" && rm -rf "$(APPS)/Co-Review.app" && cp -R "$(MAC_APP)" "$(APPS)/"
	"$(APPS)/Co-Review.app/Contents/Resources/app/bin/install-cli.sh"

install-cli: ## Install the `co-review` command running from this checkout (needs `make browser`)
	bin/install-cli.sh

mcp-claude: ## Register Co-Review as an MCP server in Claude Code (user scope)
	claude mcp add -s user co-review -- node $(CURDIR)/bin/co-review.mjs mcp

clean: ## Remove build output
	rm -rf extensions/review/lib $(BROWSER)/lib $(BROWSER)/src-gen $(ELECTRON)/lib $(ELECTRON)/src-gen $(ELECTRON)/dist $(ELECTRON)/plugins

distclean: clean ## Also remove node_modules, plugins and native-module caches
	rm -rf node_modules plugins .browser_modules

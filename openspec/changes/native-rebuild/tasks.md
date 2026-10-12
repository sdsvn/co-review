## 0. Fork and spikes

- [ ] 0.1 Fork the host editor chosen in `design.md`, rebrand, build on macOS, Linux and Windows CI
- [ ] 0.2 Spike: a thread block with a reply input inside the editor
- [ ] 0.3 Spike: a patch page as a multibuffer with thread blocks
- [ ] 0.4 Spike: Mermaid diagrams with node and edge hit areas
- [ ] 0.5 Record the Theia app's MCP sessions and the anchor, patch, design and review-data corpora

## 1. Headless core

- [ ] 1.1 review-data, comment-anchoring (tracking), review_syntax
- [ ] 1.2 Review directories, patches, design format check
- [ ] 1.3 mcp-agents and the cli bridge; Claude Code plugin and Pi pass all four workflows headless

## 2. Code review UI

- [ ] 2.1 inline-review and live placement
- [ ] 2.2 review-panel and Submit review

## 3. Change and pull-request review

- [ ] 3.1 change-review
- [ ] 3.2 github-posting

## 4. Documents and overview

- [ ] 4.1 document-review
- [ ] 4.2 overview-page

## 5. Repository review

- [ ] 5.1 repository-review with the per-repository caches

## 6. ACP agents

- [ ] 6.1 acp-agents

## 7. App

- [ ] 7.1 desktop-app, remaining cli

## 8. Ship

- [ ] 8.1 Signing, installers, `install.sh`, release workflow
- [ ] 8.2 Prompts, skills and `llms.txt`: "the review is open in Co-Review" instead of a URL
- [ ] 8.3 Docs and site
- [ ] 8.4 Remove the TypeScript app: extensions/, applications/, the npm/Theia build, the Node launchers, and the
      tests/gen recorders (their fixtures stay as the reference); Makefile, release workflow and install.sh build and
      install the Lapce app only

#!/usr/bin/env node
/**
 * Records what the Theia app's document page shows as text, for the review_markdown crate's tests.
 *
 * The page's text (the DOM text nodes text anchors are computed over, `text-anchor.ts`) comes from the compiled
 * `document-render.js` plus what `document-review-widget.tsx` adds around it: the frontmatter header, the format
 * warnings banner and each diagram's head (its block id). The HTML is turned into text nodes the way a browser parses
 * it, without a DOM: text between tags, entities decoded. Anchors for chosen selections and the place `markText`
 * picks for an anchor follow `text-anchor.ts` exactly.
 *
 * Usage: node crates/review_markdown/tests/gen/record.mjs   (needs the Theia app built: extensions/review/lib)
 */

import { writeFileSync, mkdirSync, readFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { createRequire } from 'module';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..', '..', '..', '..');
const require = createRequire(import.meta.url);
const render = require(join(root, 'extensions/review/lib/browser/document/document-render.js'));
const format = require(join(root, 'extensions/review/lib/common/design-format.js'));

const escape = text => text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;');

/** The widget's content HTML: header, warnings banner, rendered body, diagram heads (`load`, `renderDiagrams`). */
function pageHtml(markdown, fileName) {
    const rendered = render.render(markdown, fileName);
    let html = rendered.html.replace(/<div class="co-review-dgm" data-block-id="([^"]*)"><\/div>/g,
        (_m, id) => `<div class="co-review-dgm" data-block-id="${id}"><div class="co-review-dgm-head"><span class="codicon"></span><span>${id}</span><span class="co-review-spacer"></span><span class="codicon" data-diagram="1"></span></div><div class="co-review-dgm-body"></div></div>`);
    if (rendered.format.warnings.length) {
        html = `<div class="co-review-format-warnings"><strong>${rendered.format.declared ? 'Declares <code>co-review: design</code>, but:' : 'Format:'}</strong>`
            + `<ul>${rendered.format.warnings.map(w => `<li>${w.replace(/</g, '&lt;').replace(/`([^`]+)`/g, '<code>$1</code>')}</li>`).join('')}</ul></div>` + html;
    }
    const { data } = format.splitFrontmatter(markdown);
    if (data.type && !data['co-review']) {
        html = '<div class="co-review-doc-meta">' + `<span class="co-review-doc-type">${escape(data.type)}</span>`
            + (data.description ? `<span>${escape(data.description)}</span>` : '')
            + (data.resource ? `<a href="${escape(data.resource)}"><span class="codicon"></span>${escape(data.resource)}</a>` : '')
            + '</div>' + html;
    }
    return { html, rendered };
}

const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ' };

/** Text nodes of an HTML fragment, in document order (what a TreeWalker over SHOW_TEXT returns). */
function textNodes(html) {
    const nodes = [];
    for (const piece of html.split(/<[^>]*>/)) {
        if (piece) {
            nodes.push(piece.replace(/&(#x[0-9a-f]+|#\d+|\w+);/gi, (m, name) =>
                name[0] === '#' ? String.fromCodePoint(name[1] === 'x' || name[1] === 'X' ? parseInt(name.slice(2), 16) : Number(name.slice(1)))
                    : ENTITIES[name] ?? m));
        }
    }
    return nodes;
}

/** `textAnchor` for a selection from `start` to `end` of the page's text. */
function textAnchor(full, start, end) {
    while (start < end && /\s/.test(full[start])) {
        start++;
    }
    while (end > start && /\s/.test(full[end - 1])) {
        end--;
    }
    const exact = full.slice(start, end);
    return exact.length < 3 ? null : {
        type: 'text', exact,
        prefix: full.slice(Math.max(0, start - 40), start),
        suffix: full.slice(end, end + 40),
        startOffsetHint: start, endOffsetHint: end
    };
}

/** Where `markText` marks an anchor: the start offset of the chosen occurrence, or null. */
function markAt(full, anchor) {
    const exact = anchor.exact ?? '';
    if (!exact) {
        return null;
    }
    const candidates = [];
    for (let i = full.indexOf(exact); i >= 0; i = full.indexOf(exact, i + 1)) {
        candidates.push(i);
    }
    if (!candidates.length) {
        return null;
    }
    if (anchor.occurrence !== undefined && anchor.occurrence < candidates.length) {
        return candidates[anchor.occurrence];
    }
    const score = i => (anchor.prefix && full.slice(Math.max(0, i - anchor.prefix.length), i) === anchor.prefix ? 2 : 0)
        + (anchor.suffix && full.slice(i + exact.length, i + exact.length + anchor.suffix.length) === anchor.suffix ? 2 : 0)
        - Math.abs(i - (anchor.startOffsetHint ?? i)) / 1e6;
    return candidates.sort((a, b) => score(b) - score(a))[0];
}

const designDocs = readFileSync(join(root, 'docs/design-docs.md'), 'utf8');
const examples = [...designDocs.matchAll(/^(`{3,4})markdown\s*\n([\s\S]*?)^\1\s*$/gm)].map(m => m[2]);
const example = title => examples.find(e => e.includes(`\n${title}\n`)) ?? (() => { throw new Error(`no example ${title}`); })();

/**
 * Each case: a document, and selections as [text, occurrence] (the occurrence-th place the text is on the page,
 * widened by `pad` characters on each side to exercise trimming), plus anchors to locate.
 */
const cases = [
    {
        name: 'plain_blocks', file: 'notes.md',
        markdown: '# Notes\n\nA paragraph with *emphasis*, **strong**, `code` and a [link](src/a.ts:42).\nA soft break follows.  \nThen a hard break.\n\n## Lists\n\n- one\n- two\n  - nested\n- three\n\n1. first\n2. second\n\n- loose one\n\n- loose two\n\n> A quote\n> over two lines.\n\n---\n\n```rust\nfn main() {}\n```\n\n    indented code\n\nThe end.\n',
        select: [['emphasis, strong', 0], ['two\n\nnested', 0], ['one', 0], ['A quote', 0], ['fn main', 0], ['The end', 0], ['link.\nA soft', 0], ['Lists\n\none', 0]],
        locate: [{ type: 'text', exact: 'one', prefix: 'loose ', suffix: '' }, { type: 'text', exact: 'two', occurrence: 2 }]
    },
    {
        name: 'table_and_html', file: 'table.md',
        markdown: 'Intro <b>bold</b> text.\n\n| Name | Value |\n|---|--:|\n| a | 1 |\n| b |  |\n\n<div>\nblock html\n</div>\n\n~~gone~~ and https://example.com/x linked.\n\n![alt text](img.png) after image.\n\n- [ ] task\n- [x] done\n',
        select: [['Name', 0], ['a\n1', 0], ['block html', 0], ['gone', 0], ['after image', 0], ['task', 0]],
        locate: []
    },
    {
        name: 'mermaid_plain', file: 'diagram.md',
        markdown: '# Diagram\n\nBefore.\n\n```mermaid\n%% id: flow\nflowchart TD\n  a[Start] --> b[End]\n```\n\nBetween.\n```mermaid\nsequenceDiagram\n  A->>B: hi\n```\nAfter.\n',
        select: [['Before.\nflow', 0], ['Between', 0], ['After', 0]],
        locate: []
    },
    {
        name: 'bundle_page', file: 'limiter.md',
        markdown: '---\ntype: Package\ndescription: Rate limiting for the API\nresource: src/limiter.go\n---\n# Limiter\n\nSee `src/limiter.go:42` and [the store](/packages/store.md), [code](src/limiter.go#L10-L20).\n',
        select: [['Package', 0], ['Rate limiting', 0], ['Limiter\nSee', 0]],
        locate: []
    },
    {
        name: 'design_small', file: 'index.markdown', markdown: example('# Rate-limit password-reset emails'),
        select: [['shared rate limiter', 0], ['Context', 0], ['Still answer', 0]],
        locate: [{ type: 'text', exact: 'rate limiter', prefix: 'Reuse the shared ', suffix: ', keyed' }]
    },
    {
        name: 'design_medium', file: 'index.markdown', markdown: example('# Retry outbound webhooks'),
        select: [['at least once', 0], ['Endpoint down for an hour', 0], ['job runner', 1]],
        locate: [{ type: 'text', exact: 'job', startOffsetHint: 900 }]
    },
    {
        name: 'design_large', file: 'index.markdown', markdown: example('# Charge customers through an outbox'),
        select: [['new payment_outbox table', 0], ['charge', 3], ['Two workers poll', 0], ['exactly once', 0]],
        locate: []
    },
    {
        name: 'design_warnings', file: 'index.md',
        markdown: '---\nco-review: design\n---\n# Warned\n\n## Design\n* not a step\n\n## Extra\nSee a.ts:12 here.\n\n```js\nx()\n```\n',
        select: [['not a step', 0], ['Declares', 0], ['See a.ts', 0]],
        locate: []
    },
    {
        name: 'pseudocode_file', file: 'retry.pseudocode.md',
        markdown: '# Retry\n\nPreamble text.\n\n## Design\n- Retry `?` failures — they are transient\n  - Back off **exponentially** `⚠`\n    - Cap at *one hour* `✎`\n- Give up\n\n## Open Questions\n- Should we alert?\n\n## Notes\nFree text.\n',
        select: [['Retry ? failures', 0], ['they are transient', 0], ['Back off exponentially', 0], ['Should we alert', 0], ['Free text', 0]],
        locate: []
    },
    {
        name: 'overview_page', file: 'overview.md',
        markdown: '# PR #12: Limit resets\n\n3 threads, first pass by claude-code: 2 proposed, 1 open, 0 resolved. Accept or dismiss findings in the Review panel, open one with its link, or comment on this page.\n\n## The pull request\n\n[Limit resets](https://github.com/o/r/pull/12) · `feature/x` into `main` · 2 commits\n\n## Files changed\n\n| File | + | − |\n|---|---:|---:|\n| [src/a.ts](src/a.ts) | 10 | 2 |\n\n## Threads\n\n### src\n\n- **High** · proposed · [src/a.ts:42](src/a.ts:42) — The limit is never reset.\n  > Reset it on the window boundary.\n',
        select: [['first pass', 0], ['src/a.ts', 1], ['never reset', 0], ['Reset it on', 0]],
        locate: []
    }
];

const out = join(here, '..', 'fixtures');
mkdirSync(out, { recursive: true });
for (const c of cases) {
    const { html, rendered } = pageHtml(c.markdown, c.file);
    const full = textNodes(html).join('');
    const selections = c.select.map(([text, occurrence]) => {
        let at = -1;
        for (let n = 0; n <= occurrence; n++) {
            at = full.indexOf(text, at + 1);
        }
        if (at < 0) {
            throw new Error(`${c.name}: ${JSON.stringify(text)} #${occurrence} is not on the page:\n${full}`);
        }
        // Widened by a space-or-newline on each side when there is one, as a mouse selection often is.
        const start = at > 0 && /\s/.test(full[at - 1]) ? at - 1 : at;
        const end = /\s/.test(full[at + text.length] ?? '') ? at + text.length + 1 : at + text.length;
        return { text, occurrence, start, end, anchor: textAnchor(full, start, end) };
    });
    const located = c.locate.map(anchor => ({ anchor, at: markAt(full, anchor) }));
    const fixture = {
        name: c.name, fileName: c.file, markdown: c.markdown,
        title: rendered.title, pseudocode: rendered.pseudocode, blocks: rendered.blocks, format: rendered.format,
        text: full, selections, located
    };
    writeFileSync(join(out, `${c.name}.json`), JSON.stringify(fixture, null, 2) + '\n');
}
console.log(`${cases.length} fixtures written to ${out}`);

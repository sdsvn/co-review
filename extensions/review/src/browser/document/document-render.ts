import * as markdownit from '@theia/core/shared/markdown-it';
import { DESIGN_SECTIONS, designSectionIndex, DocumentFormat, documentFormat, sectionsOf, splitFrontmatter, Step, steps } from '../../common/design-format';

/**
 * Markdown / pseudocode rendering for document review. Block ids, node ids and step ids follow
 * the documented anchor format exactly, so agents can compute them (docs/design-docs.md, llms.txt).
 */

export interface MermaidBlock {
    id: string;
    code: string;
}

export interface RenderedDocument {
    title: string;
    html: string;
    blocks: MermaidBlock[];
    pseudocode: boolean;
    format: DocumentFormat;
}

const md = markdownit({ html: false, linkify: true, breaks: false });

function esc(text: string): string {
    return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

/** Replaces ```mermaid fences with host divs; `%% id: <name>` gives a block a stable id. */
export function extractMermaid(markdown: string): { markdown: string; blocks: MermaidBlock[] } {
    const blocks: MermaidBlock[] = [];
    const out = markdown.replace(/```mermaid\n([\s\S]*?)```/g, (_m, code: string) => {
        const idm = code.match(/%%\s*id:\s*([\w-]+)/);
        const id = idm ? idm[1] : `block-${blocks.length + 1}`;
        blocks.push({ id, code: code.replace(/%%\s*id:.*\n/, '').trim() });
        // A placeholder paragraph markdown-it leaves alone; swapped for the host div after rendering.
        return `\n\nCOREVIEWMERMAID${blocks.length - 1}\n\n`;
    });
    return { markdown: out, blocks };
}

function withDiagramHosts(html: string, blocks: MermaidBlock[]): string {
    return html.replace(/<p>COREVIEWMERMAID(\d+)<\/p>/g, (_m, i: string) =>
        `<div class="co-review-dgm" data-block-id="${esc(blocks[Number(i)].id)}"></div>`);
}

export function title(markdown: string, fallback: string): string {
    const { data, body } = splitFrontmatter(markdown);
    const h1 = body.match(/^#\s+(.+?)\s*$/m);
    return data.title || (h1 ? h1[1].replace(/^Feature:\s*/i, '') : fallback);
}

/** Stable step id: marker/format-stripped text, whitespace-collapsed, capped at 80. */
export function stepId(text: string): string {
    return (text || '').replace(/[`*]/g, '').replace(/\s+/g, ' ').trim().slice(0, 80);
}

const MARKERS: Record<string, [string, string]> = {
    '?': ['q', 'open question'],
    '⚠': ['warn', 'risk'],
    '✎': ['name', 'naming / new artifact']
};

function stepInline(text: string): string {
    let out = esc(text);
    for (const [glyph, [cls, hint]] of Object.entries(MARKERS)) {
        out = out.split('`' + glyph + '`').join(`<span class="co-review-pbadge ${cls}" title="${hint}">${glyph}</span>`);
    }
    out = out
        .replace(/`([^`]+)`/g, '<code>$1</code>')
        .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
        .replace(/\*([^*]+)\*/g, '<em>$1</em>');
    const i = out.indexOf(' — ');
    if (i >= 0) {
        out = out.slice(0, i) + ` <span class="co-review-reason">— ${out.slice(i + 3)}</span>`;
    }
    return out;
}

function renderSteps(nodes: Step[]): string {
    return nodes.map(node => {
        const id = esc(stepId(node.text));
        const line = `<span class="co-review-step-text">${stepInline(node.text)}</span>`
            + `<span class="co-review-step-comment codicon codicon-comment" title="Comment on this step" data-step="${id}"></span>`;
        if (!node.children.length) {
            return `<li class="co-review-step leaf" data-node-id="${id}" data-depth="${node.depth}"><div class="co-review-step-line">${line}</div></li>`;
        }
        return `<li class="co-review-step" data-node-id="${id}" data-depth="${node.depth}"><details${node.depth === 1 ? ' open' : ''}>`
            + `<summary class="co-review-step-line">${line}</summary><ul>${renderSteps(node.children)}</ul></details></li>`;
    }).join('');
}

export function render(markdown: string, fileName: string): RenderedDocument {
    const format = documentFormat(markdown, fileName);
    const { body } = splitFrontmatter(markdown);
    const docTitle = title(markdown, fileName);
    if (format.mode === 'plain') {
        const { markdown: withoutDiagrams, blocks } = extractMermaid(body);
        return { title: docTitle, html: withDiagramHosts(md.render(withoutDiagrams), blocks), blocks, pseudocode: false, format };
    }
    const blocks: MermaidBlock[] = [];
    const prose = (text: string | undefined) => {
        if (!text?.trim()) {
            return '';
        }
        const extracted = extractMermaid(text);
        const offset = blocks.length;
        extracted.blocks.forEach((b, i) => blocks.push(b.id.startsWith('block-') ? { ...b, id: `block-${offset + i + 1}` } : b));
        return withDiagramHosts(md.render(extracted.markdown).replace(/COREVIEWMERMAID(\d+)/g, (_m, i) => `COREVIEWMERMAID${offset + Number(i)}`), blocks);
    };
    const { preamble, sections } = sectionsOf(body);
    let html = `<h1>${esc(docTitle)}</h1>` + prose(preamble);
    // Every section renders, in document order: the contract's tree sections as step trees, the rest as Markdown.
    for (const section of sections) {
        const known = DESIGN_SECTIONS[designSectionIndex(section.key)];
        const tree = known?.tree && steps(section.body).length > 0;
        if (tree && section.key === 'design') {
            html += `<section class="co-review-psec co-review-design"><h2>${esc(section.heading)} <span class="co-review-altitude">`
                + '<button data-level="1">L1</button><button data-level="2">L2</button><button data-level="3">L3</button></span></h2>'
                + `<ul class="co-review-tree">${renderSteps(steps(section.body))}</ul></section>`;
        } else if (tree) {
            html += `<section class="co-review-psec"><h2>${esc(section.heading)}</h2><ul class="co-review-tree">${renderSteps(steps(section.body))}</ul></section>`;
        } else {
            html += `<section class="co-review-psec"><h2>${esc(section.heading)}</h2>${prose(section.body)}</section>`;
        }
    }
    return { title: docTitle, html, blocks, pseudocode: true, format };
}

/**
 * The design-document contract. A document declares it follows the structure with frontmatter
 *
 *     ---
 *     co-review: design
 *     ---
 *
 * and is then checked against it (`warnings`). Undeclared documents are detected by name or heading
 * (`*.pseudocode.md`, a `## Design` section) and rendered leniently; anything else is plain Markdown.
 * No mode drops content: sections outside the contract render as Markdown.
 */

export const DESIGN_FORMAT = 'design';

/**
 * Sections of the contract, in their canonical order; `tree` sections render as step trees. Only Design is
 * required. `aliases` are other lower-cased headings accepted for the same section.
 */
export const DESIGN_SECTIONS: { key: string; heading: string; tree: boolean; aliases?: string[] }[] = [
    { key: 'context', heading: 'Context', tree: false },
    { key: 'design', heading: 'Design', tree: true },
    { key: 'alternatives', heading: 'Alternatives', tree: false },
    { key: 'behaviour', heading: 'Behaviour', tree: false, aliases: ['behavior'] },
    { key: 'diagrams', heading: 'Diagrams', tree: false },
    { key: 'open questions', heading: 'Open Questions', tree: true }
];

/** Index of the contract section a heading key names (its key or an alias), or -1. */
export function designSectionIndex(key: string): number {
    return DESIGN_SECTIONS.findIndex(s => s.key === key || s.aliases?.includes(key));
}

export interface Section {
    heading: string;
    /** Lower-cased heading. */
    key: string;
    body: string;
}

export interface DocumentFormat {
    mode: 'design' | 'plain';
    /** The frontmatter declares `co-review: design`. */
    declared: boolean;
    /** Where a declared document departs from the contract. */
    warnings: string[];
}

/** Splits leading YAML frontmatter (flat `key: value` lines) from the body. */
export function splitFrontmatter(markdown: string): { data: Record<string, string>; body: string } {
    const m = markdown.match(/^---[ \t]*\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/);
    if (!m) {
        return { data: {}, body: markdown };
    }
    const data: Record<string, string> = {};
    for (const line of m[1].split(/\r?\n/)) {
        const kv = line.match(/^([\w-]+)\s*:\s*(.*?)\s*$/);
        if (kv) {
            data[kv[1].toLowerCase()] = kv[2].replace(/\s+#.*$/, '').replace(/^(['"])(.*)\1$/, '$2');
        }
    }
    return { data, body: markdown.slice(m[0].length) };
}

/** `## ` sections in document order; `preamble` is what precedes the first one (the `# ` title excluded). */
export function sectionsOf(body: string): { preamble: string; sections: Section[] } {
    const sections: Section[] = [];
    const preamble: string[] = [];
    let current: Section | undefined;
    let fence = false;
    for (const line of body.split('\n')) {
        if (/^\s*(```|~~~)/.test(line)) {
            fence = !fence;
        }
        const m = !fence && line.match(/^##\s+(.+?)\s*#*\s*$/);
        if (m) {
            current = { heading: m[1], key: m[1].toLowerCase(), body: '' };
            sections.push(current);
        } else if (current) {
            current.body += line + '\n';
        } else if (!/^#\s+/.test(line)) {
            preamble.push(line);
        }
    }
    return { preamble: preamble.join('\n'), sections };
}

export interface Step {
    text: string;
    indent: number;
    depth: number;
    children: Step[];
}

/** Indented `- ` bullets → tree, nesting by relative indent (tabs = 4); continuation lines fold up. */
export function steps(body = ''): Step[] {
    const root: Step = { text: '', indent: -1, depth: 0, children: [] };
    const stack: Step[] = [root];
    const width = (ws: string) => ws.replace(/\t/g, '    ').length;
    for (const line of body.split('\n')) {
        const m = line.match(/^([ \t]*)- (.*)$/);
        if (m) {
            const indent = width(m[1]);
            while (stack.length > 1 && stack[stack.length - 1].indent >= indent) {
                stack.pop();
            }
            const parent = stack[stack.length - 1];
            const node: Step = { text: m[2].trim(), indent, depth: parent.depth + 1, children: [] };
            parent.children.push(node);
            stack.push(node);
        } else if (line.trim() && stack.length > 1) {
            stack[stack.length - 1].text += ' ' + line.trim();
        }
    }
    return root.children;
}

export function isDesignFormat(value: string | undefined): boolean {
    return (value ?? '').trim().toLowerCase() === DESIGN_FORMAT;
}

/** How a document renders, and — when it declares the contract — where it departs from it. */
export function documentFormat(markdown: string, fileName: string): DocumentFormat {
    const { data, body } = splitFrontmatter(markdown);
    const declaredValue = data['co-review'];
    const declared = isDesignFormat(declaredValue);
    const { sections } = sectionsOf(body);
    const detected = /\.pseudocode\.md$/i.test(fileName) || sections.some(s => s.key === 'design');
    const warnings: string[] = [];
    if (declaredValue !== undefined && !declared) {
        warnings.push(`Unknown \`co-review: ${declaredValue}\` — the only format is \`co-review: ${DESIGN_FORMAT}\`; rendered as plain Markdown.`);
    }
    if (declared) {
        if (!/^#\s+\S/m.test(body) && !data.title) {
            warnings.push('No `# ` title (or `title:` in the frontmatter).');
        }
        const design = sections.find(s => s.key === 'design');
        if (!design) {
            warnings.push('No `## Design` section.');
        } else if (!steps(design.body).length) {
            warnings.push(/^\s*([*+]|\d+[.)])\s/m.test(design.body)
                ? '`## Design` uses `*`, `+` or numbered items; steps must be `- ` bullets.'
                : '`## Design` has no `- ` step list.');
        } else if (/^\s*([*+]|\d+[.)])\s/m.test(design.body)) {
            warnings.push('`## Design` mixes `- ` with `*`, `+` or numbered items; only `- ` items become steps.');
        }
        const extra = sections.filter(s => designSectionIndex(s.key) < 0).map(s => `\`## ${s.heading}\``);
        if (extra.length) {
            warnings.push(`Sections outside the contract (shown as plain Markdown): ${extra.join(', ')}.`);
        }
        const order = sections.map(s => designSectionIndex(s.key)).filter(i => i >= 0);
        if (order.some((v, i) => i > 0 && v < order[i - 1])) {
            warnings.push(`Sections are out of order (${DESIGN_SECTIONS.map(s => s.heading).join(', ')}).`);
        }
        warnings.push(...readabilityWarnings(sections));
    }
    return { mode: declared || (detected && declaredValue === undefined) ? 'design' : 'plain', declared, warnings };
}

/** Steps longer than this (about two lines) usually hold more than one decision. */
const LONG_STEP = 220;
/** A code name in this many steps or more is repeated rather than introduced once. */
const REPEATED_NAME = 3;

/**
 * A design document is read by a person, not a compiler: code details that belong in the implementation are
 * reported, so the agent can fix them before the reviewer reads the document.
 */
function readabilityWarnings(sections: Section[]): string[] {
    const warnings: string[] = [];
    const prose = sections.map(s => s.body.replace(/^\s*(```|~~~)[\s\S]*?^\s*\1\s*$/gm, '')).join('\n').replace(/\bhttps?:\/\/\S+/g, '');
    const lineRefs = prose.match(/[\w./-]+\.[a-z]{1,5}:\d+|#L\d+/gi) ?? [];
    if (lineRefs.length) {
        warnings.push(`Line references (${lineRefs.slice(0, 3).map(r => `\`${r}\``).join(', ')}): a person reads this; describe the code in words and name only the top-level module or function.`);
    }
    const code = sections.flatMap(s => [...s.body.matchAll(/^\s*(?:```|~~~)\s*(\w*)/gm)].filter((_m, i) => i % 2 === 0).map(m => m[1]));
    if (code.some(lang => lang.toLowerCase() !== 'mermaid')) {
        warnings.push('Code blocks other than Mermaid: describe the behaviour in words; the code belongs in the implementation.');
    }
    const design = sections.find(s => s.key === 'design');
    const all: Step[] = [];
    const walk = (list: Step[]): void => list.forEach(step => {
        all.push(step);
        walk(step.children);
    });
    walk(steps(design?.body));
    const long = all.filter(s => s.text.length > LONG_STEP).length;
    if (long) {
        warnings.push(`${long} step${long > 1 ? 's are' : ' is'} longer than two lines: keep one decision per step, and move details into child steps.`);
    }
    const uses = new Map<string, number>();
    for (const step of all) {
        new Set([...step.text.matchAll(/`([^`?⚠✎]+)`/g)].map(m => m[1])).forEach(name => uses.set(name, (uses.get(name) ?? 0) + 1));
    }
    const repeated = [...uses].filter(([, n]) => n >= REPEATED_NAME).map(([name, n]) => `\`${name}\` (${n} steps)`);
    if (repeated.length) {
        warnings.push(`Code names repeated across steps: ${repeated.slice(0, 3).join(', ')}. Name each once where it is introduced, then refer to it in words.`);
    }
    return warnings;
}

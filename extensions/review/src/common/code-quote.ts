/**
 * Finds the lines a finding quotes (`code`), so a finding lands on the code it is about instead of on the line number
 * the agent wrote down. Lines compare without their indentation and trailing spaces; blank lines around the quote are
 * ignored. A quote copied from a diff (`+`/`-`/` ` in front of every line) also matches.
 */

import { PatchFile } from './patch';

/** 1-based, inclusive. */
export interface QuotedLines {
    start: number;
    end: number;
}

export type QuoteResult = { found: QuotedLines } | { error: string };

/** A run of consecutive lines with their line numbers; `changed` marks the lines a patch adds or removes. */
interface Segment {
    lines: string[];
    numbers: number[];
    changed: boolean[];
}

function normalize(line: string): string {
    return line.trim().replace(/\s+/g, ' ');
}

function quoteLines(code: string): string[] {
    const lines = code.replace(/\r\n?/g, '\n').split('\n');
    while (lines.length && !lines[0].trim()) {
        lines.shift();
    }
    while (lines.length && !lines[lines.length - 1].trim()) {
        lines.pop();
    }
    return lines;
}

function matches(segments: Segment[], quote: string[]): { lines: QuotedLines; changed: boolean }[] {
    const wanted = quote.map(normalize);
    const found: { lines: QuotedLines; changed: boolean }[] = [];
    for (const segment of segments) {
        const have = segment.lines.map(normalize);
        for (let i = 0; i + wanted.length <= have.length; i++) {
            if (wanted.every((w, k) => have[i + k] === w)) {
                found.push({
                    lines: { start: segment.numbers[i], end: segment.numbers[i + wanted.length - 1] },
                    changed: segment.changed.slice(i, i + wanted.length).some(Boolean)
                });
            }
        }
    }
    return found;
}

function locate(segments: Segment[], code: string, near: number | undefined, where: string): QuoteResult {
    const quote = quoteLines(code);
    if (!quote.length) {
        return { error: `\`code\` is empty: quote the lines of ${where} the finding is about.` };
    }
    let found = matches(segments, quote);
    if (!found.length && quote.every(l => /^[+\- ]/.test(l))) {
        found = matches(segments, quote.map(l => l.slice(1)));
    }
    if (!found.length) {
        return { error: `\`code\` doesn't match ${where}: copy the lines exactly as they are there (one or a few consecutive lines).` };
    }
    // On a patch, a quote that touches the change is what the finding is about; the same lines as context elsewhere aren't.
    if (found.some(f => f.changed)) {
        found = found.filter(f => f.changed);
    }
    if (found.length > 1) {
        if (near === undefined) {
            const at = found.map(f => f.lines.start).join(', ');
            return { error: `\`code\` matches ${found.length} places in ${where} (lines ${at}): quote more lines, or give \`line\` to pick one.` };
        }
        found.sort((a, b) => Math.abs(a.lines.start - near) - Math.abs(b.lines.start - near));
    }
    return { found: found[0].lines };
}

/** The quoted lines in a file's text; `near` (a line number) picks one when the quote is there more than once. */
export function locateInText(text: string, code: string, near?: number, where = 'the file'): QuoteResult {
    const lines = text.replace(/\r\n?/g, '\n').split('\n');
    return locate([{ lines, numbers: lines.map((_, i) => i + 1), changed: lines.map(() => false) }], code, near, where);
}

/**
 * The quoted lines on one side of a patch file: `new` (the lines kept and added) or `old` (kept and removed). Each hunk
 * is searched on its own, since the lines between hunks aren't in the patch.
 */
export function locateInPatch(file: PatchFile, code: string, side: 'new' | 'old' = 'new', near?: number): QuoteResult {
    const segments: Segment[] = [];
    let current: Segment | undefined;
    for (const row of file.rows) {
        if (row.t === 'hunk') {
            current = { lines: [], numbers: [], changed: [] };
            segments.push(current);
            continue;
        }
        const n = side === 'new' ? row.n : row.o;
        if (!current || n === undefined || row.t === (side === 'new' ? 'del' : 'add')) {
            continue;
        }
        current.lines.push(row.s);
        current.numbers.push(n);
        current.changed.push(row.t !== 'ctx');
    }
    return locate(segments, code, near, `the ${side} side of ${file.path} in the patch`);
}

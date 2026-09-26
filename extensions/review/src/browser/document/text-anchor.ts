import { DocAnchor } from '../../common/review-model';

/**
 * Text anchors in rendered content (Markdown pages): a quote with some context before and
 * after, located again by text rather than by position. `skip` excludes elements that are not part of
 * the reviewed content (inline threads, rendered diagrams, scripts).
 */
function textNodes(root: HTMLElement, skip: string): { nodes: Text[]; starts: number[]; full: string } {
    const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
        acceptNode: n => (n.parentElement?.closest(skip) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT)
    });
    const nodes: Text[] = [];
    const starts: number[] = [];
    let full = '';
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        starts.push(full.length);
        nodes.push(n as Text);
        full += (n as Text).data;
    }
    return { nodes, starts, full };
}

/** The content's text as anchors see it. */
export function plainText(root: HTMLElement, skip: string): string {
    return textNodes(root, skip).full;
}

/** A text anchor for `exact`, with 40 characters of context on each side. */
export function textAnchor(root: HTMLElement, skip: string, exact: string): DocAnchor {
    const full = plainText(root, skip);
    const at = full.indexOf(exact);
    return {
        type: 'text', exact,
        prefix: at > 0 ? full.slice(Math.max(0, at - 40), at) : '',
        suffix: at >= 0 ? full.slice(at + exact.length, at + exact.length + 40) : '',
        startOffsetHint: at, endOffsetHint: at + exact.length
    };
}

/** Wraps the quoted text in <mark>s: prefix/suffix disambiguate, the offset hint breaks ties. */
export function markText(root: HTMLElement, anchor: DocAnchor, skip: string): HTMLElement[] {
    const exact = anchor.exact ?? '';
    if (!exact) {
        return [];
    }
    const { nodes, starts, full } = textNodes(root, skip);
    const candidates: number[] = [];
    for (let i = full.indexOf(exact); i >= 0; i = full.indexOf(exact, i + 1)) {
        candidates.push(i);
    }
    if (!candidates.length) {
        return [];
    }
    const score = (i: number) => (anchor.prefix && full.slice(Math.max(0, i - anchor.prefix.length), i) === anchor.prefix ? 2 : 0)
        + (anchor.suffix && full.slice(i + exact.length, i + exact.length + anchor.suffix.length) === anchor.suffix ? 2 : 0)
        - Math.abs(i - (anchor.startOffsetHint ?? i)) / 1e6;
    const at = candidates.sort((a, b) => score(b) - score(a))[0];
    const end = at + exact.length;
    const doc = root.ownerDocument;
    const marks: HTMLElement[] = [];
    nodes.forEach((node, index) => {
        const start = starts[index];
        const stop = start + node.data.length;
        if (stop <= at || start >= end) {
            return;
        }
        const range = doc.createRange();
        range.setStart(node, Math.max(0, at - start));
        range.setEnd(node, Math.min(node.data.length, end - start));
        const mark = doc.createElement('mark');
        mark.className = 'co-review-mark';
        range.surroundContents(mark);
        marks.push(mark);
    });
    return marks;
}

/** Removes the marks `markText` added. */
export function unmark(root: HTMLElement): void {
    root.querySelectorAll('mark.co-review-mark').forEach(m => m.replaceWith(...Array.from(m.childNodes)));
    root.normalize();
}

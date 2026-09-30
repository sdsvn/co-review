import { ChangeSet } from '@codemirror/state';
import { cleanupSemantic, DIFF_EQUAL, makeDiff, match, xIndex } from '@sanity/diff-match-patch';

/**
 * What happened to the text a comment is on, as a file changes: its range is carried through the edits between the
 * version it was on and the current one, and classified. Pure (no files, no DOM), so the backend and the browser
 * resolve anchors the same way.
 *
 * - The edits are either explicit (an edit Co-Review applied itself) or recovered with a character diff
 *   (diff-match-patch), and the range is mapped through them with CodeMirror's change sets, which also say how much of
 *   the range was deleted.
 * - When the mapping can't place it (the range was deleted or rewritten), the original text is searched for with its
 *   context: exact occurrences first, scored by how much context still matches; fuzzy matching only when there is no
 *   exact one, and only near where the range should be. Two places about as good make it ambiguous: never a guess.
 */

/** `active`: the text is there unchanged (it may have moved); `modified`: it changed but is still identified;
 *  `removed`: deleted, or can't be located reliably; `ambiguous`: several places fit about equally well. */
export type AnchorStatus = 'active' | 'modified' | 'removed' | 'ambiguous';

/** A replacement in the old text: characters `from`–`to` become `insert`. */
export interface Edit {
    from: number;
    to: number;
    insert: string;
}

/** A comment's target in one version of a file: character offsets, with the text and its context. */
export interface TrackedRange {
    /** The file's version (see contentVersion) the offsets are in. */
    version: string;
    from: number;
    to: number;
    /** The text at from–to in that version. */
    text: string;
    /** Up to CONTEXT characters before and after it. */
    prefix: string;
    suffix: string;
    status: AnchorStatus;
    /** The text when the comment was made, kept as the target changes. */
    original: string;
    /** For `ambiguous`: the places that fit, best first. */
    candidates?: { from: number; to: number }[];
}

export const TRACKING = {
    /** Characters of context kept on each side of the target. */
    context: 40,
    /**
     * Modified, not removed, when at least this share of the target's characters survive the edits (and match).
     * Tuned with test/anchor-corpus.mjs on 116,000 targets from two repositories' last 270 commits: at 0.7 no line
     * git keeps untouched is misplaced but one, and wrong placements of changed lines are fewest for what is kept.
     */
    modified: 0.7,
    /** Ambiguous when the second-best place scores within this of the best. */
    ambiguousMargin: 0.1,
    /** Fuzzy match threshold (diff-match-patch: 0 exact … 1 anything) and how far from the expected place to look. */
    fuzzyThreshold: 0.3,
    fuzzyDistance: 1000
};

/** A target in `text` from `from` to `to`: its text and context, in version `version`. */
export function rangeIn(text: string, version: string, from: number, to: number, status: AnchorStatus = 'active', original?: string): TrackedRange {
    return {
        version, from, to, status,
        text: text.slice(from, to),
        prefix: text.slice(Math.max(0, from - TRACKING.context), from),
        suffix: text.slice(to, to + TRACKING.context),
        original: original ?? text.slice(from, to)
    };
}

/**
 * The edits that turn `before` into `after`: a line diff first (as git's, so an untouched line is never aligned with a
 * look-alike elsewhere), then characters within the lines that changed (so an edit inside a line is an edit, not a
 * deleted and an added line); semantically cleaned up, so they read as edits.
 */
export function editsBetween(before: string, after: string): Edit[] {
    const diffs = cleanupSemantic(makeDiff(before, after, { checkLines: true, timeout: 2 }));
    const edits: Edit[] = [];
    let at = 0;
    for (let i = 0; i < diffs.length; i++) {
        const [op, text] = diffs[i];
        if (op === DIFF_EQUAL) {
            at += text.length;
            continue;
        }
        // A run of deletions and insertions between two equal parts is one replacement.
        let removed = 0;
        let inserted = '';
        for (; i < diffs.length && diffs[i][0] !== DIFF_EQUAL; i++) {
            if (diffs[i][0] < 0) {
                removed += diffs[i][1].length;
            } else {
                inserted += diffs[i][1];
            }
        }
        i--;
        edits.push({ from: at, to: at + removed, insert: inserted });
        at += removed;
    }
    return edits;
}

/** How alike two texts are, 0 to 1: the share of their characters in common (twice the matches over both lengths). */
export function similarity(a: string, b: string): number {
    if (!a.length && !b.length) {
        return 1;
    }
    const same = makeDiff(a, b, { checkLines: false, timeout: 1 }).filter(([op]) => op === DIFF_EQUAL).reduce((n, [, t]) => n + t.length, 0);
    return (2 * same) / (a.length + b.length);
}

/**
 * Carries `anchor` (in `before`) into `after`, version `version`: through `edits` when Co-Review knows them, else
 * through a diff of the two. Without `before` (no snapshot of the old version), only the search can place it.
 */
export function track(anchor: TrackedRange, before: string | undefined, after: string, version: string, edits?: Edit[]): TrackedRange {
    if (before === undefined) {
        return search(anchor, after, version, anchor.from);
    }
    if (before === after) {
        return { ...anchor, version };
    }
    const changes = ChangeSet.of((edits ?? editsBetween(before, after)).map(e => ({ from: e.from, to: e.to, insert: e.insert })), before.length);
    // An insertion right at either end stays outside the target.
    const from = changes.mapPos(anchor.from, 1);
    const to = Math.max(from, changes.mapPos(anchor.to, -1));
    const now = after.slice(from, to);
    if (now === anchor.text && now.length) {
        return rangeIn(after, version, from, to, 'active', anchor.original);
    }
    // How much of the target the edits deleted, and what they wrote (the only places a lost target can have gone).
    let deleted = 0;
    const written: [number, number][] = [];
    changes.iterChangedRanges((fromA, toA, fromB, toB) => {
        deleted += Math.max(0, Math.min(toA, anchor.to) - Math.max(fromA, anchor.from));
        if (toB > fromB) {
            written.push([fromB, toB]);
        }
    });
    const kept = anchor.to > anchor.from ? 1 - deleted / (anchor.to - anchor.from) : 0;
    if (now.trim() && kept >= TRACKING.modified && similarity(anchor.text, now) >= TRACKING.modified) {
        // The unchanged text, with nearly all its context, where the edits wrote (a moved block): that is the target,
        // not the look-alike the diff happened to line it up with.
        const exact = moved(anchor, after, written);
        if (exact !== undefined) {
            return rangeIn(after, version, exact, exact + anchor.text.length, 'active', anchor.original);
        }
        return rangeIn(after, version, ...snap(after, from, to), 'modified', anchor.original);
    }
    return search(anchor, after, version, from, written);
}

/**
 * Finds the target by its text when mapping couldn't: exact occurrences scored by matching context (one clear best:
 * active; several close: ambiguous), else a fuzzy match near `near` (modified), else removed.
 *
 * `written`: the parts of `text` the edits wrote (known when the old version is). A target that was deleted or
 * rewritten can only have gone there: code that was already there, untouched, is other code, however alike.
 */
export function search(anchor: TrackedRange, text: string, version: string, near: number, written?: [number, number][]): TrackedRange {
    const needle = anchor.text;
    if (!needle.trim()) {
        return { ...anchor, version, status: 'removed' };
    }
    const possible = (from: number, to: number) => !written || written.some(([a, b]) => a < to && b > from);
    const found: { from: number; score: number }[] = [];
    for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, at + 1)) {
        if (possible(at, at + needle.length)) {
            found.push({ from: at, score: contextScore(anchor, text, at, at + needle.length) });
        }
    }
    if (found.length) {
        found.sort((a, b) => b.score - a.score || Math.abs(a.from - near) - Math.abs(b.from - near));
        const [best, second] = found;
        if (second && best.score - second.score <= TRACKING.ambiguousMargin) {
            return {
                ...anchor, version, status: 'ambiguous',
                candidates: found.filter(f => best.score - f.score <= TRACKING.ambiguousMargin).map(f => ({ from: f.from, to: f.from + needle.length }))
            };
        }
        return rangeIn(text, version, best.from, best.from + needle.length, 'active', anchor.original);
    }
    // Fuzzy, near where it should be: diff-match-patch's matcher takes at most 32 characters, so the start of the text.
    const head = needle.slice(0, 32);
    const at = match(text, head, Math.min(Math.max(0, near), text.length), { threshold: TRACKING.fuzzyThreshold, distance: TRACKING.fuzzyDistance });
    if (at >= 0 && possible(at, at + needle.length)) {
        // The matcher finds the place, roughly; the edges come from aligning the text with what is around it.
        const [from, to] = align(needle, text, at);
        if (to > from && similarity(needle, text.slice(from, to)) >= TRACKING.modified) {
            return rangeIn(text, version, from, to, 'modified', anchor.original);
        }
    }
    return { ...anchor, version, status: 'removed', candidates: undefined };
}

/** The one place in `written` holding the target's text unchanged with at least 80% of its context, if there is one. */
function moved(anchor: TrackedRange, text: string, written: [number, number][]): number | undefined {
    const places: number[] = [];
    for (let at = text.indexOf(anchor.text); at >= 0 && places.length < 2; at = text.indexOf(anchor.text, at + 1)) {
        const to = at + anchor.text.length;
        if (written.some(([a, b]) => a < to && b > at) && contextScore(anchor, text, at, to) >= 0.8) {
            places.push(at);
        }
    }
    return places.length === 1 ? places[0] : undefined;
}

/** Where `needle`, found roughly at `at`, starts and ends in `text`: its ends mapped through a diff with the region around. */
function align(needle: string, text: string, at: number): [number, number] {
    const start = Math.max(0, at - Math.ceil(needle.length / 2));
    const region = text.slice(start, at + needle.length + Math.ceil(needle.length / 2));
    const diffs = cleanupSemantic(makeDiff(needle, region, { checkLines: false, timeout: 1 }));
    // The end through its last character: mapping the very end would take in whatever was inserted after it.
    let [from, to] = snap(text, start + xIndex(diffs, 0), start + xIndex(diffs, needle.length - 1) + 1);
    // Whole lines at either edge that make it less like the original belong to the code around it.
    for (let cut = text.indexOf('\n', from); cut >= 0 && cut + 1 < to; cut = text.indexOf('\n', from)) {
        if (similarity(needle, text.slice(cut + 1, to)) < similarity(needle, text.slice(from, to))) {
            break;
        }
        from = cut + 1;
    }
    for (let cut = text.lastIndexOf('\n', to - 1); cut > from; cut = text.lastIndexOf('\n', to - 1)) {
        if (similarity(needle, text.slice(from, cut)) < similarity(needle, text.slice(from, to))) {
            break;
        }
        to = cut;
    }
    return snap(text, from, to);
}

/** A changed range never starts or ends inside a word: an edit that cut into it leaves the whole word in. */
function snap(text: string, from: number, to: number): [number, number] {
    const word = (c: string | undefined) => !!c && /[\p{L}\p{N}_]/u.test(c);
    while (from > 0 && word(text[from - 1]) && word(text[from])) {
        from--;
    }
    while (to < text.length && word(text[to - 1]) && word(text[to])) {
        to++;
    }
    return [from, to];
}

/** 0 to 1: how much of the stored context is still around `from`–`to` (the closest characters count). */
function contextScore(anchor: TrackedRange, text: string, from: number, to: number): number {
    const before = text.slice(Math.max(0, from - anchor.prefix.length), from);
    const after = text.slice(to, to + anchor.suffix.length);
    let p = 0;
    while (p < anchor.prefix.length && p < before.length && anchor.prefix[anchor.prefix.length - 1 - p] === before[before.length - 1 - p]) {
        p++;
    }
    let s = 0;
    while (s < anchor.suffix.length && s < after.length && anchor.suffix[s] === after[s]) {
        s++;
    }
    const total = anchor.prefix.length + anchor.suffix.length;
    return total ? (p + s) / total : 0;
}

/** Character offset of a 0-based line and character in `text`. */
export function offsetAt(text: string, line: number, character: number): number {
    let at = 0;
    for (let l = 0; l < line; l++) {
        const next = text.indexOf('\n', at);
        if (next < 0) {
            return text.length;
        }
        at = next + 1;
    }
    const end = text.indexOf('\n', at);
    return Math.min(at + character, end < 0 ? text.length : end);
}

/** The 0-based line and character of an offset in `text`. */
export function positionAt(text: string, offset: number): { line: number; character: number } {
    const upTo = text.slice(0, offset);
    const line = upTo.split('\n').length - 1;
    return { line, character: offset - (upTo.lastIndexOf('\n') + 1) };
}

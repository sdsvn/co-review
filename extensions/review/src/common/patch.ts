/** Unified-diff parsing and patch metadata (paths and line numbers as in the patch). */

export interface PatchRow {
    t: 'hunk' | 'ctx' | 'add' | 'del';
    o?: number;
    n?: number;
    s: string;
}

export interface PatchFile {
    path: string;
    tag: string;
    added: number;
    removed: number;
    rows: PatchRow[];
}

export interface PatchMeta {
    title?: string;
    number?: number;
    branch?: string;
    base?: string;
    commits?: number;
    /** The GitHub repository of the pull request (`owner/name`), so the review can be posted to it. */
    repo?: string;
    /** The pull request's URL (`https://github.com/<owner>/<name>/pull/<n>`); gives `repo` and `number` too. */
    url?: string;
    body: string;
}

function stripPrefix(p: string): string {
    let cleaned = p.trim();
    if (cleaned.startsWith('"') && cleaned.endsWith('"')) {
        cleaned = cleaned.slice(1, -1);
    }
    for (const pre of ['a/', 'b/', 'i/', 'w/', 'c/', 'o/']) {
        if (cleaned.startsWith(pre)) {
            return cleaned.slice(pre.length);
        }
    }
    return cleaned;
}

function hunkStarts(line: string): [number, number] {
    let body = line;
    const idx = body.slice(2).indexOf('@@');
    if (idx >= 0) {
        body = body.slice(0, idx + 4);
    }
    let oldStart = 1;
    let newStart = 1;
    for (const tok of body.replace(/@/g, '').trim().split(/\s+/)) {
        if (tok.length < 2) {
            continue;
        }
        const val = parseInt(tok.slice(1).split(',')[0], 10);
        if (isNaN(val)) {
            continue;
        }
        if (tok[0] === '-') {
            oldStart = val;
        }
        if (tok[0] === '+') {
            newStart = val;
        }
    }
    return [oldStart, newStart];
}

/** A hunk header's line counts (`@@ -1,5 +1,6 @@`: 5 and 6; a missing count is 1), or undefined if it has none. */
function hunkCounts(line: string): [number, number] | undefined {
    const m = line.match(/^@@+ -\d+(?:,(\d+))? \+\d+(?:,(\d+))? @@/);
    return m ? [m[1] === undefined ? 1 : Number(m[1]), m[2] === undefined ? 1 : Number(m[2])] : undefined;
}

export function parsePatch(src: string): PatchFile[] {
    const files: PatchFile[] = [];
    let cur: PatchFile | undefined;
    let oldNo = 0;
    let newNo = 0;
    // The lines left in the current hunk, per side (its header's counts): inside one, a line is a diff line whatever it
    // starts with (`--- x` is a removed `-- x`); after it, anything up to the next header (a commit message, a diffstat,
    // a `-- ` signature in `format-patch` output) is not. Undefined: a header without counts, read up to the next one.
    let left: [number, number] | undefined = [0, 0];
    const flush = () => {
        if (cur) {
            files.push(cur);
            cur = undefined;
        }
    };
    for (const line of src.split(/\r?\n/)) {
        if (cur && left && (left[0] > 0 || left[1] > 0)) {
            const t = line === '' || line[0] === ' ' ? 'ctx' : line[0] === '+' ? 'add' : line[0] === '-' ? 'del' : undefined;
            if (t === 'ctx' && left[0] > 0 && left[1] > 0) {
                cur.rows.push({ t, o: oldNo++, n: newNo++, s: line.slice(1) });
                left = [left[0] - 1, left[1] - 1];
                continue;
            }
            if (t === 'add' && left[1] > 0) {
                cur.rows.push({ t, n: newNo++, s: line.slice(1) });
                cur.added++;
                left = [left[0], left[1] - 1];
                continue;
            }
            if (t === 'del' && left[0] > 0) {
                cur.rows.push({ t, o: oldNo++, s: line.slice(1) });
                cur.removed++;
                left = [left[0] - 1, left[1]];
                continue;
            }
            if (line.startsWith('\\')) {
                continue;
            }
            // Not what the header said is left (a hand-edited patch): headers are read as headers again.
            left = [0, 0];
        }
        if (line.startsWith('diff --git ')) {
            flush();
            const parts = line.slice('diff --git '.length).split(/\s+/).filter(Boolean);
            cur = { path: stripPrefix(parts.length === 2 ? parts[1] : parts.join(' ')), tag: 'modified', added: 0, removed: 0, rows: [] };
        } else if (!cur && (line.startsWith('--- ') || line.startsWith('+++ '))) {
            if (line.startsWith('+++ ')) {
                cur = { path: stripPrefix(line.slice(4).trim()), tag: 'modified', added: 0, removed: 0, rows: [] };
            }
        } else if (line.startsWith('new file mode')) {
            cur && (cur.tag = 'new file');
        } else if (line.startsWith('deleted file mode')) {
            cur && (cur.tag = 'deleted');
        } else if (line.startsWith('rename from ') || line.startsWith('rename to ')) {
            cur && (cur.tag = 'renamed');
        } else if (line.startsWith('Binary files')) {
            cur && (cur.tag = 'binary');
        } else if (line.startsWith('--- ') || line.startsWith('+++ ')) {
            if (cur && line.startsWith('+++ ')) {
                const p = stripPrefix(line.slice(4).trim());
                if (p && p !== '/dev/null') {
                    cur.path = p;
                }
            }
        } else if (line.startsWith('@@')) {
            if (!cur) {
                continue;
            }
            [oldNo, newNo] = hunkStarts(line);
            left = hunkCounts(line);
            cur.rows.push({ t: 'hunk', s: line });
        } else if (cur && cur.rows.length > 0 && !left) {
            if (line === '') {
                cur.rows.push({ t: 'ctx', o: oldNo++, n: newNo++, s: '' });
            } else if (line[0] === '+') {
                cur.rows.push({ t: 'add', n: newNo++, s: line.slice(1) });
                cur.added++;
            } else if (line[0] === '-') {
                cur.rows.push({ t: 'del', o: oldNo++, s: line.slice(1) });
                cur.removed++;
            } else if (line[0] === ' ') {
                cur.rows.push({ t: 'ctx', o: oldNo++, n: newNo++, s: line.slice(1) });
            }
        }
    }
    flush();
    return files;
}

/** `key: value` lines (optionally inside a `---` fence), a blank line, then Markdown. */
export function parsePatchMeta(text: string): PatchMeta {
    const lines = text.replace(/^---\s*\n/, '').split('\n');
    const meta: PatchMeta = { body: '' };
    let i = 0;
    for (; i < lines.length; i++) {
        const line = lines[i];
        if (!line.trim() || line.trim() === '---') {
            i++;
            break;
        }
        const m = line.match(/^(\w+):\s*(.*)$/);
        if (!m) {
            break;
        }
        const [, key, value] = m;
        if (key === 'title') { meta.title = value; }
        if (key === 'pr' || key === 'number') { meta.number = parseInt(value, 10) || undefined; }
        if (key === 'branch') { meta.branch = value; }
        if (key === 'base') { meta.base = value; }
        if (key === 'commits') { meta.commits = parseInt(value, 10) || undefined; }
        if (key === 'repo') { meta.repo = value.trim(); }
        if (key === 'url') { meta.url = value.trim(); }
    }
    const fromUrl = meta.url?.match(/github\.com\/([\w.-]+\/[\w.-]+)\/pull\/(\d+)/);
    if (fromUrl) {
        meta.repo ??= fromUrl[1];
        meta.number ??= Number(fromUrl[2]);
    }
    meta.body = lines.slice(i).join('\n').trim();
    return meta;
}

/** Document of a review directory, first match wins (see llms.txt). */
export const DOCUMENT_NAMES = ['index.markdown', 'index.md', 'INDEX.md', 'README.md', 'readme.md'];

/**
 * The lines of the new side that a patch changes, per file: added lines, and for removed lines the new-side line
 * where they were (the next line kept or added).
 */
export function changedLines(files: PatchFile[]): Map<string, number[]> {
    const changed = new Map<string, number[]>();
    for (const file of files) {
        const lines = new Set<number>();
        let removed = false;
        for (const row of file.rows) {
            if (row.t === 'del') {
                removed = true;
            } else if (row.t === 'add' && row.n) {
                lines.add(row.n);
                removed = false;
            } else if (row.t === 'ctx' && row.n) {
                if (removed) {
                    lines.add(row.n);
                }
                removed = false;
            } else {
                removed = false;
            }
        }
        if (lines.size) {
            changed.set(file.path, [...lines].sort((a, b) => a - b));
        }
    }
    return changed;
}

/** Lock files and generated files: changed with the change, rarely read in a review. */
const GENERATED = /(^|\/)(package-lock\.json|npm-shrinkwrap\.json|yarn\.lock|pnpm-lock\.yaml|Cargo\.lock|go\.sum|poetry\.lock|Gemfile\.lock|composer\.lock)$|\.gen\.[a-z]+$|(^|\/)(dist|generated|__generated__)\//;

/** A file whose only change is a version number (a release bump): `"version": …`, `version = …`, `"@scope/pkg": "1.2.3"`. */
function versionOnly(file: { rows?: PatchRow[] }): boolean {
    const changed = (file.rows ?? []).filter(r => r.t === 'add' || r.t === 'del');
    return changed.length > 0 && changed.every(r => /^\s*("?version"?\s*[:=]|"[@\w./-]+"\s*:\s*"[\^~]?\d+\.\d+\.\d+[^"]*",?\s*$)/.test(r.s));
}

/** A file of a change a reviewer rarely reads: generated, a lock file, or only a version bump. Listed apart, opened last. */
export function isRoutineFile(file: { path: string; rows?: PatchRow[] }): boolean {
    return GENERATED.test(file.path) || versionOnly(file);
}

/** Whether `query` matches a path: its characters in order (`prw` matches `patch-review-widget`), ignoring case. */
export function matchesPath(query: string, path: string): boolean {
    const q = query.toLowerCase().replace(/\s+/g, '');
    const p = path.toLowerCase();
    let i = 0;
    for (const c of p) {
        if (c === q[i]) {
            i++;
        }
    }
    return i === q.length;
}

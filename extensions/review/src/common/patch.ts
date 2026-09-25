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

export function parsePatch(src: string): PatchFile[] {
    const files: PatchFile[] = [];
    let cur: PatchFile | undefined;
    let oldNo = 0;
    let newNo = 0;
    const flush = () => {
        if (cur) {
            files.push(cur);
            cur = undefined;
        }
    };
    for (const line of src.split(/\r?\n/)) {
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
            cur.rows.push({ t: 'hunk', s: line });
        } else if (cur && cur.rows.length > 0) {
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
    }
    meta.body = lines.slice(i).join('\n').trim();
    return meta;
}

/** Document of a review directory, first match wins (see llms.txt). */
export const DOCUMENT_NAMES = ['index.markdown', 'index.md', 'INDEX.md', 'README.md', 'readme.md'];

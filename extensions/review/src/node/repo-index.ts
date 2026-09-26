import { FileUri } from '@theia/core/lib/common/file-uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { execFile } from 'child_process';
import { promises as fs } from 'fs';
import * as path from 'path';
import { ReviewCoverage } from '../common/review-model';
import { ReviewStore } from './review-store';
import { repositoryOverview } from './repo-overview';
import { SyntaxServiceImpl } from './syntax-service-impl';

export const LANGUAGE_BY_EXTENSION: Record<string, string> = {
    '.go': 'go', '.ts': 'typescript', '.tsx': 'typescriptreact', '.js': 'javascript', '.jsx': 'javascriptreact', '.mjs': 'javascript',
    '.py': 'python', '.java': 'java', '.rs': 'rust', '.cs': 'csharp', '.cpp': 'cpp', '.cc': 'cpp', '.hpp': 'cpp', '.c': 'c', '.h': 'c',
    '.rb': 'ruby', '.php': 'php', '.sh': 'shellscript', '.ps1': 'powershell'
};

const MAX_FILES = 5000;
const MAX_FILE_BYTES = 256 * 1024;
const MAX_CHILDREN = 12;
/** A map younger than this is reused as is; older ones re-check file times and re-parse what changed. */
const FRESH_MS = 30_000;
/** Bumped when the stored map's shape changes; an older map is rebuilt. */
const MAP_VERSION = 1;
const SKIPPED_DIRS = new Set(['node_modules', 'vendor', 'dist', 'out', 'target', '__pycache__']);

/** One source file: its top-level definitions with their members, and the stat it was parsed at. */
interface FileEntry {
    mtimeMs: number;
    size: number;
    symbols: [name: string, members: string[]][];
}

export interface RepoMap {
    version: number;
    builtAt: number;
    files: Record<string, FileEntry>;
}

export interface RenderOptions {
    /** Repository-relative file the question is about; files near it come first. */
    focus?: string;
    /** Only files under this repository-relative path. */
    path?: string;
    /** Only files whose path or symbols contain this (case-insensitive). */
    query?: string;
    maxChars?: number;
}

/**
 * An on-demand outline of the repository — each source file and the classes, functions and methods it defines,
 * from Tree-sitter — so an agent answering a question can go straight to the right files instead of searching.
 * Built the first time it is asked for, kept next to the reviews (outside the repository), and refreshed
 * incrementally: only files whose size or modification time changed are parsed again.
 */
@injectable()
export class RepoIndex {

    @inject(SyntaxServiceImpl) protected readonly syntax: SyntaxServiceImpl;
    @inject(ReviewStore) protected readonly store: ReviewStore;

    /** root -> the latest build; builds of one repository run one after the other. */
    protected readonly maps = new Map<string, Promise<RepoMap>>();

    /** The map of the repository at `root`, refreshed when older than a few seconds (or always, with `refresh`). */
    get(root: string, refresh = false): Promise<RepoMap> {
        const previous = this.maps.get(root) ?? this.load(root);
        const next = previous.catch(() => undefined).then(map =>
            map && !refresh && Date.now() - map.builtAt < FRESH_MS ? map : this.build(root, map));
        this.maps.set(root, next);
        return next;
    }

    /** Starts building the map if there is none yet, so the first question does not wait for it. */
    warm(root: string): void {
        if (!this.maps.has(root)) {
            this.get(root).catch(error => console.error('[co-review] repository map failed', error));
        }
    }

    /**
     * How much of the repository the reviewer has viewed, over its source files: overall and per area (folders;
     * a folder holding most of the repository is split into its subfolders, so `src/…` gives `src/api`, `src/db`, …).
     */
    async coverage(root: string, viewed: Record<string, number> = {}): Promise<ReviewCoverage> {
        const map = await this.get(root);
        const files = Object.entries(map.files);
        // A file changed since it was marked counts as not viewed.
        const seen = (file: string, entry: FileEntry) => (viewed[file] ?? 0) >= entry.mtimeMs;
        // Folders as areas: top-level ones, and any holding over 40% of the files split into their subfolders,
        // so a repository that is mostly `src/` or `extensions/` still gets areas of a useful size.
        const folders = (file: string) => file.split('/').slice(0, -1);
        const areaOf = (file: string, split: Set<string>) => {
            const parts = folders(file);
            let depth = 1;
            while (depth < parts.length && depth < 4 && split.has(parts.slice(0, depth).join('/'))) {
                depth++;
            }
            return parts.slice(0, depth).join('/') || '.';
        };
        const split = new Set<string>();
        for (let changed = true; changed;) {
            changed = false;
            const counts = new Map<string, number>();
            files.forEach(([f]) => counts.set(areaOf(f, split), (counts.get(areaOf(f, split)) ?? 0) + 1));
            for (const [area, count] of counts) {
                const deeper = files.some(([f]) => areaOf(f, split) === area && folders(f).length > area.split('/').length);
                if (area !== '.' && count > Math.max(10, files.length * 0.4) && deeper && area.split('/').length < 4) {
                    split.add(area);
                    changed = true;
                }
            }
        }
        const areas = new Map<string, ReviewCoverage['areas'][number]>();
        for (const [file, entry] of files) {
            const key = areaOf(file, split);
            const area = areas.get(key) ?? { path: key, total: 0, viewed: 0 };
            area.total++;
            if (seen(file, entry)) {
                area.viewed++;
            } else {
                area.next ??= file;
            }
            areas.set(key, area);
        }
        const list = [...areas.values()].sort((a, b) => a.path.localeCompare(b.path));
        return { total: files.length, viewed: list.reduce((n, a) => n + a.viewed, 0), areas: list };
    }

    /** The repository overview page (Markdown): where to start, the areas and how they connect. See repo-overview.ts. */
    async overview(root: string, viewed: Record<string, number> = {}): Promise<string> {
        const map = await this.get(root);
        const outlines = new Map(Object.entries(map.files).map(([file, entry]) => [file, outline(entry)]));
        return repositoryOverview({ root, coverage: await this.coverage(root, viewed), outlines });
    }

    /** Writes the overview next to the workspace's reviews (outside the repository); returns the file's path. */
    async writeOverview(root: string, viewed: Record<string, number> = {}): Promise<string> {
        const file = path.join(this.store.workspaceDir(FileUri.create(root).toString()), 'overview.md');
        await fs.mkdir(path.dirname(file), { recursive: true });
        await fs.writeFile(file, await this.overview(root, viewed));
        return file;
    }

    /** The map as text for an agent: one line per file, `path: Symbol { member, … }, function, …`. */
    async render(root: string, options: RenderOptions = {}): Promise<string> {
        const map = await this.get(root);
        const query = options.query?.toLowerCase();
        const prefix = options.path?.replace(/^\.\/?/, '').replace(/\/$/, '');
        let files = Object.entries(map.files).filter(([file, entry]) =>
            (!prefix || file === prefix || file.startsWith(prefix + '/'))
            && (!query || file.toLowerCase().includes(query) || entry.symbols.flat(2).some(name => name.toLowerCase().includes(query))));
        if (options.focus) {
            const near = (file: string) => commonSegments(file, options.focus!);
            files = files.sort(([a], [b]) => near(b) - near(a) || a.localeCompare(b));
        }
        const maxChars = options.maxChars ?? 20_000;
        const total = Object.keys(map.files).length;
        const lines = [`Repository map: ${total} source files, each with the classes, functions and methods it defines (Tree-sitter outline).`];
        if (await exists(path.join(root, 'graphify-out', 'GRAPH_REPORT.md'))) {
            lines.push('A Graphify knowledge graph of this repository is in graphify-out/ (GRAPH_REPORT.md summarizes it).');
        }
        let size = lines.join('\n').length;
        let shown = 0;
        for (const [file, entry] of files) {
            const text = outline(entry, query);
            const line = text ? `${file}: ${text}` : file;
            if (size + line.length + 1 > maxChars) {
                break;
            }
            lines.push(line);
            size += line.length + 1;
            shown++;
        }
        if (shown < files.length) {
            lines.push(`… and ${files.length - shown} more files (narrow it down with a path or query).`);
        } else if (!files.length) {
            lines.push(query || prefix ? 'No files match.' : 'No source files found.');
        }
        return lines.join('\n');
    }

    protected mapFile(root: string): string {
        return path.join(this.store.workspaceDir(FileUri.create(root).toString()), 'repo-map.json');
    }

    protected async load(root: string): Promise<RepoMap | undefined> {
        try {
            // Loaded from disk it is never fresh: the repository may have changed while Co-Review was closed.
            const map = JSON.parse(await fs.readFile(this.mapFile(root), 'utf8')) as RepoMap;
            return map.version === MAP_VERSION ? { ...map, builtAt: 0 } : undefined;
        } catch {
            return undefined;
        }
    }

    protected async build(root: string, previous: RepoMap | undefined): Promise<RepoMap> {
        const files: Record<string, FileEntry> = {};
        for (const file of (await this.sourceFiles(root)).slice(0, MAX_FILES)) {
            const absolute = path.join(root, file);
            const stat = await fs.stat(absolute).catch(() => undefined);
            if (!stat?.isFile() || stat.size > MAX_FILE_BYTES) {
                continue;
            }
            const old = previous?.files[file];
            if (old && old.mtimeMs === stat.mtimeMs && old.size === stat.size) {
                files[file] = old;
                continue;
            }
            const text = await fs.readFile(absolute, 'utf8').catch(() => undefined);
            const symbols = text === undefined ? [] : await this.syntax.getSymbols(LANGUAGE_BY_EXTENSION[path.extname(file)], text).catch(() => []);
            files[file] = {
                mtimeMs: stat.mtimeMs, size: stat.size,
                symbols: symbols.map(s => [s.name, s.children.filter(c => c.kind !== 'field').map(c => c.name)])
            };
        }
        const map: RepoMap = { version: MAP_VERSION, builtAt: Date.now(), files };
        const target = this.mapFile(root);
        await fs.mkdir(path.dirname(target), { recursive: true });
        await fs.writeFile(target, JSON.stringify(map));
        return map;
    }

    /** Source files, repository-relative: from git (tracked and untracked, not ignored), else a directory walk. */
    protected async sourceFiles(root: string): Promise<string[]> {
        const fromGit = await new Promise<string[] | undefined>(resolve =>
            execFile('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard'], { cwd: root, maxBuffer: 64 * 1024 * 1024 },
                (error, stdout) => resolve(error ? undefined : stdout.split('\0').filter(Boolean))));
        const all = fromGit ?? await this.walk(root, '');
        return [...new Set(all)].filter(file => LANGUAGE_BY_EXTENSION[path.extname(file)]).sort();
    }

    protected async walk(root: string, dir: string, found: string[] = []): Promise<string[]> {
        const entries = await fs.readdir(path.join(root, dir), { withFileTypes: true }).catch(() => []);
        for (const entry of entries) {
            if (found.length >= MAX_FILES || entry.name.startsWith('.')) {
                continue;
            }
            const relative = dir ? `${dir}/${entry.name}` : entry.name;
            if (entry.isDirectory() && !SKIPPED_DIRS.has(entry.name)) {
                await this.walk(root, relative, found);
            } else if (entry.isFile()) {
                found.push(relative);
            }
        }
        return found;
    }
}

/** `Class { method, … }, function`; a long member list is cut short, except for members matching `query`. */
function outline(entry: FileEntry, query?: string): string {
    return entry.symbols.map(([name, members]) => {
        if (!members.length) {
            return name;
        }
        const shown = members.filter((member, i) => i < MAX_CHILDREN || (query && member.toLowerCase().includes(query)));
        return `${name} { ${shown.join(', ')}${shown.length < members.length ? ', …' : ''} }`;
    }).join(', ');
}

function commonSegments(a: string, b: string): number {
    const x = a.split('/');
    const y = b.split('/');
    let i = 0;
    while (i < x.length && i < y.length && x[i] === y[i]) {
        i++;
    }
    return i;
}

function exists(file: string): Promise<boolean> {
    return fs.access(file).then(() => true, () => false);
}

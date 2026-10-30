import { FileUri } from '@theia/core/lib/common/file-uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { execFile } from 'child_process';
import { createHash } from 'crypto';
import { promises as fs } from 'fs';
import * as path from 'path';
import { FileEntry, RepoIndex } from './repo-index';
import { ReviewStore } from './review-store';

/**
 * The repository's facts: what can be worked out about it without a model, so that a guide to the repository
 * (/co-review:understand) starts from the same evidence whichever agent writes it. Entry points and external
 * contracts, each folder's role and how deep the map goes, the load-bearing code, naming rules, repeated shapes, and
 * what the history says. Computed from the repository map (Tree-sitter: definitions, imports, calls), the manifests
 * and plain git; pinned to the commit. The method is docs/proposals/understanding-a-repository.pseudocode.md (its ⚙
 * steps); `computeFacts` is the deterministic part a port reimplements, `RepoFacts` gathers its inputs.
 */

/** Bumped when the shape of the facts changes; cached facts of another version are recomputed. */
export const FACTS_VERSION = 1;
/** Commits read from the history; older ones say little about the code as it is. */
const MAX_COMMITS = 2000;
/** A commit touching more files than this is a move or a reformat: it would fake every coupling. */
const LARGE_COMMIT = 50;
/** Fewer commits than this and the history is too thin to infer from. */
const THIN_HISTORY = 50;
/** The map descends into a folder holding more than this share of the files, or mixing roles, at most MAX_DEPTH deep. */
const SPLIT_SHARE = 0.4;
const MAX_DEPTH = 4;
/** A naming form is a rule when it covers this share of its kind, with at least RULE_MIN instances. */
const RULE_SHARE = 0.7;
const RULE_MIN = 5;
/** A shape counts as repeated from this many instances. */
const REPEATED = 3;

export type FolderRole = 'core' | 'infra' | 'glue' | 'tooling' | 'tests' | 'generated' | 'vendored' | 'unclear';
export type FactsSection = 'entry' | 'folders' | 'load' | 'names' | 'shapes' | 'history';
export const FACTS_SECTIONS: FactsSection[] = ['entry', 'folders', 'load', 'names', 'shapes', 'history'];

export interface Commit {
    sha: string;
    date: string;
    author: string;
    subject: string;
    files: { path: string; changed: number }[];
    /** Files the commit created. */
    added?: string[];
}

export interface FactsInput {
    files: Record<string, FileEntry>;
    /** Whether the repository map stopped at its file limit. */
    truncated?: boolean;
    head: { sha: string; date: string; dirty: boolean };
    commits: Commit[];
    totalCommits: number;
    /** Repository-relative path → text, for package.json, Makefile, go.mod, Cargo.toml, pyproject.toml. */
    manifests: Record<string, string>;
    /** Only this repository-relative folder. */
    focus?: string;
}

export interface Facts {
    version: number;
    commit: { sha: string; date: string; dirty: boolean };
    files: number;
    truncated: boolean;
    focus?: string;
    /** How dependencies were found; a role from imports alone is a weaker guess than one from a call graph. */
    dependencies: 'imports';
    entry: {
        points: { kind: 'bin' | 'script' | 'make' | 'main' | 'registration'; name: string; path: string; line?: number; via?: string }[];
        outbound: { target: string; via: string; files: string[] }[];
        env: { name: string; files: string[] }[];
        external: { module: string; files: number }[];
    };
    folders: {
        map: { path: string; files: number; lines: number; role: FolderRole; why: string; confidence: 'high' | 'low'; keyFiles: string[]; dependsOn: string[]; usedBy: string[] }[];
        ignore: string[];
    };
    load: { path: string; importers: number; rank: number }[];
    names: {
        casing: { kind: string; form: string; share: number; count: number; deviations: string[] }[];
        folderRules: { folder: string; kind: 'file' | 'type'; position: 'prefix' | 'suffix'; word: string; share: number; count: number; examples: string[] }[];
        filePatterns: { pattern: string; count: number; examples: string[] }[];
        verbs: { verb: string; count: number; examples: string[] }[];
        suffixes: { suffix: string; count: number; examples: string[] }[];
        vocabulary: { term: string; folders: number; types: string[] }[];
        lookalikes: { stem: string; names: { name: string; path: string }[] }[];
    };
    shapes: {
        siblingSets: { parent: string; files: string[]; folders: string[] }[];
        registrations: { via: string; count: number; files: number; examples: { name: string; path: string; line: number }[] }[];
    };
    history: {
        thin: boolean;
        commits: number;
        read: number;
        ignoredLarge: number;
        churn: { path: string; commits: number; changed: number }[];
        hotspots: { path: string; commits: number; lines: number; score: number }[];
        coupling: { a: string; b: string; together: number; degree: number; crossFolder: boolean }[];
        authors: { folder: string; authors: { name: string; commits: number }[] }[];
        recipes: { adds: string; alsoTouched: string[]; times: number; commits: { sha: string; subject: string; added: string[] }[] }[];
    };
}

const TYPE_KINDS = new Set(['class', 'interface', 'type', 'struct', 'enum']);
const TEST_FILE = /(\.|_)(test|spec)\.[a-z]+$|^test_|Test\.java$|_test\.go$/;
const TEST_DIRS = new Set(['test', 'tests', '__tests__', 'spec', 'specs', 'e2e', 'testdata', 'fixtures', '__mocks__']);
const TOOLING_DIRS = new Set(['scripts', 'script', 'bin', 'tools', 'tooling', 'ci', '.github', 'hack', 'devtools', '.circleci']);
const VENDORED_DIRS = new Set(['vendor', 'third_party', 'third-party', 'external', 'vendored']);
const GENERATED_NAME = /\.gen\.|_gen\.|\.pb\.|\.generated\.|_generated\./;
const SOURCE_EXTENSIONS = ['.ts', '.tsx', '.js', '.jsx', '.mjs', '.cjs'];
/** Words that say nothing about the domain: they are the language's or every program's. */
const STOPWORDS = new Set(('get set new create update delete add remove is has can to from with for of the and or not on in by at as '
    + 'data info item items list map value values type types string number int bool boolean error errors err result results '
    + 'options option opts config util utils helper helpers impl base default index main test tests spec mock mocks src lib '
    + 'node common browser app api id ids name names key keys file files path paths dir all one two first last make run '
    + 'init start stop handle handler handlers props state context ctx args arg params param request response req res').split(' '));

/** `UserStoreImpl` → user, store, impl; `user_store` → user, store; `HTTPServer` → http, server. */
export function words(name: string): string[] {
    return name.replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/([A-Z]+)([A-Z][a-z])/g, '$1 $2')
        .split(/[\s_\-.$]+/).filter(Boolean).map(w => w.toLowerCase());
}

/** The casing of a name with at least two words; a single lowercase word fits several casings and says nothing. */
function casing(name: string): string | undefined {
    if (/^[A-Z][A-Z0-9]*(_[A-Z0-9]+)+$/.test(name)) {
        return 'UPPER_SNAKE';
    }
    if (/^[A-Z][a-zA-Z0-9]*$/.test(name)) {
        return /[a-z]/.test(name) ? 'PascalCase' : undefined;
    }
    if (/^[a-z][a-z0-9]*([A-Z][a-z0-9]*)+$/.test(name)) {
        return 'camelCase';
    }
    if (/^[a-z][a-z0-9]*(_[a-z0-9]+)+$/.test(name)) {
        return 'snake_case';
    }
    if (/^[a-z][a-z0-9]*(-[a-z0-9]+)+$/.test(name)) {
        return 'kebab-case';
    }
    return undefined;
}

function dirOf(file: string): string {
    const i = file.lastIndexOf('/');
    return i < 0 ? '.' : file.slice(0, i);
}

function under(file: string, folder: string): boolean {
    return folder === '.' ? true : file === folder || file.startsWith(folder + '/');
}

/** `src/a/user-store.test.ts` → `user-store`. */
function stem(file: string): string {
    return path.basename(file).replace(/\..*$/, '').replace(/_test$/, '');
}

function top<T>(list: T[], n: number): T[] {
    return list.slice(0, n);
}

function share(n: number, total: number): number {
    return Math.round((n / total) * 100) / 100;
}

/** Counts of `key` over `items`, most frequent first, with a few examples each. */
function tally<T>(items: T[], key: (item: T) => string | undefined, example: (item: T) => string): { key: string; count: number; examples: string[] }[] {
    const counts = new Map<string, { count: number; examples: string[] }>();
    for (const item of items) {
        const k = key(item);
        if (!k) {
            continue;
        }
        const entry = counts.get(k) ?? { count: 0, examples: [] };
        entry.count++;
        if (entry.examples.length < 3) {
            entry.examples.push(example(item));
        }
        counts.set(k, entry);
    }
    return [...counts.entries()].map(([k, v]) => ({ key: k, ...v })).sort((a, b) => b.count - a.count || a.key.localeCompare(b.key));
}

export function computeFacts(input: FactsInput): Facts {
    const focus = input.focus?.replace(/^\.\/?/, '').replace(/\/$/, '') || undefined;
    const files = Object.fromEntries(Object.entries(input.files).filter(([f]) => !focus || under(f, focus)));
    const paths = Object.keys(files).sort();
    const goModule = input.manifests['go.mod']?.match(/^module\s+(\S+)/m)?.[1];

    // ---- dependencies: imports resolved to files in the repository (Go: to the package's folder) ----
    const byStem = new Map<string, string>();
    const pyModules = new Map<string, string>();
    const javaClasses = new Map<string, string>();
    for (const file of Object.keys(input.files)) {
        byStem.set(file.replace(/\.[^./]+$/, ''), file);
        if (file.endsWith('.py')) {
            pyModules.set(file.replace(/\.py$/, '').replace(/\/__init__$/, '').replace(/\//g, '.'), file);
        }
        if (file.endsWith('.java')) {
            javaClasses.set(file.replace(/\.java$/, ''), file);
        }
    }
    const resolve = (from: string, spec: string): { target?: string; external?: string } => {
        const ext = path.extname(from);
        if (SOURCE_EXTENSIONS.includes(ext)) {
            if (!spec.startsWith('.')) {
                return { external: spec.startsWith('@') ? spec.split('/').slice(0, 2).join('/') : spec.split('/')[0] };
            }
            const base = path.posix.normalize(path.posix.join(dirOf(from), spec)).replace(/\.(js|mjs|cjs|jsx|ts|tsx)$/, '');
            return { target: byStem.get(base) ?? byStem.get(base + '/index') };
        }
        if (ext === '.go') {
            if (goModule && (spec === goModule || spec.startsWith(goModule + '/'))) {
                return { target: spec.slice(goModule.length + 1) || '.' };
            }
            return { external: spec.split('/').slice(0, 3).join('/') };
        }
        if (ext === '.py') {
            if (spec.startsWith('.')) {
                const ups = spec.match(/^\.+/)![0].length;
                let dir = dirOf(from);
                for (let i = 1; i < ups; i++) {
                    dir = dirOf(dir);
                }
                const rest = spec.slice(ups);
                const module = (dir === '.' ? '' : dir.replace(/\//g, '.')) + (rest ? (dir === '.' ? '' : '.') + rest : '');
                return { target: pyModules.get(module) };
            }
            for (const [module, file] of pyModules) {
                if (module === spec || module.endsWith('.' + spec)) {
                    return { target: file };
                }
            }
            return { external: spec.split('.')[0] };
        }
        if (ext === '.rs') {
            const crateRoot = from.includes('/src/') ? from.slice(0, from.indexOf('/src/') + 4) : from.startsWith('src/') ? 'src' : dirOf(from);
            let parts = spec.split('::');
            let base: string;
            if (parts[0] === 'crate') {
                base = crateRoot;
            } else if (parts[0] === 'super' || parts[0] === 'self') {
                base = parts[0] === 'super' ? dirOf(dirOf(from)) : dirOf(from);
            } else {
                return { external: parts[0] };
            }
            parts = parts.slice(1);
            while (parts.length) {
                const candidate = `${base}/${parts.join('/')}`;
                const found = byStem.get(candidate) ?? byStem.get(candidate + '/mod');
                if (found) {
                    return { target: found };
                }
                parts = parts.slice(0, -1);
            }
            return {};
        }
        if (ext === '.java') {
            const suffix = spec.replace(/\./g, '/');
            for (const [stemPath, file] of javaClasses) {
                if (stemPath === suffix || stemPath.endsWith('/' + suffix)) {
                    return { target: file };
                }
            }
            return { external: spec.split('.').slice(0, 2).join('.') };
        }
        return {};
    };
    /** file → files (or Go package folders) it imports, inside the repository. */
    const edges = new Map<string, Set<string>>();
    const externals = new Map<string, Set<string>>();
    for (const file of paths) {
        const targets = new Set<string>();
        for (const spec of files[file].imports ?? []) {
            const { target, external } = resolve(file, spec);
            if (target && target !== file) {
                targets.add(target);
            } else if (external) {
                externals.set(external, (externals.get(external) ?? new Set()).add(file));
            }
        }
        edges.set(file, targets);
    }
    /** A target is a file, or a Go package folder: the files it stands for. */
    const packages = new Map<string, string[]>();
    const filesOf = (target: string): string[] => {
        if (files[target]) {
            return [target];
        }
        let found = packages.get(target);
        if (!found) {
            found = paths.filter(f => dirOf(f) === target && f.endsWith('.go'));
            packages.set(target, found);
        }
        return found;
    };
    const importers = new Map<string, Set<string>>();
    for (const [file, targets] of edges) {
        for (const target of targets) {
            importers.set(target, (importers.get(target) ?? new Set()).add(file));
        }
    }

    // ---- load-bearing: importers, then a PageRank over the import graph ----
    const nodes = [...new Set([...paths, ...importers.keys()])];
    let rank = new Map(nodes.map(n => [n, 1 / nodes.length]));
    for (let i = 0; i < 20; i++) {
        const next = new Map(nodes.map(n => [n, 0.15 / nodes.length]));
        for (const [file, targets] of edges) {
            const out = [...targets];
            for (const target of out) {
                next.set(target, (next.get(target) ?? 0) + 0.85 * (rank.get(file) ?? 0) / out.length);
            }
        }
        rank = next;
    }
    const load = nodes.filter(n => (importers.get(n)?.size ?? 0) > 0)
        .map(n => ({ path: n, importers: importers.get(n)!.size, rank: Math.round((rank.get(n) ?? 0) * nodes.length * 100) / 100 }))
        .sort((a, b) => b.rank - a.rank || b.importers - a.importers).slice(0, 25);

    // ---- folders: a role for each, and how deep the map goes ----
    const filesUnder = (folder: string) => paths.filter(f => folder === '.' ? !f.includes('/') : under(f, folder));
    const roles = new Map<string, { role: FolderRole; why: string; confidence: 'high' | 'low' }>();
    const roleOf = (folder: string) => {
        let role = roles.get(folder);
        if (!role) {
            role = classify(folder);
            roles.set(folder, role);
        }
        return role;
    };
    const classify = (folder: string): { role: FolderRole; why: string; confidence: 'high' | 'low' } => {
        const inside = filesUnder(folder);
        const segments = folder === '.' ? [] : folder.split('/');
        const generated = inside.filter(f => files[f].generated || GENERATED_NAME.test(f)).length;
        if (inside.length && generated / inside.length >= 0.8) {
            return { role: 'generated', why: `${generated} of ${inside.length} files say they are generated or are named like it`, confidence: 'high' };
        }
        const vendored = segments.find(s => VENDORED_DIRS.has(s));
        if (vendored) {
            return { role: 'vendored', why: `under ${vendored}/`, confidence: 'high' };
        }
        const testDir = segments.find(s => TEST_DIRS.has(s));
        const tests = inside.filter(f => TEST_FILE.test(path.basename(f))).length;
        if (testDir || (inside.length && tests / inside.length >= 0.8)) {
            return { role: 'tests', why: testDir ? `under ${testDir}/` : `${tests} of ${inside.length} files are tests`, confidence: 'high' };
        }
        const tooling = segments.find(s => TOOLING_DIRS.has(s));
        if (tooling) {
            return { role: 'tooling', why: `under ${tooling}/`, confidence: 'high' };
        }
        const mine = new Set(inside);
        const isMine = (target: string) => filesOf(target).some(f => mine.has(f));
        const usedBy = new Set<string>();
        const uses = new Set<string>();
        for (const f of inside) {
            edges.get(f)?.forEach(t => !isMine(t) && uses.add(t));
        }
        for (const [file, targets] of edges) {
            if (!mine.has(file) && [...targets].some(isMine)) {
                usedBy.add(file);
            }
        }
        let internal = 0;
        for (const f of inside) {
            edges.get(f)?.forEach(t => isMine(t) && internal++);
        }
        const defs = inside.flatMap(f => files[f].kinds ?? []);
        const typeShare = defs.length ? defs.filter(k => TYPE_KINDS.has(k)).length / defs.length : 0;
        const total = usedBy.size + uses.size;
        if (inside.length >= 5 && internal >= 2 * total) {
            return { role: 'core', why: `${internal} of its imports stay inside it, ${total} cross its edge: it does its own work`, confidence: 'low' };
        }
        if (!total) {
            return { role: 'unclear', why: 'no imports in or out were found', confidence: 'low' };
        }
        const inward = usedBy.size / total;
        const counts = `used by ${usedBy.size} files outside, uses ${uses.size}`;
        if (inward >= 0.6) {
            return typeShare >= 0.4
                ? { role: 'core', why: `${counts}; ${Math.round(typeShare * 100)}% of its definitions are types`, confidence: 'low' }
                : { role: 'infra', why: `${counts}; mostly functions`, confidence: 'low' };
        }
        if (inward <= 0.3) {
            return { role: 'glue', why: counts, confidence: 'low' };
        }
        return { role: 'unclear', why: counts, confidence: 'low' };
    };
    const childFolders = (folder: string) => [...new Set((folder === '.' ? paths : filesUnder(folder)).map(f => {
        const rest = folder === '.' ? f : f.slice(folder.length + 1);
        const i = rest.indexOf('/');
        return i < 0 ? undefined : (folder === '.' ? rest.slice(0, i) : `${folder}/${rest.slice(0, i)}`);
    }).filter((f): f is string => !!f))].sort();
    const mapped: string[] = [];
    /** Folders the map went into that also hold files of their own: their note covers those files only. */
    const shallow = new Set<string>();
    const descend = (folder: string, depth: number) => {
        const children = childFolders(folder);
        const direct = paths.filter(f => dirOf(f) === folder);
        const kinds = new Set(children.map(c => roleOf(c).role));
        if (direct.length) {
            kinds.add(roleOf(folder).role);
        }
        const big = filesUnder(folder).length > SPLIT_SHARE * paths.length;
        if (folder === focus || (children.length && depth < MAX_DEPTH && (big || kinds.size > 1))) {
            if (direct.length && folder !== '.') {
                mapped.push(folder);
                shallow.add(folder);
            }
            children.forEach(c => descend(c, depth + 1));
        } else {
            mapped.push(folder);
        }
    };
    const start = focus ?? '.';
    if (paths.some(f => dirOf(f) === '.' && start === '.')) {
        mapped.push('.');
    }
    childFolders(start).forEach(c => descend(c, start === '.' ? 1 : start.split('/').length + 1));
    if (focus && !mapped.length) {
        mapped.push(focus);
    }
    const mappedOf = (file: string) => mapped.filter(m => m === '.' || shallow.has(m) ? dirOf(file) === m : under(file, m))
        .sort((a, b) => b.length - a.length)[0] ?? '.';
    const folderNotes = [...new Set(mapped)].sort().map(folder => {
        const inside = folder === '.' || shallow.has(folder) ? paths.filter(f => dirOf(f) === folder) : filesUnder(folder);
        const role = roleOf(folder);
        const dependsOn = new Set<string>();
        const usedBy = new Set<string>();
        for (const f of inside) {
            edges.get(f)?.forEach(t => filesOf(t).forEach(tf => mappedOf(tf) !== folder && dependsOn.add(mappedOf(tf))));
            for (const t of [f, dirOf(f)]) {
                importers.get(t)?.forEach(i => mappedOf(i) !== folder && usedBy.add(mappedOf(i)));
            }
        }
        const keyFiles = [...inside].sort((a, b) => (importers.get(b)?.size ?? 0) - (importers.get(a)?.size ?? 0) || (files[b].lines ?? 0) - (files[a].lines ?? 0)).slice(0, 3);
        return {
            path: folder, files: inside.length, lines: inside.reduce((n, f) => n + (files[f].lines ?? 0), 0), ...role,
            keyFiles, dependsOn: [...dependsOn].sort(), usedBy: [...usedBy].sort()
        };
    });

    // ---- entry points and external contracts ----
    const points: Facts['entry']['points'] = [];
    for (const [manifest, text] of Object.entries(input.manifests)) {
        if (focus && !under(manifest, focus) && dirOf(manifest) !== '.') {
            continue;
        }
        const base = path.basename(manifest);
        if (base === 'package.json') {
            try {
                const pkg = JSON.parse(text) as { bin?: string | Record<string, string>; scripts?: Record<string, string>; main?: string; name?: string };
                const bins = typeof pkg.bin === 'string' ? { [pkg.name ?? 'bin']: pkg.bin } : pkg.bin ?? {};
                Object.entries(bins).forEach(([name, file]) => points.push({ kind: 'bin', name, path: path.posix.join(dirOf(manifest), file), via: manifest }));
                Object.entries(pkg.scripts ?? {}).forEach(([name, command]) => points.push({ kind: 'script', name, path: manifest, via: command }));
                pkg.main && points.push({ kind: 'main', name: pkg.name ?? pkg.main, path: path.posix.join(dirOf(manifest), pkg.main), via: manifest });
            } catch {
                // Not JSON: nothing to read from it.
            }
        } else if (base === 'Makefile') {
            const phony = new Set((text.match(/^\.PHONY:(.*)$/gm) ?? []).flatMap(l => l.replace('.PHONY:', '').trim().split(/\s+/)));
            for (const match of text.matchAll(/^([A-Za-z0-9][\w.-]*)\s*:(?!=)/gm)) {
                if (!match[1].includes('%') && (phony.size === 0 || phony.has(match[1]))) {
                    points.push({ kind: 'make', name: match[1], path: manifest });
                }
            }
        } else if (base === 'Cargo.toml') {
            for (const match of text.matchAll(/\[\[bin\]\][^[]*?name\s*=\s*"([^"]+)"/g)) {
                points.push({ kind: 'bin', name: match[1], path: manifest });
            }
        } else if (base === 'pyproject.toml') {
            const scripts = text.match(/\[project\.scripts\]([^[]*)/)?.[1] ?? '';
            for (const match of scripts.matchAll(/^\s*([\w.-]+)\s*=\s*"([^"]+)"/gm)) {
                points.push({ kind: 'bin', name: match[1], path: manifest, via: match[2] });
            }
        }
    }
    const isTest = (file: string) => TEST_FILE.test(path.basename(file)) || file.split('/').some(s => TEST_DIRS.has(s));
    for (const file of paths.filter(f => !isTest(f))) {
        const entry = files[file];
        if (entry.symbols.some(([name]) => name === 'main' || name === 'Main') || /^(__main__|server|cli)\.[a-z]+$/.test(path.basename(file))) {
            points.push({ kind: 'main', name: file, path: file });
        }
        for (const [kind, via, arg, line] of entry.calls ?? []) {
            if (kind === 'reg') {
                points.push({ kind: 'registration', name: arg, path: file, line: line + 1, via });
            }
        }
    }
    const outbound = tally(paths.filter(f => !isTest(f)).flatMap(f => (files[f].calls ?? []).filter(c => c[0] === 'out').map(c => ({ f, via: c[1], arg: c[2] }))),
        c => `${c.arg && /^[\w.-]+$/.test(c.arg) ? c.arg : c.via.split('.').pop()}\u0000${c.via}`, c => c.f);
    const env = tally(paths.flatMap(f => (files[f].env ?? []).map(name => ({ f, name }))), e => e.name, e => e.f);

    // ---- names ----
    type Named = { name: string; file: string };
    const types: Named[] = [];
    const functions: Named[] = [];
    const methods: Named[] = [];
    for (const file of paths) {
        const entry = files[file];
        entry.symbols.forEach(([name, members], i) => {
            const kind = entry.kinds?.[i];
            const short = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1) : name;
            if (kind && TYPE_KINDS.has(kind)) {
                types.push({ name, file });
            } else if (kind === 'method') {
                methods.push({ name: short, file });
            } else if (kind === 'function') {
                functions.push({ name, file });
            }
            members.forEach(member => methods.push({ name: member, file }));
        });
    }
    const folderNames = [...new Set(paths.flatMap(f => dirOf(f) === '.' ? [] : dirOf(f).split('/')))]
        .map(name => ({ name, file: name }));
    const fileNames = paths.map(file => ({ name: stem(file), file }));
    const casingRules: Facts['names']['casing'] = [];
    for (const [kind, named] of [['folder', folderNames], ['file', fileNames], ['type', types], ['function', functions], ['method', methods]] as [string, Named[]][]) {
        const forms = tally(named, n => casing(n.name), n => n.name);
        const counted = forms.reduce((n, f) => n + f.count, 0);
        if (forms.length && counted >= RULE_MIN && forms[0].count / counted >= RULE_SHARE) {
            casingRules.push({
                kind, form: forms[0].key, share: share(forms[0].count, counted), count: counted,
                deviations: top(named.filter(n => { const c = casing(n.name); return c && c !== forms[0].key; }).map(n => kind === 'folder' ? n.name : `${n.name} (${n.file})`), 8)
            });
        }
    }
    const folderRules: Facts['names']['folderRules'] = [];
    for (const folder of new Set(paths.map(dirOf))) {
        const direct = paths.filter(f => dirOf(f) === folder);
        for (const [kind, named] of [['file', direct.map(f => ({ name: stem(f), file: f }))], ['type', types.filter(t => dirOf(t.file) === folder)]] as ['file' | 'type', Named[]][]) {
            const multi = named.filter(n => words(n.name).length > 1);
            if (multi.length < RULE_MIN) {
                continue;
            }
            for (const position of ['prefix', 'suffix'] as const) {
                const forms = tally(multi, n => { const w = words(n.name); return position === 'prefix' ? w[0] : w[w.length - 1]; }, n => n.name);
                if (forms[0] && forms[0].count / multi.length >= RULE_SHARE) {
                    folderRules.push({ folder, kind, position, word: forms[0].key, share: share(forms[0].count, multi.length), count: multi.length, examples: forms[0].examples });
                }
            }
        }
    }
    const filePatterns = tally(paths, f => path.basename(f).split('.').length > 2 ? '*.' + path.basename(f).split('.').slice(1).join('.') : undefined, f => f)
        .filter(p => p.count >= RULE_MIN).map(p => ({ pattern: p.key, count: p.count, examples: p.examples }));
    const verbs = tally([...functions, ...methods].filter(n => words(n.name).length > 1), n => words(n.name)[0], n => `${n.name} (${n.file})`)
        .filter(v => v.count >= REPEATED).slice(0, 20).map(v => ({ verb: v.key, count: v.count, examples: v.examples }));
    const suffixes = tally(types.filter(t => words(t.name).length > 1), t => words(t.name).pop(), t => `${t.name} (${t.file})`)
        .filter(s => s.count >= REPEATED).slice(0, 20).map(s => ({ suffix: s.key, count: s.count, examples: s.examples }));
    const termDirs = new Map<string, Set<string>>();
    const termTypes = new Map<string, string[]>();
    for (const t of types) {
        for (const w of new Set(words(t.name))) {
            if (w.length >= 3 && !STOPWORDS.has(w)) {
                termTypes.set(w, [...(termTypes.get(w) ?? []), t.name]);
            }
        }
    }
    for (const file of paths) {
        const named = [stem(file), ...files[file].symbols.map(([name]) => name)];
        for (const w of new Set(named.flatMap(words))) {
            if (termTypes.has(w)) {
                termDirs.set(w, (termDirs.get(w) ?? new Set()).add(dirOf(file)));
            }
        }
    }
    const vocabulary = [...termTypes.entries()].filter(([w]) => (termDirs.get(w)?.size ?? 0) >= REPEATED)
        .map(([term, named]) => ({ term, folders: termDirs.get(term)!.size, types: top([...new Set(named)], 4) }))
        .sort((a, b) => b.folders - a.folders || a.term.localeCompare(b.term)).slice(0, 40);
    const stems = new Map<string, Named[]>();
    for (const t of types) {
        const w = words(t.name);
        if (w.length > 1) {
            const key = t.name.slice(0, t.name.length - (w[w.length - 1].length));
            stems.set(key, [...(stems.get(key) ?? []), t]);
        }
    }
    const lookalikes = [...stems.entries()]
        .filter(([, named]) => new Set(named.map(n => words(n.name).pop())).size > 1)
        .map(([stemName, named]) => ({ stem: stemName, names: top(named, 8).map(n => ({ name: n.name, path: n.file })) }))
        .sort((a, b) => b.names.length - a.names.length || a.stem.localeCompare(b.stem)).slice(0, 20);

    // ---- repeated shapes ----
    const siblingSets: Facts['shapes']['siblingSets'] = [];
    for (const parent of new Set(paths.map(f => dirOf(dirOf(f))))) {
        const children = [...new Set(paths.filter(f => dirOf(dirOf(f)) === parent && dirOf(f) !== parent).map(dirOf))];
        if (children.length < REPEATED) {
            continue;
        }
        const counts = tally(children.flatMap(c => paths.filter(f => dirOf(f) === c).map(f => ({ c, base: path.basename(f) }))), x => x.base, x => x.c);
        const members = counts.filter(c => c.count >= REPEATED && c.count >= children.length / 2).map(c => c.key);
        if (members.length) {
            siblingSets.push({ parent, files: members, folders: children.filter(c => members.some(m => files[`${c}/${m}`])).sort() });
        }
    }
    const regs = paths.filter(f => !isTest(f)).flatMap(f => (files[f].calls ?? []).filter(c => c[0] === 'reg').map(c => ({ f, via: c[1], name: c[2], line: c[3] + 1 })));
    const registrations = tally(regs, r => r.via, r => r.f).filter(r => r.count >= REPEATED).map(r => {
        const all = regs.filter(x => x.via === r.key);
        return { via: r.key, count: r.count, files: new Set(all.map(x => x.f)).size, examples: top(all, 5).map(x => ({ name: x.name, path: x.f, line: x.line })) };
    });

    // ---- history ----
    const inScope = (file: string) => !focus || under(file, focus);
    const commits = input.commits.map(c => ({ ...c, files: c.files.filter(f => inScope(f.path)) })).filter(c => c.files.length);
    const small = commits.filter(c => c.files.length <= LARGE_COMMIT);
    const perFile = new Map<string, { commits: number; changed: number }>();
    for (const c of small) {
        for (const f of c.files) {
            const entry = perFile.get(f.path) ?? { commits: 0, changed: 0 };
            entry.commits++;
            entry.changed += f.changed;
            perFile.set(f.path, entry);
        }
    }
    const current = [...perFile.entries()].filter(([f]) => files[f]);
    const churn = current.map(([p, v]) => ({ path: p, ...v })).sort((a, b) => b.commits - a.commits || b.changed - a.changed).slice(0, 25);
    const maxCommits = Math.max(1, ...current.map(([, v]) => v.commits));
    const maxLines = Math.max(1, ...current.map(([f]) => files[f].lines ?? 0));
    const hotspots = current.map(([p, v]) => ({ path: p, commits: v.commits, lines: files[p].lines ?? 0, score: Math.round((v.commits / maxCommits) * ((files[p].lines ?? 0) / maxLines) * 100) / 100 }))
        .filter(h => h.score > 0).sort((a, b) => b.score - a.score).slice(0, 15);
    const pairs = new Map<string, number>();
    for (const c of small) {
        const touched = [...new Set(c.files.map(f => f.path).filter(f => files[f]))].sort();
        for (let i = 0; i < touched.length; i++) {
            for (let j = i + 1; j < touched.length; j++) {
                const key = `${touched[i]}\u0000${touched[j]}`;
                pairs.set(key, (pairs.get(key) ?? 0) + 1);
            }
        }
    }
    const coupling = [...pairs.entries()].map(([key, together]) => {
        const [a, b] = key.split('\u0000');
        const degree = together / Math.min(perFile.get(a)!.commits, perFile.get(b)!.commits);
        return { a, b, together, degree: Math.round(degree * 100) / 100, crossFolder: mappedOf(a) !== mappedOf(b) };
    }).filter(p => p.together >= REPEATED && p.degree >= 0.6)
        .sort((x, y) => Number(y.crossFolder) - Number(x.crossFolder) || y.together - x.together).slice(0, 20);
    const authors = folderNotes.map(note => {
        const counts = tally(small.filter(c => c.files.some(f => mappedOf(f.path) === note.path && files[f.path])), c => c.author, c => c.sha);
        return { folder: note.path, authors: top(counts, 3).map(a => ({ name: a.key, commits: a.count })) };
    }).filter(a => a.authors.length);
    // A recipe: commits that each created a file of the same kind in the same folder (a new command, a new test), and
    // what those commits also had to touch, the files changed in at least half of them.
    const creations = new Map<string, Commit[]>();
    for (const c of small) {
        for (const kind of new Set((c.added ?? []).filter(inScope).map(f => `${dirOf(f)}/*${path.extname(f)}`))) {
            creations.set(kind, [...(creations.get(kind) ?? []), c]);
        }
    }
    const recipes: Facts['history']['recipes'] = [];
    for (const [adds, made] of creations) {
        if (made.length < 2) {
            continue;
        }
        const dir = adds.slice(0, adds.lastIndexOf('/*'));
        const ext = adds.slice(adds.lastIndexOf('/*') + 2);
        const isNew = (c: Commit, f: string) => dirOf(f) === dir && path.extname(f) === ext && (c.added ?? []).includes(f);
        const touched = new Map<string, number>();
        for (const c of made) {
            for (const f of new Set(c.files.map(x => x.path))) {
                if (!isNew(c, f)) {
                    touched.set(f, (touched.get(f) ?? 0) + 1);
                }
            }
        }
        const alsoTouched = [...touched.entries()].filter(([, n]) => n >= Math.max(2, made.length / 2))
            .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).map(([f]) => f);
        if (alsoTouched.length) {
            recipes.push({
                adds, alsoTouched: alsoTouched.slice(0, 15), times: made.length,
                commits: made.slice(0, 5).map(c => ({ sha: c.sha.slice(0, 7), subject: c.subject, added: (c.added ?? []).filter(f => isNew(c, f)).slice(0, 4) }))
            });
        }
    }
    recipes.sort((a, b) => b.times - a.times || b.alsoTouched.length - a.alsoTouched.length);

    return {
        version: FACTS_VERSION,
        commit: input.head,
        files: paths.length,
        truncated: !!input.truncated,
        focus,
        dependencies: 'imports',
        entry: {
            points,
            outbound: outbound.map(o => ({ target: o.key.split('\u0000')[0], via: o.key.split('\u0000')[1], files: [...new Set(o.examples)] })),
            env: env.map(e => ({ name: e.key, files: [...new Set(e.examples)] })),
            external: [...externals.entries()].map(([module, by]) => ({ module, files: by.size })).sort((a, b) => b.files - a.files).slice(0, 30)
        },
        folders: {
            map: folderNotes,
            ignore: folderNotes.filter(n => n.role === 'generated' || n.role === 'vendored').map(n => n.path)
        },
        load,
        names: { casing: casingRules, folderRules, filePatterns, verbs, suffixes, vocabulary, lookalikes },
        shapes: { siblingSets, registrations },
        history: {
            thin: input.totalCommits < THIN_HISTORY,
            commits: input.totalCommits,
            read: commits.length,
            ignoredLarge: commits.length - small.length,
            churn, hotspots, coupling, authors,
            recipes: recipes.slice(0, 10)
        }
    };
}

/**
 * Parses `git log --numstat --format=%x1e%H%x1f%aI%x1f%an%x1f%s`, and the files each commit created from
 * `git log --diff-filter=A --name-only --format=%x1e%H`.
 */
export function parseLog(text: string, created = ''): Commit[] {
    const added = new Map(created.split('\x1e').filter(r => r.trim()).map(record => {
        const [sha, ...lines] = record.split('\n');
        return [sha.trim(), lines.filter(Boolean)] as const;
    }));
    return text.split('\x1e').filter(r => r.trim()).map(record => {
        const [header, ...lines] = record.split('\n');
        const [sha, date, author, subject] = header.split('\x1f');
        const files = lines.filter(l => l.includes('\t')).map(line => {
            const [added, removed, ...rest] = line.split('\t');
            // A rename is written `a => b` or `src/{a => b}/x`: keep the new path.
            const file = rest.join('\t').replace(/\{[^}]*? => ([^}]*)\}/g, '$1').replace(/^.* => /, '').replace(/\/\/+/g, '/');
            return { path: file, changed: (Number(added) || 0) + (Number(removed) || 0) };
        });
        return { sha, date, author, subject, files, added: added.get(sha) ?? [] };
    });
}

const MANIFESTS = ['package.json', 'Makefile', 'go.mod', 'Cargo.toml', 'pyproject.toml'];

function git(cwd: string, args: string[]): Promise<string | undefined> {
    return new Promise(resolve => execFile('git', args, { cwd, maxBuffer: 256 * 1024 * 1024 }, (error, stdout) => resolve(error ? undefined : stdout)));
}

/**
 * Gathers what `computeFacts` needs (the repository map, the manifests, the head commit and the history) and caches
 * the result per commit next to the repository map: a clean checkout's facts are read back from disk, a dirty one's
 * are kept in memory until its files change.
 */
@injectable()
export class RepoFacts {

    @inject(RepoIndex) protected readonly index: RepoIndex;
    @inject(ReviewStore) protected readonly store: ReviewStore;

    protected readonly recent = new Map<string, Facts>();

    async get(root: string, focus?: string): Promise<Facts> {
        const [sha, date, status] = await Promise.all([
            git(root, ['rev-parse', '--short', 'HEAD']), git(root, ['log', '-1', '--format=%cI']), git(root, ['status', '--porcelain'])]);
        const dirty = !!status?.trim();
        const head = { sha: sha?.trim() || 'none', date: date?.trim() || '', dirty };
        const key = `${head.sha}:${dirty ? createHash('sha1').update(status!).digest('hex').slice(0, 12) : 'clean'}:${focus ?? ''}`;
        const cached = this.recent.get(key) ?? (dirty ? undefined : await this.load(root, key));
        if (cached) {
            return cached;
        }
        const [map, log, created, count, manifests] = await Promise.all([
            this.index.get(root),
            git(root, ['log', '-n', String(MAX_COMMITS), '--no-merges', '--numstat', '--format=%x1e%H%x1f%aI%x1f%an%x1f%s']),
            git(root, ['log', '-n', String(MAX_COMMITS), '--no-merges', '--diff-filter=A', '--name-only', '--format=%x1e%H']),
            git(root, ['rev-list', '--count', 'HEAD']),
            this.manifests(root)
        ]);
        const facts = computeFacts({
            files: map.files, truncated: Object.keys(map.files).length >= 5000, head,
            commits: parseLog(log ?? '', created ?? ''), totalCommits: Number(count?.trim()) || 0, manifests, focus
        });
        this.recent.set(key, facts);
        if (this.recent.size > 6) {
            this.recent.delete(this.recent.keys().next().value!);
        }
        if (!dirty) {
            await this.save(root, key, facts).catch(() => undefined);
        }
        return facts;
    }

    protected cacheFile(root: string): string {
        return path.join(this.store.workspaceDir(FileUri.create(root).toString()), 'repo-facts.json');
    }

    protected async load(root: string, key: string): Promise<Facts | undefined> {
        try {
            const saved = JSON.parse(await fs.readFile(this.cacheFile(root), 'utf8')) as Record<string, Facts>;
            const facts = saved[key];
            return facts?.version === FACTS_VERSION ? facts : undefined;
        } catch {
            return undefined;
        }
    }

    /** Keeps the facts of the last few commits, so switching branches back and forth doesn't recompute them. */
    protected async save(root: string, key: string, facts: Facts): Promise<void> {
        const file = this.cacheFile(root);
        const saved = await fs.readFile(file, 'utf8').then(t => JSON.parse(t) as Record<string, Facts>, () => ({} as Record<string, Facts>));
        const kept = Object.fromEntries([...Object.entries(saved).filter(([k]) => k !== key).slice(-4), [key, facts]]);
        await fs.mkdir(path.dirname(file), { recursive: true });
        await fs.writeFile(file, JSON.stringify(kept));
    }

    protected async manifests(root: string): Promise<Record<string, string>> {
        const listed = await git(root, ['ls-files', '-z', '--', ...MANIFESTS.flatMap(m => [m, `**/${m}`])]);
        const result: Record<string, string> = {};
        for (const file of (listed ?? '').split('\0').filter(Boolean).slice(0, 50)) {
            const text = await fs.readFile(path.join(root, file), 'utf8').catch(() => undefined);
            if (text !== undefined && text.length < 256 * 1024) {
                result[file] = text;
            }
        }
        return result;
    }
}

/** One section of the facts for an agent, or (no section) the summary: the commit, the counts, and what each section holds. */
export function renderFacts(facts: Facts, section?: FactsSection): string {
    if (section) {
        return JSON.stringify({ commit: facts.commit, focus: facts.focus, [section]: facts[section] });
    }
    const { entry, folders, names, shapes, history } = facts;
    return JSON.stringify({
        version: facts.version, commit: facts.commit, files: facts.files, truncated: facts.truncated, focus: facts.focus,
        dependencies: facts.dependencies,
        sections: {
            entry: `${entry.points.length} entry points (${[...new Set(entry.points.map(p => p.kind))].join(', ')}), ${entry.outbound.length} outbound targets, ${entry.env.length} environment variables, ${entry.external.length} external modules`,
            folders: `${folders.map.length} mapped folders (${[...new Set(folders.map.map(f => f.role))].join(', ')}), ${folders.ignore.length} to ignore`,
            load: `${facts.load.length} load-bearing files, by importers and rank`,
            names: `${names.casing.length} casing rules, ${names.folderRules.length} per-folder rules, ${names.verbs.length} verbs, ${names.suffixes.length} type suffixes, ${names.vocabulary.length} terms, ${names.lookalikes.length} look-alike groups`,
            shapes: `${shapes.siblingSets.length} repeated sibling-file sets, ${shapes.registrations.length} repeated registration calls`,
            history: history.thin ? `thin: ${history.commits} commits` : `${history.commits} commits (${history.read} read, ${history.ignoredLarge} large ignored), ${history.hotspots.length} hotspots, ${history.coupling.length} coupled pairs, ${history.recipes.length} candidate recipes`
        }
    });
}

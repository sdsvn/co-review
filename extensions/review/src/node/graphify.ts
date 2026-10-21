import { execFile } from 'child_process';
import { promises as fs } from 'fs';
import * as path from 'path';
import type { GraphifyGraph } from './repo-overview';

/**
 * Graphify (https://graphify.net) builds a knowledge graph of a repository: definitions, calls, imports, clusters of
 * code that work together (communities) and the most connected code. For code it uses Tree-sitter only (no LLM, no
 * API key), and `graphify update <repo>` takes seconds, so Co-Review builds the graph itself for whole-repository
 * reviews when the `graphify` command is installed. It writes `<repo>/graphify-out/`.
 *
 * A large repository may keep scoped graphs instead, one per folder (`services/graphify-out/`, `pkg/graphify-out/`, as
 * its AGENTS.md says), and a graph is untracked, so a git worktree has none: {@link findGraphs} looks for both.
 */

export type GraphState = 'built' | 'missing' | 'failed';

/** A graph younger than this is fresh enough for the overview; building it again is not worth the wait. */
const FRESH_MS = 60_000;

let installed: Promise<boolean> | undefined;
const running = new Map<string, Promise<GraphState>>();
const builtAt = new Map<string, number>();

function run(command: string, args: string[], options: { cwd?: string; timeout?: number } = {}): Promise<{ ok: boolean; stdout: string }> {
    return new Promise(resolve => execFile(command, args, { ...options, maxBuffer: 16 * 1024 * 1024 },
        (error, stdout) => resolve({ ok: !error, stdout: String(stdout) })));
}

/** Whether the `graphify` command is on the PATH; only a yes is remembered, so installing it later needs no restart. */
export function graphifyInstalled(): Promise<boolean> {
    installed ??= run('graphify', ['--help'], { timeout: 10_000 }).then(r => {
        if (!r.ok) {
            installed = undefined;
        }
        return r.ok;
    });
    return installed;
}

/** A Graphify graph of (part of) a repository. */
export interface GraphSource {
    /** The repository-relative folder it covers, '' for the whole repository; its `source_file`s are relative to it. */
    scope: string;
    /** Its `graph.json`. */
    file: string;
}

const NOT_SCOPES = new Set(['node_modules', 'vendor', 'dist', 'build', 'target', 'graphify-out']);

async function exists(file: string): Promise<boolean> {
    return fs.access(file).then(() => true, () => false);
}

/**
 * The repository's graphs in one checkout: its scoped graphs up to two folders down when it keeps them (the repository
 * chose those, e.g. one per service), else its own.
 */
async function graphsIn(base: string): Promise<GraphSource[]> {
    const found: GraphSource[] = [];
    const scan = async (scope: string, depth: number): Promise<void> => {
        const entries = await fs.readdir(path.join(base, scope), { withFileTypes: true }).catch(() => []);
        for (const entry of entries) {
            if (!entry.isDirectory() || entry.name.startsWith('.') || NOT_SCOPES.has(entry.name)) {
                continue;
            }
            const sub = scope ? `${scope}/${entry.name}` : entry.name;
            const file = path.join(base, sub, 'graphify-out', 'graph.json');
            if (await exists(file)) {
                found.push({ scope: sub, file });
            } else if (depth < 2) {
                await scan(sub, depth + 1);
            }
        }
    };
    await scan('', 1);
    if (found.length) {
        return found.sort((a, b) => a.scope.localeCompare(b.scope));
    }
    const own = path.join(base, 'graphify-out', 'graph.json');
    return await exists(own) ? [{ scope: '', file: own }] : [];
}

/** The main checkout of the repository `root` is a linked git worktree of; undefined when it is the main one. */
async function mainCheckout(root: string): Promise<string | undefined> {
    const common = await run('git', ['-C', root, 'rev-parse', '--path-format=absolute', '--git-common-dir']);
    const dir = common.ok ? common.stdout.trim() : '';
    if (path.basename(dir) !== '.git') {
        return undefined;
    }
    const main = path.dirname(dir);
    const [a, b] = await Promise.all([fs.realpath(main).catch(() => main), fs.realpath(root).catch(() => root)]);
    return a === b ? undefined : main;
}

/**
 * The repository's Graphify graphs: its scoped graphs (`<folder>/graphify-out/`), or else `<root>/graphify-out/graph.json`,
 * or else, in a linked git worktree (graphs are untracked, so it has none), those of the main checkout.
 */
export async function findGraphs(root: string): Promise<GraphSource[]> {
    const own = await graphsIn(root);
    if (own.length) {
        return own;
    }
    const main = await mainCheckout(root);
    return main ? graphsIn(main) : [];
}

/**
 * The repository's graphs as one graph with repository-relative `source_file`s (see {@link findGraphs}); scoped graphs
 * keep their nodes and communities apart. Undefined without a graph.
 */
export async function loadGraph(root: string): Promise<{ graph: GraphifyGraph; sources: GraphSource[] } | undefined> {
    const sources = await findGraphs(root);
    const graph: GraphifyGraph = { nodes: [], links: [] };
    let communities = 0;
    for (const source of sources) {
        const g = await fs.readFile(source.file, 'utf8').then(text => JSON.parse(text) as GraphifyGraph, () => undefined);
        if (!g?.nodes?.length) {
            continue;
        }
        const id = (n: string) => source.scope ? `${source.scope}:${n}` : n;
        const offset = communities;
        for (const node of g.nodes) {
            graph.nodes.push({
                ...node, id: id(node.id),
                source_file: node.source_file && source.scope ? path.posix.join(source.scope, node.source_file) : node.source_file,
                community: node.community === undefined ? undefined : node.community + offset
            });
            communities = Math.max(communities, (node.community ?? -1) + offset + 1);
        }
        for (const edge of g.links ?? g.edges ?? []) {
            graph.links!.push({ ...edge, source: id(edge.source), target: id(edge.target) });
        }
    }
    return graph.nodes.length ? { graph, sources } : undefined;
}

/**
 * Builds or refreshes `<root>/graphify-out/graph.json`; `missing` without the `graphify` command. A repository with
 * scoped graphs (in `root`, or in the main checkout of the worktree `root` is) keeps them: no whole-repository graph.
 */
export function buildGraph(root: string): Promise<GraphState> {
    const pending = running.get(root);
    if (pending) {
        return pending;
    }
    const build = (async (): Promise<GraphState> => {
        const found = await findGraphs(root);
        if (found.length && found.every(g => g.scope)) {
            return 'built';
        }
        if (!await graphifyInstalled()) {
            return 'missing';
        }
        const out = path.join(root, 'graphify-out');
        const graph = path.join(out, 'graph.json');
        const existed = await fs.access(out).then(() => true, () => false);
        if (existed && Date.now() - (builtAt.get(root) ?? 0) < FRESH_MS) {
            return 'built';
        }
        const result = await run('graphify', ['update', root], { cwd: root, timeout: 180_000 });
        const ok = await fs.access(graph).then(() => true, () => false);
        if (!ok) {
            console.error(`[co-review] graphify update ${root} failed${result.stdout ? `: ${result.stdout.slice(-400)}` : ''}`);
            return 'failed';
        }
        builtAt.set(root, Date.now());
        if (!existed) {
            await excludeFromGit(root);
        }
        return 'built';
    })().finally(() => running.delete(root));
    running.set(root, build);
    return build;
}

/**
 * Co-Review created `graphify-out/`: keep it out of `git status` with the clone's own exclude file
 * (`.git/info/exclude`, never committed), unless the repository already ignores it.
 */
async function excludeFromGit(root: string): Promise<void> {
    if ((await run('git', ['-C', root, 'check-ignore', '-q', 'graphify-out/graph.json'])).ok) {
        return;
    }
    const excludePath = await run('git', ['-C', root, 'rev-parse', '--git-path', 'info/exclude']);
    if (!excludePath.ok || !excludePath.stdout.trim()) {
        return;
    }
    const file = path.resolve(root, excludePath.stdout.trim());
    await fs.mkdir(path.dirname(file), { recursive: true });
    const current = await fs.readFile(file, 'utf8').catch(() => '');
    await fs.appendFile(file, `${current && !current.endsWith('\n') ? '\n' : ''}# Graphify's code graph, built by Co-Review\ngraphify-out/\n`);
}

/** A definition in the graph, for a change's blast radius. */
export interface GraphNode {
    id: string;
    label: string;
    file: string;
    line?: number;
    area?: string;
}

/** What a change touches, and what depends on it: see {@link blastRadius}. */
export interface BlastRadius {
    /** The definitions the change touches. */
    changed: GraphNode[];
    /** What calls, imports, references or implements them (two hops), and what they call. */
    affected: (GraphNode & { hops: number; direction: 'uses it' | 'it uses' })[];
    /** Dependencies among those, `from` depends on `to`. */
    edges: { from: string; to: string }[];
    /** The files of the affected definitions, repository-relative: the change's neighbourhood. */
    files: string[];
}

const MAX_CHANGED = 40;
const MAX_AFFECTED = 60;

/**
 * A change's blast radius from the repository's Graphify graphs (see {@link findGraphs}), if it has any: the
 * definitions whose code the change touches (the nearest one starting at or above each changed line), then what
 * depends on them (callers, importers, implementers: two hops) and what they call (one hop). Undefined without a graph.
 */
export async function blastRadius(root: string, changedLines: Map<string, number[]>): Promise<BlastRadius | undefined> {
    const { DEPENDENCIES } = await import('./repo-overview');
    const graph = (await loadGraph(root))?.graph;
    if (!graph) {
        return undefined;
    }
    const nodes = new Map(graph.nodes.filter(n => n.source_file).map(n => [n.id, {
        id: n.id, label: n.label ?? n.id, file: n.source_file!, line: Number(/\d+/.exec(n.source_location ?? '')?.[0]) || undefined, area: n.community_name
    } as GraphNode]));
    const byFile = new Map<string, GraphNode[]>();
    for (const node of nodes.values()) {
        byFile.set(node.file, [...byFile.get(node.file) ?? [], node]);
    }
    const changed = new Set<string>();
    for (const [file, lines] of changedLines) {
        const defs = (byFile.get(file) ?? []).filter(n => n.line).sort((a, b) => a.line! - b.line!);
        for (const line of lines) {
            // The innermost definition starting at or above the line (the file's own node when there is none).
            const def = [...defs].reverse().find(n => n.line! <= line) ?? (byFile.get(file) ?? []).find(n => !n.line || n.line === 1);
            if (def) {
                changed.add(def.id);
            }
        }
    }
    const dependsOn = new Map<string, Set<string>>();
    const dependedOnBy = new Map<string, Set<string>>();
    for (const edge of graph.links ?? graph.edges ?? []) {
        if (DEPENDENCIES.has(edge.relation ?? '') && nodes.has(edge.source) && nodes.has(edge.target) && edge.source !== edge.target) {
            dependsOn.set(edge.source, (dependsOn.get(edge.source) ?? new Set()).add(edge.target));
            dependedOnBy.set(edge.target, (dependedOnBy.get(edge.target) ?? new Set()).add(edge.source));
        }
    }
    const affected = new Map<string, { hops: number; direction: 'uses it' | 'it uses' }>();
    const edges: { from: string; to: string }[] = [];
    let frontier = [...changed];
    for (let hops = 1; hops <= 2; hops++) {
        const next: string[] = [];
        for (const id of frontier) {
            for (const user of dependedOnBy.get(id) ?? []) {
                edges.push({ from: user, to: id });
                if (!changed.has(user) && !affected.has(user)) {
                    affected.set(user, { hops, direction: 'uses it' });
                    next.push(user);
                }
            }
        }
        frontier = next;
    }
    for (const id of changed) {
        for (const used of dependsOn.get(id) ?? []) {
            edges.push({ from: id, to: used });
            if (!changed.has(used) && !affected.has(used)) {
                affected.set(used, { hops: 1, direction: 'it uses' });
            }
        }
    }
    const changedNodes = [...changed].map(id => nodes.get(id)!).slice(0, MAX_CHANGED);
    const affectedNodes = [...affected].sort(([, a], [, b]) => a.hops - b.hops || (a.direction === 'uses it' ? -1 : 1))
        .slice(0, MAX_AFFECTED).map(([id, how]) => ({ ...nodes.get(id)!, ...how }));
    const shown = new Set([...changedNodes, ...affectedNodes].map(n => n.id));
    return {
        changed: changedNodes,
        affected: affectedNodes,
        edges: edges.filter(e => shown.has(e.from) && shown.has(e.to)),
        files: [...new Set(affectedNodes.map(n => n.file))].filter(f => !changedLines.has(f))
    };
}

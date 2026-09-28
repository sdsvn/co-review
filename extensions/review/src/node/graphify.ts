import { execFile } from 'child_process';
import { promises as fs } from 'fs';
import * as path from 'path';

/**
 * Graphify (https://graphify.net) builds a knowledge graph of a repository: definitions, calls, imports, clusters of
 * code that work together (communities) and the most connected code. For code it uses Tree-sitter only (no LLM, no
 * API key), and `graphify update <repo>` takes seconds, so Co-Review builds the graph itself for whole-repository
 * reviews when the `graphify` command is installed. It writes `<repo>/graphify-out/`.
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

/** Builds or refreshes `<root>/graphify-out/graph.json`; `missing` without the `graphify` command. */
export function buildGraph(root: string): Promise<GraphState> {
    const pending = running.get(root);
    if (pending) {
        return pending;
    }
    const build = (async (): Promise<GraphState> => {
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

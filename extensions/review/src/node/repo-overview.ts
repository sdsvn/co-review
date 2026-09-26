import { promises as fs } from 'fs';
import * as path from 'path';
import { ReviewCoverage } from '../common/review-model';

/** A Graphify `graph.json` (networkx node-link): what the overview reads from it. */
interface GraphifyGraph {
    nodes: { id: string; label?: string; source_file?: string; source_location?: string; community?: number; community_name?: string }[];
    links?: GraphifyEdge[];
    edges?: GraphifyEdge[];
}

interface GraphifyEdge {
    source: string;
    target: string;
    relation?: string;
}

/** Edges that say how code depends on code; `contains` / `method` only restate the file's structure. */
const DEPENDENCIES = new Set(['calls', 'references', 'imports', 'imports_from', 'implements', 'indirect_call', 'inherits', 'uses']);
const ENTRY_POINT = /(^|\/)(main|index|server|app|cli|cmd)\.[a-z]+$/i;

export interface OverviewInput {
    root: string;
    coverage: ReviewCoverage;
    /** Repository-relative file → its outline line (see RepoIndex). */
    outlines: Map<string, string>;
}

/**
 * The repository overview: where to start, the areas of the code and how they connect — the page a reviewer opens
 * before reviewing a whole repository. Built from a Graphify graph (`graphify-out/graph.json`) when there is one,
 * which knows calls, imports and clusters; otherwise from the Tree-sitter repo map (files, folders, definitions).
 */
export async function repositoryOverview(input: OverviewInput): Promise<string> {
    const graphFile = path.join(input.root, 'graphify-out', 'graph.json');
    const graph = await fs.readFile(graphFile, 'utf8').then(text => JSON.parse(text) as GraphifyGraph, () => undefined);
    const title = `# Repository overview: ${path.basename(input.root)}`;
    return (graph?.nodes?.length ? fromGraph(input, graph) : fromMap(input)).join('\n');

    function fromGraph(inputs: OverviewInput, g: GraphifyGraph): string[] {
        const nodes = new Map(g.nodes.map(n => [n.id, n]));
        const degree = new Map<string, number>();
        const between = new Map<string, number>();
        for (const edge of g.links ?? g.edges ?? []) {
            if (!DEPENDENCIES.has(edge.relation ?? '')) {
                continue;
            }
            degree.set(edge.source, (degree.get(edge.source) ?? 0) + 1);
            degree.set(edge.target, (degree.get(edge.target) ?? 0) + 1);
            const a = nodes.get(edge.source)?.community;
            const b = nodes.get(edge.target)?.community;
            if (a !== undefined && b !== undefined && a !== b) {
                const key = a < b ? `${a}-${b}` : `${b}-${a}`;
                between.set(key, (between.get(key) ?? 0) + 1);
            }
        }
        const communities = new Map<number, { name: string; nodes: GraphifyGraph['nodes'] }>();
        for (const node of g.nodes) {
            if (node.community !== undefined) {
                const c = communities.get(node.community) ?? { name: node.community_name ?? `Area ${node.community}`, nodes: [] };
                c.nodes.push(node);
                communities.set(node.community, c);
            }
        }
        const areas = [...communities.entries()].filter(([, c]) => c.nodes.length >= 3)
            .sort(([, a], [, b]) => b.nodes.length - a.nodes.length).slice(0, 12);
        const lines = [title, '',
            `Built from the Graphify graph of this repository (\`graphify-out/graph.json\`: ${g.nodes.length} definitions, `
            + `${communities.size} areas). ${inputs.coverage.viewed} of ${inputs.coverage.total} source files viewed so far.`, '',
            '## Start here', '',
            'The most connected code: what the rest of the repository calls, imports or implements.', ''];
        for (const [id, count] of [...degree].sort((a, b) => b[1] - a[1]).slice(0, 8)) {
            const node = nodes.get(id);
            if (node) {
                const area = node.community !== undefined ? communities.get(node.community)?.name : undefined;
                lines.push(`- ${link(node.label ?? id, node.source_file, node.source_location)}: ${count} connections${area ? `, in ${area}` : ''}`);
            }
        }
        lines.push('', '## Areas', '', 'Clusters of code that work together (Graphify communities), largest first, with their most used pieces.', '');
        for (const [, area] of areas) {
            const files = [...new Set(area.nodes.map(n => n.source_file).filter((f): f is string => !!f))];
            // The pieces the rest depends on most, as links into the code; the files come with them.
            const key = [...area.nodes].sort((a, b) => (degree.get(b.id) ?? 0) - (degree.get(a.id) ?? 0)).slice(0, 3)
                .map(n => link(n.label ?? n.id, n.source_file, n.source_location));
            lines.push(`- **${area.name}** (${files.length} file${files.length === 1 ? '' : 's'}): ${key.join(', ')}`);
        }
        const shown = new Set(areas.map(([id]) => id));
        const edges = [...between].map(([key, count]) => ({ a: Number(key.split('-')[0]), b: Number(key.split('-')[1]), count }))
            .filter(e => shown.has(e.a) && shown.has(e.b) && e.count >= 2).sort((x, y) => y.count - x.count).slice(0, 16);
        if (edges.length) {
            lines.push('', '## How the areas connect', '', 'Lines are calls, references and imports between areas; thicker means more.', '',
                '```mermaid', '%% id: areas', 'flowchart LR');
            for (const [id, area] of areas) {
                lines.push(`    a${id}["${area.name.replace(/"/g, '\'')} (${area.nodes.length})"]`);
            }
            for (const e of edges) {
                lines.push(`    a${e.a} ${e.count >= 10 ? '===' : '---'}|${e.count}| a${e.b}`);
            }
            lines.push('```');
        }
        return lines;
    }

    function fromMap(inputs: OverviewInput): string[] {
        const files = [...inputs.outlines.keys()];
        const entry = files.filter(f => ENTRY_POINT.test(f)).slice(0, 6);
        const rich = [...inputs.outlines].sort((a, b) => b[1].split(',').length - a[1].split(',').length).slice(0, 4).map(([f]) => f);
        const lines = [title, '',
            `Built from the Tree-sitter repo map: ${files.length} source files. ${inputs.coverage.viewed} of ${inputs.coverage.total} viewed so far.`,
            'For calls, imports and clusters of related code, build a Graphify graph (`graphify update .`) and reopen this page.', '',
            '## Start here', ''];
        for (const readme of ['README.md', 'readme.md', 'README']) {
            if (require('fs').existsSync(path.join(inputs.root, readme))) {
                lines.push(`- ${link(readme, readme)}: what the project says about itself`);
                break;
            }
        }
        entry.forEach(f => lines.push(`- ${link(f, f)}: looks like an entry point`));
        const count = (f: string) => (inputs.outlines.get(f) ?? '').split(/,\s*(?![^{]*\})/).filter(Boolean).length;
        rich.filter(f => !entry.includes(f)).forEach(f => lines.push(`- ${link(f, f)}: one of the largest files (${count(f)} top-level definitions)`));
        lines.push('', '## Areas', '', 'Folders, with what they define.', '');
        for (const area of inputs.coverage.areas) {
            const inArea = files.filter(f => area.path === '.' ? !f.includes('/') : f.startsWith(`${area.path}/`));
            const names = inArea.flatMap(f => (inputs.outlines.get(f) ?? '').split(/,\s*(?![^{]*\})/).map(s => s.split(' {')[0]))
                .filter(Boolean).slice(0, 6);
            lines.push(`- **${area.path === '.' ? '(root)' : area.path}**: ${area.total} file${area.total === 1 ? '' : 's'}, `
                + `${area.viewed} viewed${names.length ? `. Defines ${names.join(', ')}` : ''}${area.next ? `. Next: ${link(path.basename(area.next), area.next)}` : ''}.`);
        }
        return lines;
    }
}

/** A Markdown link that opens the code at that line (`path:line`, resolved from the repository root). */
function link(label: string, file?: string, location?: string): string {
    if (!file) {
        return `\`${label}\``;
    }
    const line = location?.match(/L(\d+)/)?.[1];
    return `[${label.replace(/[[\]]/g, '')}](${encodeURI(file)}${line && line !== '1' ? `:${line}` : ''})`;
}

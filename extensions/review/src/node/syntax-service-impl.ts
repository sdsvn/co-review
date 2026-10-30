import { injectable } from '@theia/core/shared/inversify';
import * as path from 'path';
import type { Language as TSLanguage, Node as TSNode, Parser as TSParser, Tree } from '@vscode/tree-sitter-wasm';
import { Range, SyntaxAnchor } from '../common/review-model';
import { SyntaxService, SyntaxSymbol, SyntaxSymbolKind } from '../common/syntax-protocol';

/** Monaco language id -> grammar file stem in @vscode/tree-sitter-wasm. */
const GRAMMARS: Record<string, string> = {
    go: 'go',
    typescript: 'typescript',
    typescriptreact: 'tsx',
    javascript: 'javascript',
    javascriptreact: 'javascript',
    python: 'python',
    java: 'java',
    rust: 'rust',
    csharp: 'c-sharp',
    cpp: 'cpp',
    c: 'cpp',
    ruby: 'ruby',
    php: 'php',
    shellscript: 'bash',
    powershell: 'powershell'
};

/** Definition node types per grammar, and the symbol kind each one produces. */
const DEFINITIONS: Record<string, Record<string, SyntaxSymbolKind>> = {
    go: { type_spec: 'type', function_declaration: 'function', method_declaration: 'method', method_elem: 'method', field_declaration: 'field' },
    typescript: {
        class_declaration: 'class', abstract_class_declaration: 'class', interface_declaration: 'interface', enum_declaration: 'enum',
        type_alias_declaration: 'type', function_declaration: 'function', generator_function_declaration: 'function',
        method_definition: 'method', method_signature: 'method', abstract_method_signature: 'method', internal_module: 'module',
        public_field_definition: 'field', variable_declarator: 'function'
    },
    javascript: {
        class_declaration: 'class', function_declaration: 'function', generator_function_declaration: 'function',
        method_definition: 'method', field_definition: 'field', variable_declarator: 'function'
    },
    python: { class_definition: 'class', function_definition: 'function' },
    java: {
        class_declaration: 'class', interface_declaration: 'interface', enum_declaration: 'enum', record_declaration: 'class',
        method_declaration: 'method', constructor_declaration: 'constructor'
    },
    rust: {
        function_item: 'function', function_signature_item: 'function', struct_item: 'struct', enum_item: 'enum',
        trait_item: 'interface', impl_item: 'class', mod_item: 'module', type_item: 'type'
    },
    'c-sharp': {
        namespace_declaration: 'module', file_scoped_namespace_declaration: 'module', class_declaration: 'class', struct_declaration: 'struct',
        interface_declaration: 'interface', enum_declaration: 'enum', record_declaration: 'class',
        method_declaration: 'method', constructor_declaration: 'constructor', property_declaration: 'field'
    },
    cpp: {
        namespace_definition: 'module', class_specifier: 'class', struct_specifier: 'struct', enum_specifier: 'enum', function_definition: 'function'
    },
    ruby: { module: 'module', class: 'class', method: 'method', singleton_method: 'method' },
    php: {
        namespace_definition: 'module', class_declaration: 'class', interface_declaration: 'interface', trait_declaration: 'interface',
        enum_declaration: 'enum', function_definition: 'function', method_declaration: 'method'
    },
    bash: { function_definition: 'function' },
    powershell: { function_statement: 'function', class_statement: 'class' }
};
DEFINITIONS.tsx = DEFINITIONS.typescript;

/** Import node types per grammar (see getReach); grammars not listed report no imports. */
const IMPORT_NODES: Record<string, string[]> = {
    typescript: ['import_statement', 'export_statement'], tsx: ['import_statement', 'export_statement'],
    javascript: ['import_statement', 'export_statement'], go: ['import_spec'],
    python: ['import_statement', 'import_from_statement'], rust: ['use_declaration'], java: ['import_declaration']
};
const CALL_NODES: Record<string, string[]> = {
    typescript: ['call_expression'], tsx: ['call_expression'], javascript: ['call_expression'], go: ['call_expression'],
    python: ['call'], rust: ['call_expression'], java: ['method_invocation']
};
/** Calls that register an entry point by name: `registerTool('x')`, `addCommand('x')`, `HandleFunc('/x')`. */
const REGISTRATION = /^(register\w*|add(Command|Route|Tool|Handler|Job)\w*|handle(Func)?|route|command|tool|subscribe|schedule)$/i;
/** HTTP routes count only on something named like a router, so `map.get('x')` isn't one. */
const HTTP_VERB = /^(get|post|put|patch|delete|options|head|all)$/i;
const ROUTER = /^(app|router|r|e|g|api|server|srv|mux|fastify|routes?|v\d+)$/i;
/** Calls that leave the process: HTTP clients and spawned programs. */
const OUTBOUND = /^(fetch|execFile|execFileSync|exec|execSync|spawn|spawnSync|fork|urlopen|NewRequest|Command|CommandContext)$/;
const OUTBOUND_RECEIVER = /^(axios|requests|httpx|got|superagent|reqwest|subprocess)$/;
/** `exec` and friends count only bare or on a process module, so `pattern.exec(text)` isn't one. */
const SPAWNER = /^(child_process|cp|childProcess|exec|os|globalThis|window)$/;

/** Anchors longer than this are not worth a token-level anchor (symbols cover them). */
const MAX_ANCHOR_TOKENS = 600;

function toRange(node: TSNode): Range {
    return {
        start: { line: node.startPosition.row, character: node.startPosition.column },
        end: { line: node.endPosition.row, character: node.endPosition.column }
    };
}

function before(a: Range['start'], b: Range['start']): boolean {
    return a.line < b.line || (a.line === b.line && a.character < b.character);
}

@injectable()
export class SyntaxServiceImpl implements SyntaxService {

    protected module: Promise<typeof import('@vscode/tree-sitter-wasm')> | undefined;
    protected readonly languages = new Map<string, Promise<TSLanguage>>();
    protected readonly parsers = new Map<string, TSParser>();
    /** Small cache so decorating several threads of one file parses it once. */
    protected readonly trees: { grammar: string; text: string; tree: Tree }[] = [];

    protected get wasmDir(): string {
        return path.dirname(require.resolve('@vscode/tree-sitter-wasm'));
    }

    protected load(): Promise<typeof import('@vscode/tree-sitter-wasm')> {
        if (!this.module) {
            this.module = (async () => {
                // eslint-disable-next-line @typescript-eslint/no-require-imports
                const ts = require('@vscode/tree-sitter-wasm') as typeof import('@vscode/tree-sitter-wasm');
                await ts.Parser.init({ locateFile: (file: string) => path.join(this.wasmDir, file) });
                return ts;
            })();
        }
        return this.module;
    }

    protected async parse(languageId: string, text: string): Promise<{ grammar: string; tree: Tree } | undefined> {
        const grammar = GRAMMARS[languageId];
        if (!grammar) {
            return undefined;
        }
        const index = this.trees.findIndex(t => t.grammar === grammar && t.text === text);
        if (index >= 0) {
            // Most recently used first: the tree a caller holds is the last to be evicted (and deleted).
            const [cached] = this.trees.splice(index, 1);
            this.trees.unshift(cached);
            return cached;
        }
        const ts = await this.load();
        let language = this.languages.get(grammar);
        if (!language) {
            language = ts.Language.load(path.join(this.wasmDir, `tree-sitter-${grammar}.wasm`));
            this.languages.set(grammar, language);
        }
        let parser = this.parsers.get(grammar);
        if (!parser) {
            parser = new ts.Parser();
            parser.setLanguage(await language);
            this.parsers.set(grammar, parser);
        }
        const tree = parser.parse(text);
        if (!tree) {
            return undefined;
        }
        this.trees.unshift({ grammar, text, tree });
        this.trees.splice(8).forEach(t => t.tree.delete());
        return { grammar, tree };
    }

    async getSupportedLanguages(): Promise<string[]> {
        return Object.keys(GRAMMARS);
    }

    async getSymbols(languageId: string, text: string): Promise<SyntaxSymbol[]> {
        const parsed = await this.parse(languageId, text);
        if (!parsed) {
            return [];
        }
        const definitions = DEFINITIONS[parsed.grammar] ?? {};
        const collect = (node: TSNode): SyntaxSymbol[] => {
            const result: SyntaxSymbol[] = [];
            for (const child of node.namedChildren) {
                if (!child) {
                    continue;
                }
                const kind = definitions[child.type];
                const nameNode = kind && this.nameOf(parsed.grammar, child);
                if (kind && nameNode && this.isDefinition(parsed.grammar, child)) {
                    result.push({
                        name: this.qualifiedName(parsed.grammar, child, nameNode),
                        kind,
                        range: toRange(child),
                        selectionRange: toRange(nameNode),
                        children: collect(child)
                    });
                } else {
                    result.push(...collect(child));
                }
            }
            return result;
        };
        return collect(parsed.tree.rootNode);
    }

    /**
     * What a file reaches outside itself, for the repository facts (repo-facts.ts): the modules it imports, as written
     * (`./store`, `github.com/x/y`, `..models`), and the calls that register an entry point or go outside the process
     * (`registerTool('open_review')`, `execFile('git')`), each with its first string argument and 0-based line.
     */
    async getReach(languageId: string, text: string): Promise<{ imports: string[]; calls: [kind: 'reg' | 'out', callee: string, arg: string, line: number][] }> {
        const parsed = await this.parse(languageId, text);
        if (!parsed) {
            return { imports: [], calls: [] };
        }
        const unquote = (s: string) => s.replace(/^[`'"]|[`'"]$/g, '');
        const imports = new Set<string>();
        const calls: [kind: 'reg' | 'out', callee: string, arg: string, line: number][] = [];
        const root = parsed.tree.rootNode;
        const grammar = parsed.grammar;
        for (const node of root.descendantsOfType(IMPORT_NODES[grammar] ?? [])) {
            if (!node) {
                continue;
            }
            if (grammar === 'go') {
                const spec = node.childForFieldName('path');
                spec && imports.add(unquote(spec.text));
            } else if (grammar === 'python') {
                const from = node.childForFieldName('module_name');
                if (from) {
                    imports.add(from.text);
                } else {
                    node.namedChildren.forEach(c => c && imports.add((c.childForFieldName('name') ?? c).text));
                }
            } else if (grammar === 'rust') {
                const argument = node.childForFieldName('argument');
                argument && imports.add(argument.text.replace(/::\{[\s\S]*$/, '').replace(/::\*$/, ''));
            } else if (grammar === 'java') {
                imports.add(node.text.replace(/^import\s+(static\s+)?/, '').replace(/[;\s]|\.\*$/g, ''));
            } else {
                const source = node.childForFieldName('source');
                source && imports.add(unquote(source.text));
            }
        }
        for (const call of root.descendantsOfType(CALL_NODES[grammar] ?? [])) {
            const fn = call && (call.childForFieldName('function') ?? call.childForFieldName('name'));
            if (!call || !fn) {
                continue;
            }
            const object = call.childForFieldName('object')?.text;
            const callee = object ? `${object}.${fn.text}` : fn.text;
            const first = call.childForFieldName('arguments')?.namedChildren[0];
            const arg = first && /string/.test(first.type) ? unquote(first.text) : '';
            if ((callee === 'require' || fn.type === 'import') && arg && ['javascript', 'typescript', 'tsx'].includes(grammar)) {
                imports.add(arg);
                continue;
            }
            const parts = callee.split(/\.|::/);
            const name = parts[parts.length - 1];
            const receiver = parts.length > 1 ? parts[parts.length - 2] : '';
            if (arg && (REGISTRATION.test(name) || (HTTP_VERB.test(name) && ROUTER.test(receiver)))) {
                calls.push(['reg', callee, arg, call.startPosition.row]);
            } else if ((OUTBOUND.test(name) && (!receiver || SPAWNER.test(receiver))) || OUTBOUND_RECEIVER.test(receiver)
                || (/^https?$/.test(receiver) && /^(request|get)$/.test(name))) {
                calls.push(['out', callee, arg, call.startPosition.row]);
            }
        }
        return { imports: [...imports], calls };
    }

    /** Filters definition node types that are only sometimes definitions. */
    protected isDefinition(grammar: string, node: TSNode): boolean {
        if (node.type === 'variable_declarator') {
            // Only `const foo = () => {}` / `const foo = function () {}` style functions.
            const value = node.childForFieldName('value');
            return !!value && ['arrow_function', 'function_expression', 'function', 'generator_function'].includes(value.type);
        }
        if (grammar === 'go' && node.type === 'field_declaration') {
            return false;
        }
        return true;
    }

    protected nameOf(grammar: string, node: TSNode): TSNode | null {
        if (grammar === 'rust' && node.type === 'impl_item') {
            return node.childForFieldName('type');
        }
        const name = node.childForFieldName('name');
        if (name) {
            return name;
        }
        // C/C++ functions: dig through the declarator chain to the identifier.
        let declarator = node.childForFieldName('declarator');
        while (declarator) {
            if (['identifier', 'field_identifier', 'qualified_identifier', 'destructor_name', 'operator_name'].includes(declarator.type)) {
                return declarator;
            }
            declarator = declarator.childForFieldName('declarator') ?? declarator.namedChildren.find(c => c?.type.endsWith('identifier')) ?? null;
        }
        return null;
    }

    /** Go methods live at the top level; qualify them with their receiver type (`OrderService.CreateOrder`). */
    protected qualifiedName(grammar: string, node: TSNode, nameNode: TSNode): string {
        if (grammar === 'go' && node.type === 'method_declaration') {
            const receiver = node.childForFieldName('receiver')?.text.match(/([A-Za-z_]\w*)(\[[^\]]*\])?\s*\)$/);
            if (receiver) {
                return `${receiver[1]}.${nameNode.text}`;
            }
        }
        return nameNode.text;
    }

    protected leaves(root: TSNode): TSNode[] {
        const result: TSNode[] = [];
        const cursor = root.walk();
        let done = false;
        while (!done) {
            const node = cursor.currentNode;
            if (node.childCount === 0) {
                if (!node.type.includes('comment') && node.text.trim()) {
                    result.push(node);
                }
            } else if (!node.type.includes('comment') && cursor.gotoFirstChild()) {
                continue;
            }
            while (!cursor.gotoNextSibling()) {
                if (!cursor.gotoParent()) {
                    done = true;
                    break;
                }
            }
        }
        cursor.delete();
        return result;
    }

    async captureAnchor(languageId: string, text: string, range: Range): Promise<SyntaxAnchor | undefined> {
        const parsed = await this.parse(languageId, text);
        if (!parsed) {
            return undefined;
        }
        const tokens = this.leaves(parsed.tree.rootNode).filter(leaf => {
            const r = toRange(leaf);
            return !before(r.start, range.start) && !before(range.end, r.end);
        }).map(leaf => leaf.text);
        if (!tokens.length || tokens.length > MAX_ANCHOR_TOKENS) {
            return undefined;
        }
        return { grammar: parsed.grammar, tokens };
    }

    async locateAnchor(languageId: string, text: string, anchor: SyntaxAnchor, nearLine: number): Promise<Range | undefined> {
        const parsed = await this.parse(languageId, text);
        if (!parsed || parsed.grammar !== anchor.grammar || !anchor.tokens.length) {
            return undefined;
        }
        const leaves = this.leaves(parsed.tree.rootNode);
        const first = anchor.tokens[0];
        let best: Range | undefined;
        for (let i = 0; i + anchor.tokens.length <= leaves.length; i++) {
            if (leaves[i].text !== first || !anchor.tokens.every((t, j) => leaves[i + j].text === t)) {
                continue;
            }
            const candidate: Range = {
                start: toRange(leaves[i]).start,
                end: toRange(leaves[i + anchor.tokens.length - 1]).end
            };
            if (!best || Math.abs(candidate.start.line - nearLine) < Math.abs(best.start.line - nearLine)) {
                best = candidate;
            }
        }
        return best;
    }
}

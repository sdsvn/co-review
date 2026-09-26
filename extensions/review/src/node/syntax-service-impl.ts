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

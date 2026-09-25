import { inject, injectable } from '@theia/core/shared/inversify';
import * as monaco from '@theia/monaco-editor-core';
import { CancellationToken } from '@theia/monaco-editor-core/esm/vs/base/common/cancellation';
import { DocumentSymbol, SymbolKind } from '@theia/monaco-editor-core/esm/vs/editor/common/languages';
import { ITextModel } from '@theia/monaco-editor-core/esm/vs/editor/common/model';
import { ILanguageFeaturesService } from '@theia/monaco-editor-core/esm/vs/editor/common/services/languageFeatures';
import { StandaloneServices } from '@theia/monaco-editor-core/esm/vs/editor/standalone/browser/standaloneServices';
import { CodeAnchor, CodeLocation, Range } from '../common/review-model';
import { SyntaxService, SyntaxSymbol } from '../common/syntax-protocol';

export function toMonacoRange(range: Range): monaco.Range {
    return new monaco.Range(range.start.line + 1, range.start.character + 1, range.end.line + 1, range.end.character + 1);
}

export function fromMonacoRange(range: monaco.IRange): Range {
    return {
        start: { line: range.startLineNumber - 1, character: range.startColumn - 1 },
        end: { line: range.endLineNumber - 1, character: range.endColumn - 1 }
    };
}

/**
 * - `exact`: the anchored code is still at the stored range.
 * - `moved`: the code (or its symbol) was found elsewhere in the file.
 * - `outdated`: the anchored code no longer exists; the stored range is shown as-is.
 */
export type AnchorState = 'exact' | 'moved' | 'outdated';

export interface ResolvedLocation {
    range: monaco.IRange;
    state: AnchorState;
}

export interface ResolvedSymbol {
    path: string;
    range: monaco.IRange;
    selectionRange: monaco.IRange;
}

/** Source-independent symbol tree node (Tree-sitter or LSP). */
interface SymbolNode {
    name: string;
    /** Too fine-grained to be part of a symbol path (locals, literals, …). */
    ignored: boolean;
    range: monaco.IRange;
    selectionRange: monaco.IRange;
    children: SymbolNode[];
}

const IGNORED_LSP_KINDS = new Set([SymbolKind.Variable, SymbolKind.Constant, SymbolKind.String, SymbolKind.Number,
    SymbolKind.Boolean, SymbolKind.Array, SymbolKind.Object, SymbolKind.Key, SymbolKind.Null, SymbolKind.TypeParameter]);

function fromLsp(symbols: DocumentSymbol[]): SymbolNode[] {
    return symbols.map(s => ({
        name: s.name, ignored: IGNORED_LSP_KINDS.has(s.kind), range: s.range, selectionRange: s.selectionRange, children: fromLsp(s.children ?? [])
    }));
}

function fromSyntax(symbols: SyntaxSymbol[]): SymbolNode[] {
    return symbols.map(s => ({
        name: s.name, ignored: false, range: toMonacoRange(s.range), selectionRange: toMonacoRange(s.selectionRange), children: fromSyntax(s.children)
    }));
}

function contains(range: monaco.IRange, line: number): boolean {
    return range.startLineNumber <= line && line <= range.endLineNumber;
}

function findEnclosing(symbols: SymbolNode[], line: number, parents: string[] = []): ResolvedSymbol | undefined {
    for (const symbol of symbols) {
        if (!contains(symbol.range, line)) {
            continue;
        }
        const path = symbol.ignored ? parents : [...parents, symbol.name];
        const child = findEnclosing(symbol.children, line, path);
        if (child) {
            return child;
        }
        if (!symbol.ignored) {
            return { path: path.join('.'), range: symbol.range, selectionRange: symbol.selectionRange };
        }
    }
    return undefined;
}

function findByPath(symbols: SymbolNode[], path: string, parents: string[] = []): ResolvedSymbol | undefined {
    for (const symbol of symbols) {
        const own = symbol.ignored ? parents : [...parents, symbol.name];
        const joined = own.join('.');
        if (!symbol.ignored && joined === path) {
            return { path, range: symbol.range, selectionRange: symbol.selectionRange };
        }
        if (symbol.ignored || path.startsWith(joined + '.')) {
            const found = findByPath(symbol.children, path, own);
            if (found) {
                return found;
            }
        }
    }
    return undefined;
}

/** Whitespace-insensitive search for a (possibly multi-line) anchor, nearest to `nearLine` wins. */
function findNormalised(model: monaco.editor.ITextModel, text: string, nearLine: number): monaco.IRange | undefined {
    const wanted = text.split('\n').map(l => l.trim().replace(/\s+/g, ' '));
    while (wanted.length && !wanted[wanted.length - 1]) {
        wanted.pop();
    }
    if (!wanted.length || !wanted.join('')) {
        return undefined;
    }
    const lines = model.getLinesContent().map(l => l.trim().replace(/\s+/g, ' '));
    let best: number | undefined;
    for (let i = 0; i + wanted.length <= lines.length; i++) {
        if (wanted.every((w, j) => w === lines[i + j]) && (best === undefined || Math.abs(i + 1 - nearLine) < Math.abs(best + 1 - nearLine))) {
            best = i;
        }
    }
    if (best === undefined) {
        return undefined;
    }
    const endLine = best + wanted.length;
    return new monaco.Range(best + 1, 1, endLine, model.getLineMaxColumn(endLine));
}

/**
 * Computes and resolves semantic review locations.
 *
 * Structure comes from Tree-sitter when a grammar exists for the language (deterministic,
 * available before any language server has started, robust against reformatting), and
 * from the language server's document symbols otherwise. Navigation itself stays with LSP.
 */
@injectable()
export class ReviewLocations {

    @inject(SyntaxService) protected readonly syntax: SyntaxService;

    protected supported: Promise<Set<string>> | undefined;

    protected async hasGrammar(model: monaco.editor.ITextModel): Promise<boolean> {
        this.supported ??= this.syntax.getSupportedLanguages().then(ids => new Set(ids), () => new Set<string>());
        return (await this.supported).has(model.getLanguageId());
    }

    protected async getSymbols(model: monaco.editor.ITextModel): Promise<SymbolNode[]> {
        if (await this.hasGrammar(model)) {
            const symbols = await this.syntax.getSymbols(model.getLanguageId(), model.getValue()).catch(() => []);
            if (symbols.length) {
                return fromSyntax(symbols);
            }
        }
        const registry = StandaloneServices.get(ILanguageFeaturesService).documentSymbolProvider;
        for (const provider of registry.ordered(model as unknown as ITextModel)) {
            try {
                const symbols = await provider.provideDocumentSymbols(model as unknown as ITextModel, CancellationToken.None);
                if (symbols?.length) {
                    return fromLsp(symbols);
                }
            } catch {
                /* try the next provider */
            }
        }
        return [];
    }

    /** Deepest meaningful symbol containing the line, e.g. `OrderService.CreateOrder`. */
    async findEnclosingSymbol(model: monaco.editor.ITextModel, line: number): Promise<ResolvedSymbol | undefined> {
        return findEnclosing(await this.getSymbols(model), line);
    }

    async findSymbol(model: monaco.editor.ITextModel, path: string): Promise<ResolvedSymbol | undefined> {
        return findByPath(await this.getSymbols(model), path);
    }

    async captureAnchor(model: monaco.editor.ITextModel, range: monaco.IRange): Promise<CodeAnchor> {
        const lines = model.getLineCount();
        const anchor: CodeAnchor = {
            text: model.getValueInRange(range),
            before: range.startLineNumber > 1 ? model.getLineContent(range.startLineNumber - 1) : undefined,
            after: range.endLineNumber < lines ? model.getLineContent(range.endLineNumber + 1) : undefined
        };
        if (await this.hasGrammar(model)) {
            anchor.syntax = await this.syntax.captureAnchor(model.getLanguageId(), model.getValue(), fromMonacoRange(range)).catch(() => undefined);
        }
        return anchor;
    }

    async resolve(model: monaco.editor.ITextModel, location: CodeLocation): Promise<ResolvedLocation | undefined> {
        if (!location.range) {
            return undefined;
        }
        const stored = model.validateRange(toMonacoRange(location.range));
        const anchor = location.anchor;
        const state = (range: monaco.IRange): ResolvedLocation => ({ range, state: monaco.Range.equalsRange(range, stored) ? 'exact' : 'moved' });

        if (location.kind === 'symbol' && location.symbol) {
            const symbol = await this.findSymbol(model, location.symbol);
            if (symbol) {
                return state(symbol.range);
            }
        }
        if (!anchor || !anchor.text.trim()) {
            return { range: stored, state: 'exact' };
        }
        if (model.getValueInRange(stored) === anchor.text) {
            return { range: stored, state: 'exact' };
        }
        // Syntax-aware: same token sequence, regardless of formatting and comments.
        if (anchor.syntax && await this.hasGrammar(model)) {
            const found = await this.syntax.locateAnchor(model.getLanguageId(), model.getValue(), anchor.syntax, stored.startLineNumber - 1).catch(() => undefined);
            if (found) {
                return state(toMonacoRange(found));
            }
            // The tokens are gone: the code changed, even if similar text exists elsewhere.
            return { range: stored, state: 'outdated' };
        }
        const matches = model.findMatches(anchor.text, false, false, true, null, false, 50);
        if (matches.length) {
            const score = (r: monaco.IRange) => {
                let s = -Math.abs(r.startLineNumber - stored.startLineNumber) / 10000;
                if (anchor.before !== undefined && r.startLineNumber > 1 && model.getLineContent(r.startLineNumber - 1) === anchor.before) {
                    s += 1;
                }
                if (anchor.after !== undefined && r.endLineNumber < model.getLineCount() && model.getLineContent(r.endLineNumber + 1) === anchor.after) {
                    s += 1;
                }
                return s;
            };
            return state(matches.map(m => m.range).sort((a, b) => score(b) - score(a))[0]);
        }
        const normalised = findNormalised(model, anchor.text, stored.startLineNumber);
        if (normalised) {
            return state(normalised);
        }
        // Not found: keep it visible where it was, but never move it onto code it may not describe.
        return { range: stored, state: 'outdated' };
    }
}

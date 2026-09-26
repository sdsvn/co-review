import { FrontendApplicationContribution } from '@theia/core/lib/browser/frontend-application-contribution';
import { inject, injectable } from '@theia/core/shared/inversify';
import * as monaco from '@theia/monaco-editor-core';
import { ITextModel } from '@theia/monaco-editor-core/esm/vs/editor/common/model';
import { ILanguageFeaturesService } from '@theia/monaco-editor-core/esm/vs/editor/common/services/languageFeatures';
import { StandaloneServices } from '@theia/monaco-editor-core/esm/vs/editor/standalone/browser/standaloneServices';
import { SyntaxService, SyntaxSymbol, SyntaxSymbolKind } from '../common/syntax-protocol';
import { toMonacoRange } from './review-locations';

const KINDS: Record<SyntaxSymbolKind, monaco.languages.SymbolKind> = {
    module: monaco.languages.SymbolKind.Module,
    class: monaco.languages.SymbolKind.Class,
    interface: monaco.languages.SymbolKind.Interface,
    struct: monaco.languages.SymbolKind.Struct,
    enum: monaco.languages.SymbolKind.Enum,
    type: monaco.languages.SymbolKind.TypeParameter,
    function: monaco.languages.SymbolKind.Function,
    method: monaco.languages.SymbolKind.Method,
    constructor: monaco.languages.SymbolKind.Constructor,
    field: monaco.languages.SymbolKind.Field
};

function toDocumentSymbol(symbol: SyntaxSymbol): monaco.languages.DocumentSymbol {
    return {
        name: symbol.name, detail: '', kind: KINDS[symbol.kind], tags: [],
        range: toMonacoRange(symbol.range), selectionRange: toMonacoRange(symbol.selectionRange),
        children: symbol.children.map(toDocumentSymbol)
    };
}

/**
 * The file's structure from Tree-sitter, in the editor itself: breadcrumbs, sticky scroll, Go to Symbol
 * and the outline work for every language with a grammar, with no language server installed or started.
 * Where a language server provides symbols, those are used instead (no duplicates).
 */
@injectable()
export class SyntaxSymbols implements FrontendApplicationContribution {

    @inject(SyntaxService) protected readonly syntax: SyntaxService;

    async onStart(): Promise<void> {
        const languages = await this.syntax.getSupportedLanguages().catch(() => []);
        const registry = StandaloneServices.get(ILanguageFeaturesService).documentSymbolProvider;
        const provider: monaco.languages.DocumentSymbolProvider = {
            displayName: 'Tree-sitter',
            provideDocumentSymbols: async model => {
                if (registry.all(model as unknown as ITextModel).length > 1) {
                    return [];
                }
                const symbols = await this.syntax.getSymbols(model.getLanguageId(), model.getValue()).catch(() => []);
                return symbols.map(toDocumentSymbol);
            }
        };
        for (const language of languages) {
            monaco.languages.registerDocumentSymbolProvider(language, provider);
        }
    }
}

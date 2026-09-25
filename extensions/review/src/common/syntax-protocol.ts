import { Range, SyntaxAnchor } from './review-model';

export const SYNTAX_SERVICE_PATH = '/services/co-review-syntax';
export const SyntaxService = Symbol('SyntaxService');

export type SyntaxSymbolKind = 'module' | 'class' | 'interface' | 'struct' | 'enum' | 'type' | 'function' | 'method' | 'constructor' | 'field';

export interface SyntaxSymbol {
    name: string;
    kind: SyntaxSymbolKind;
    range: Range;
    selectionRange: Range;
    children: SyntaxSymbol[];
}

/**
 * Tree-sitter based structural analysis. It is deliberately limited to what makes review
 * locations durable (definitions, token-level anchors); navigation stays with LSP.
 */
export interface SyntaxService {
    /** Monaco/VS Code language ids that have a Tree-sitter grammar. */
    getSupportedLanguages(): Promise<string[]>;
    getSymbols(languageId: string, text: string): Promise<SyntaxSymbol[]>;
    /** Token sequence covered by `range`, ignoring whitespace and comments. */
    captureAnchor(languageId: string, text: string, range: Range): Promise<SyntaxAnchor | undefined>;
    /** Finds the token sequence of `anchor` in `text`, nearest to `nearLine` when it occurs several times. */
    locateAnchor(languageId: string, text: string, anchor: SyntaxAnchor, nearLine: number): Promise<Range | undefined>;
}

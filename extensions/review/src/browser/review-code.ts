import { DiffUris } from '@theia/core/lib/browser/diff-uris';
import { open, OpenerService } from '@theia/core/lib/browser/opener-service';
import { Resource, ResourceResolver } from '@theia/core/lib/common/resource';
import URI from '@theia/core/lib/common/uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { CancellationToken } from '@theia/monaco-editor-core/esm/vs/base/common/cancellation';
import { Position } from '@theia/monaco-editor-core/esm/vs/editor/common/core/position';
import { IRange } from '@theia/monaco-editor-core/esm/vs/editor/common/core/range';
import { ITextModel } from '@theia/monaco-editor-core/esm/vs/editor/common/model';
import { ILanguageFeaturesService } from '@theia/monaco-editor-core/esm/vs/editor/common/services/languageFeatures';
import { StandaloneServices } from '@theia/monaco-editor-core/esm/vs/editor/standalone/browser/standaloneServices';
import { EditorManager } from '@theia/editor/lib/browser/editor-manager';
import { EditorWidget } from '@theia/editor/lib/browser/editor-widget';
import { MonacoDiffEditor } from '@theia/monaco/lib/browser/monaco-diff-editor';
import { MonacoTextModelService } from '@theia/monaco/lib/browser/monaco-text-model-service';
import { Review } from '../common/review-model';
import { ReviewService } from '../common/review-protocol';

/** A change review's file at the base of the change: read-only, from git (`co-review-base:/<path>?<review id>`). */
export const BASE_SCHEME = 'co-review-base';

@injectable()
export class BaseFileResolver implements ResourceResolver {

    @inject(ReviewService) protected readonly service: ReviewService;

    resolve(uri: URI): Resource {
        if (uri.scheme !== BASE_SCHEME) {
            throw new Error(`not a ${BASE_SCHEME} URI: ${uri}`);
        }
        const service = this.service;
        // No saveContents: the editor opens it read-only.
        return {
            uri,
            async readContents(): Promise<string> {
                return (await service.readBaseFile(uri.query, uri.path.toString().replace(/^\//, ''))) ?? '';
            },
            dispose: () => undefined
        };
    }
}

/** What a language server says about a name: its type and docs (Markdown). */
export interface CodeHover {
    contents: string[];
}

/**
 * Steps from a change review's diff into its code (see CodeFolders): the head file at a line, the base version, the two
 * side by side, and what the language server knows about a name on a new-side line (definition, hover), asked at the
 * same position in the head file, without opening it first.
 */
@injectable()
export class CodeNavigation {

    @inject(EditorManager) protected readonly editors: EditorManager;
    @inject(OpenerService) protected readonly openers: OpenerService;
    @inject(MonacoTextModelService) protected readonly models: MonacoTextModelService;

    /** Whether the review has code to step into. */
    has(review: Review | undefined): review is Review & { bundle: { code: string } } {
        return !!review?.bundle?.code;
    }

    headUri(review: Review, file: string): URI | undefined {
        return this.has(review) ? URI.fromFilePath(review.bundle.code).resolve(file) : undefined;
    }

    baseUri(review: Review, file: string): URI {
        return new URI(`${BASE_SCHEME}:/${file}`).withQuery(review.id);
    }

    /** The head file at a line (and column), in the editor. */
    async openHead(review: Review, file: string, line: number, column = 0): Promise<void> {
        const uri = this.headUri(review, file);
        if (uri) {
            const at = { line: line - 1, character: column };
            const widget = await this.editors.open(uri, { mode: 'activate', selection: { start: at, end: at } });
            widget.editor.revealPosition(at, { vertical: 'center' });
        }
    }

    /** The base version of a file at a line, read-only. */
    async openBase(review: Review, file: string, line: number): Promise<void> {
        const at = { line: line - 1, character: 0 };
        const widget = await this.editors.open(this.baseUri(review, file), { mode: 'activate', selection: { start: at, end: at } });
        widget.editor.revealPosition(at, { vertical: 'center' });
    }

    /**
     * Base against head, side by side, the full editor on the head (where comments are), at a line of either side.
     * `options` passes the opener's own (an explorer preview reveals without taking focus, in a preview tab).
     */
    async compare(review: Review, file: string, line?: number, side: 'new' | 'old' = 'new',
        options: { mode?: 'open' | 'reveal' | 'activate'; preview?: boolean } = {}): Promise<EditorWidget | undefined> {
        const head = this.headUri(review, file);
        if (head) {
            const at = line && side === 'new' ? { line: line - 1, character: 0 } : undefined;
            const widget = await open(this.openers, DiffUris.encode(this.baseUri(review, file), head, `${file} (base ↔ head)`),
                { mode: 'activate', ...options, ...(at ? { selection: { start: at, end: at } } : {}) });
            if (widget instanceof EditorWidget) {
                const reveal = () => {
                    if (at) {
                        widget.editor.revealPosition(at, { vertical: 'center' });
                    } else if (line && widget.editor instanceof MonacoDiffEditor) {
                        widget.editor.diffEditor.getOriginalEditor().revealLineInCenter(line);
                    }
                };
                reveal();
                // A diff editor shown for the first time scrolls to its first change once the diff is computed, over
                // this line: reveal it again then.
                if (line && widget.editor instanceof MonacoDiffEditor) {
                    const once = widget.editor.diffEditor.onDidUpdateDiff(() => { once.dispose(); setTimeout(reveal, 50); });
                    setTimeout(() => once.dispose(), 3000);
                }
                return widget;
            }
        }
        return undefined;
    }

    /** Goes to the definition of what is at `line`, `column` (1-based line, 0-based column) of the head file. */
    async goToDefinition(review: Review, file: string, line: number, column: number): Promise<boolean> {
        const found = await this.ask(review, file, line, column, async (model, position) => {
            for (const provider of StandaloneServices.get(ILanguageFeaturesService).definitionProvider.ordered(model)) {
                const result = await provider.provideDefinition(model, position, CancellationToken.None);
                const first = Array.isArray(result) ? result[0] : result;
                if (first) {
                    return first;
                }
            }
            return undefined;
        });
        if (!found) {
            return false;
        }
        // A location link carries the name's own range; a plain location, the whole definition.
        const range: IRange = (found as { targetSelectionRange?: IRange }).targetSelectionRange ?? found.range;
        const selection = { start: { line: range.startLineNumber - 1, character: range.startColumn - 1 }, end: { line: range.endLineNumber - 1, character: range.endColumn - 1 } };
        const widget = await this.editors.open(new URI(found.uri.toString()), { mode: 'activate', selection });
        widget.editor.revealRange(selection, { at: 'center' });
        return true;
    }

    /** The language server's hover for what is at `line`, `column` of the head file. */
    hover(review: Review, file: string, line: number, column: number): Promise<CodeHover | undefined> {
        return this.ask(review, file, line, column, async (model, position) => {
            for (const provider of StandaloneServices.get(ILanguageFeaturesService).hoverProvider.ordered(model)) {
                const hover = await provider.provideHover(model, position, CancellationToken.None);
                const contents = hover?.contents.map(c => c.value).filter(v => v.trim());
                if (contents?.length) {
                    return { contents };
                }
            }
            return undefined;
        });
    }

    /** Loads the head file as a model (which starts its language server, as opening it would) and asks. */
    protected async ask<T>(review: Review, file: string, line: number, column: number,
        question: (model: ITextModel, position: Position) => Promise<T | undefined>): Promise<T | undefined> {
        const uri = this.headUri(review, file);
        if (!uri) {
            return undefined;
        }
        const reference = await this.models.createModelReference(uri);
        try {
            const model = reference.object.textEditorModel as unknown as ITextModel;
            return await question(model, new Position(line, column + 1));
        } catch {
            return undefined;
        } finally {
            // Kept briefly, so a hover followed by a click doesn't load the file twice.
            setTimeout(() => reference.dispose(), 30_000);
        }
    }
}

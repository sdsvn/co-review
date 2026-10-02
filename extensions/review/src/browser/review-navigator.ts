import URI from '@theia/core/lib/common/uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { EditorManager } from '@theia/editor/lib/browser/editor-manager';
import { FileNavigatorContribution } from '@theia/navigator/lib/browser/navigator-contribution';
import { CodeLocation } from '../common/review-model';
import { ReviewManager } from './review-manager';
import { fromMonacoRange, ReviewLocations } from './review-locations';
import { MonacoEditor } from '@theia/monaco/lib/browser/monaco-editor';
import { OpenerService } from '@theia/core/lib/browser/opener-service';
import { ReviewChange } from './review-change';
import { CodeNavigation } from './review-code';

/** Id of the document review opener (kept here to avoid an import cycle with the document widget). */
const DOCUMENT_OPENER_ID = 'co-review:document';
const PATCH_OPENER_ID = 'co-review:patch';

/**
 * Opens review locations using Theia's regular editor and explorer, so review navigation
 * and IDE navigation are the same mechanism.
 */
@injectable()
export class ReviewNavigator {

    @inject(EditorManager) protected readonly editorManager: EditorManager;
    @inject(FileNavigatorContribution) protected readonly fileNavigator: FileNavigatorContribution;
    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(ReviewLocations) protected readonly locations: ReviewLocations;
    @inject(OpenerService) protected readonly openers: OpenerService;
    @inject(CodeNavigation) protected readonly code: CodeNavigation;
    @inject(ReviewChange) protected readonly change: ReviewChange;

    /** Opens an agent-provided `path:line[-end]` reference (relative to the repository, or absolute). */
    async openReference(path: string, line?: number, endLine?: number): Promise<void> {
        const root = this.reviews.root;
        const uri = path.startsWith('/') || !root ? URI.fromFilePath(path) : new URI(root).resolve(path);
        const range = line ? { start: { line: line - 1, character: 0 }, end: { line: (endLine ?? line) - 1, character: 0 } } : undefined;
        const widget = await this.editorManager.open(uri, { mode: 'activate', selection: range });
        if (range) {
            widget.editor.revealRange(range, { at: 'center' });
        }
    }

    /**
     * A file of the change, as the change: modified, base ↔ head side by side (at a line of either side); added, the
     * file; deleted, its base. Comments on the diff show on the head side, where they are placed (see ReviewManager).
     */
    async openChangedFile(path: string, line?: number, side: 'new' | 'old' = 'new'): Promise<void> {
        const review = this.reviews.activeReview;
        if (!review) {
            return;
        }
        const tag = this.change.file(path)?.tag;
        if (tag === 'deleted') {
            await this.code.openBase(review, path, line ?? 1);
        } else if (tag === 'new file') {
            await this.code.openHead(review, path, line ?? 1);
        } else {
            await this.code.compare(review, path, line, side);
        }
    }

    /** `onPage`: a location on the patch page opens the page (a draft written there is only shown there). */
    async open(location: CodeLocation, threadId?: string, onPage = false): Promise<void> {
        // A comment on a new-side line of the diff opens that file's diff, where it is shown inline. One on an old-side
        // line or on a whole file opens on the patch page, the one place that shows it.
        const file = onPage ? undefined : this.reviews.patchFile(location);
        if (file?.line && file.side === 'new') {
            await this.openChangedFile(file.path, file.line);
            return;
        }
        // Rendered pages are Markdown; a document thread on anything else (e.g. an HTML file) opens in the editor.
        const rendered = location.kind === 'patch' || (location.kind === 'document' && /\.(md|markdown)$/i.test(location.uri ?? ''));
        if (rendered && location.uri) {
            const openerId = location.kind === 'document' ? DOCUMENT_OPENER_ID : PATCH_OPENER_ID;
            const opener = (await this.openers.getOpeners()).find(o => o.id === openerId);
            const widget = await opener?.open(new URI(location.uri), { mode: 'activate' }) as { reveal?(id: string): void } | undefined;
            if (threadId) {
                setTimeout(() => widget?.reveal?.(threadId), 200);
            }
            return;
        }
        if (!location.uri) {
            await this.fileNavigator.openView({ activate: true, reveal: true });
            return;
        }
        const uri = new URI(location.uri);
        if (location.kind === 'directory') {
            await this.fileNavigator.openView({ activate: false, reveal: true });
            await this.fileNavigator.selectFileNode(uri);
            return;
        }
        const widget = await this.editorManager.open(uri, { mode: 'activate' });
        const editor = MonacoEditor.get(widget);
        const model = editor?.getControl().getModel();
        if (!editor || !model || !location.range) {
            return;
        }
        const resolved = await this.locations.resolve(model, location);
        const range = resolved ? fromMonacoRange(resolved.range) : location.range;
        editor.revealRange(range, { at: 'center' });
        editor.cursor = range.start;
        editor.focus();
    }
}

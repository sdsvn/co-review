import * as React from '@theia/core/shared/react';
import { FrontendApplicationContribution } from '@theia/core/lib/browser/frontend-application-contribution';
import { DisposableCollection } from '@theia/core/lib/common/disposable';
import { CommandService } from '@theia/core/lib/common/command';
import { inject, injectable } from '@theia/core/shared/inversify';
import { ReviewCommands } from './review-commands';
import { ReviewNavigator } from './review-navigator';
import { EditorManager } from '@theia/editor/lib/browser/editor-manager';
import { EditorWidget } from '@theia/editor/lib/browser/editor-widget';
import { MonacoEditor } from '@theia/monaco/lib/browser/monaco-editor';
import * as monaco from '@theia/monaco-editor-core';
import { CodeLocation, ReviewThread, ThreadIntent } from '../common/review-model';
import { DraftEditor, ThreadView } from './review-components';
import { InlineZone } from './review-inline-zone';
import { ReviewDraft, ReviewManager } from './review-manager';
import { AnchorState, fromMonacoRange, ReviewLocations } from './review-locations';

function escapeMarkdown(text: string): string {
    return text.replace(/[\\`*_{}\[\]()#+\-.!|<>]/g, '\\$&');
}

/** A selection ending at column 1 of the next line means "up to the end of the previous line". */
function lastLine(range: monaco.IRange): number {
    return range.endColumn === 1 && range.endLineNumber > range.startLineNumber ? range.endLineNumber - 1 : range.endLineNumber;
}

/**
 * Brings the review into the editor, GitHub-style:
 * - a `+` button in the gutter of the hovered line (click, or drag across lines) opens an inline draft;
 * - threads and drafts are shown inline under the code they are about, with reply/resolve;
 * - gutter glyphs, highlights and hovers mark commented code;
 * - thread locations are kept attached to the code when it moves.
 */
@injectable()
export class ReviewEditorDecorator implements FrontendApplicationContribution {

    @inject(EditorManager) protected readonly editorManager: EditorManager;
    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(ReviewLocations) protected readonly locations: ReviewLocations;
    @inject(CommandService) protected readonly commands: CommandService;
    @inject(ReviewNavigator) protected readonly navigator: ReviewNavigator;

    /** Resolved threads shown inline even though they are resolved. */
    protected readonly shownResolved = new Set<string>();
    protected readonly pendingMoves = new Set<string>();

    onStart(): void {
        this.editorManager.all.forEach(widget => this.track(widget));
        this.editorManager.onCreated(widget => this.track(widget));
    }

    isShownInline(thread: ReviewThread): boolean {
        return thread.status === 'open' ? !this.reviews.isCollapsed(thread.id) : this.shownResolved.has(thread.id);
    }

    setShownInline(thread: ReviewThread, shown: boolean): void {
        if (thread.status === 'open') {
            this.reviews.setCollapsed(thread.id, !shown);
        } else {
            if (shown) {
                this.shownResolved.add(thread.id);
            } else {
                this.shownResolved.delete(thread.id);
            }
            this.reviews.setCollapsed(thread.id, false); // fires a change
        }
    }

    protected readonly tracked = new WeakSet<EditorWidget>();

    /** Folds every inline conversation (the gutter icons and the panel keep them reachable). */
    collapseAll(): void {
        this.shownResolved.clear();
        this.reviews.setCollapsedMany(this.reviews.activeReview?.threads.map(t => t.id) ?? [], true);
    }

    /** Shows every open conversation inline. */
    expandAll(): void {
        this.reviews.setCollapsedMany(this.reviews.activeReview?.threads.map(t => t.id) ?? [], false);
    }

    /** Moves the cursor to the next/previous commented line in the current editor and shows that thread. */
    goToComment(direction: 1 | -1): void {
        const editor = MonacoEditor.get(this.editorManager.currentEditor);
        const model = editor?.getControl().getModel();
        if (!editor || !model) {
            return;
        }
        const lines = new Map<number, string>();
        for (const d of model.getAllDecorations()) {
            const id = d.options.glyphMarginClassName?.match(/co-review-thread-([\w-]+)/)?.[1];
            if (id) {
                lines.set(d.range.startLineNumber, id);
            }
        }
        const sorted = [...lines.keys()].sort((a, b) => a - b);
        if (!sorted.length) {
            return;
        }
        const current = editor.getControl().getPosition()?.lineNumber ?? 0;
        const target = direction > 0
            ? sorted.find(l => l > current) ?? sorted[0]
            : [...sorted].reverse().find(l => l < current) ?? sorted[sorted.length - 1];
        const thread = this.reviews.activeReview?.threads.find(t => t.id === lines.get(target));
        if (thread) {
            this.setShownInline(thread, true);
        }
        editor.getControl().setPosition({ lineNumber: target, column: 1 });
        editor.getControl().revealLineInCenterIfOutsideViewport(target);
        editor.focus();
    }

    protected track(widget: EditorWidget): void {
        const editor = MonacoEditor.get(widget);
        // Restored editors are both in `all` and reported by `onCreated`.
        if (!editor || this.tracked.has(widget)) {
            return;
        }
        this.tracked.add(widget);
        const control = editor.getControl();
        control.updateOptions({ glyphMargin: true, lineDecorationsWidth: 18 });
        const decorations = control.createDecorationsCollection();
        const hoverDecoration = control.createDecorationsCollection();
        const zones = new Map<string, InlineZone>();
        const toDispose = new DisposableCollection();
        let timer: number | undefined;
        let generation = 0;

        const refresh = async () => {
            const model = control.getModel();
            if (!model) {
                return;
            }
            const current = ++generation;
            const uri = editor.uri.toString();
            const threads = this.reviews.threadsForUri(uri).filter(t => t.location.range);
            const drafts = this.reviews.draftsForUri(uri).filter(d => d.location.range);
            const [resolvedThreads, resolvedDrafts] = await Promise.all([
                Promise.all(threads.map(async thread => ({ thread, resolved: await this.locations.resolve(model, thread.location) }))),
                Promise.all(drafts.map(async draft => ({ draft, resolved: await this.locations.resolve(model, draft.location) })))
            ]);
            if (current !== generation || model.isDisposed()) {
                return;
            }
            const next: monaco.editor.IModelDeltaDecoration[] = [];
            const wanted = new Set<string>();
            for (const { thread, resolved } of resolvedThreads) {
                if (!resolved) {
                    continue;
                }
                this.reviews.setAnchorState(thread.id, resolved.state);
                next.push(...this.decorationsFor(thread, resolved.range, resolved.state));
                if (resolved.state === 'moved' && !editor.document.dirty) {
                    this.persistMove(thread, model, resolved.range);
                }
                if (this.isShownInline(thread)) {
                    const key = `thread:${thread.id}`;
                    wanted.add(key);
                    const zone = zones.get(key) ?? new InlineZone(control);
                    zones.set(key, zone);
                    zone.place(lastLine(resolved.range));
                    zone.render(this.renderThread(thread, resolved.state));
                }
            }
            for (const { draft, resolved } of resolvedDrafts) {
                if (!resolved) {
                    continue;
                }
                next.push({ range: resolved.range, options: { className: 'co-review-draft-range', isWholeLine: draft.location.kind !== 'range' } });
                const key = `draft:${draft.id}`;
                wanted.add(key);
                const isNew = !zones.has(key);
                const zone = zones.get(key) ?? new InlineZone(control);
                zones.set(key, zone);
                zone.place(lastLine(resolved.range));
                zone.render(this.renderDraft(draft));
                if (isNew) {
                    control.revealLineInCenterIfOutsideViewport(lastLine(resolved.range) + 1);
                    this.reviews.focusDraft(draft.id);
                }
            }
            for (const [key, zone] of zones) {
                if (!wanted.has(key)) {
                    zone.dispose();
                    zones.delete(key);
                }
            }
            decorations.set(next);
        };
        const schedule = (delay: number) => {
            window.clearTimeout(timer);
            timer = window.setTimeout(refresh, delay);
        };

        toDispose.push(this.reviews.onDidChange(() => schedule(30)));
        toDispose.push(this.reviews.onDidChangeDrafts(() => schedule(0)));
        toDispose.push(this.reviews.onDidChangeCollapsed(() => schedule(0)));
        toDispose.push(this.reviews.onDidRequestFocusDraft(() => schedule(0)));
        toDispose.push(control.onDidChangeModelContent(() => schedule(400)));
        toDispose.push(control.onDidChangeModel(() => schedule(0)));
        toDispose.push(editor.document.onDidSaveModel(() => schedule(0)));
        this.trackAddButton(editor, hoverDecoration, toDispose);
        toDispose.push(control.onMouseDown(e => {
            if (e.target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN && e.target.element?.className.includes('co-review-glyph')) {
                const thread = this.threadAtLine(editor, e.target.position?.lineNumber ?? -1);
                if (thread) {
                    this.setShownInline(thread, !this.isShownInline(thread));
                    this.reviews.revealThread(thread.id);
                }
            }
        }));
        toDispose.push({
            dispose: () => {
                window.clearTimeout(timer);
                decorations.clear();
                hoverDecoration.clear();
                zones.forEach(z => z.dispose());
                zones.clear();
            }
        });
        widget.disposed.connect(() => toDispose.dispose());
        schedule(0);
    }

    /** GitHub-style `+` in the gutter of the hovered line; click for one line, drag for several. */
    protected trackAddButton(editor: MonacoEditor, hover: monaco.editor.IEditorDecorationsCollection, toDispose: DisposableCollection): void {
        const control = editor.getControl();
        let hoveredLine = -1;
        let dragFrom: number | undefined;
        let dragTo = -1;

        const showPlus = (line: number) => {
            if (line === hoveredLine && dragFrom === undefined) {
                return;
            }
            hoveredLine = line;
            const decorations: monaco.editor.IModelDeltaDecoration[] = [];
            if (dragFrom !== undefined) {
                const [from, to] = [Math.min(dragFrom, dragTo), Math.max(dragFrom, dragTo)];
                decorations.push({ range: new monaco.Range(from, 1, to, 1), options: { isWholeLine: true, className: 'co-review-drag-range' } });
                decorations.push({ range: new monaco.Range(dragTo, 1, dragTo, 1), options: { linesDecorationsClassName: 'co-review-add codicon codicon-add' } });
            } else if (line > 0) {
                decorations.push({ range: new monaco.Range(line, 1, line, 1), options: { linesDecorationsClassName: 'co-review-add codicon codicon-add' } });
            }
            hover.set(decorations);
        };
        const finish = () => {
            if (dragFrom === undefined) {
                return;
            }
            const [from, to] = [Math.min(dragFrom, dragTo), Math.max(dragFrom, dragTo)];
            dragFrom = undefined;
            showPlus(hoveredLine);
            this.addDraftForLines(editor, from, to);
        };

        toDispose.push(control.onMouseMove(e => {
            const line = e.target.position?.lineNumber ?? -1;
            if (dragFrom !== undefined && line > 0) {
                dragTo = line;
                showPlus(line);
                return;
            }
            const overLine = e.target.type === monaco.editor.MouseTargetType.CONTENT_TEXT
                || e.target.type === monaco.editor.MouseTargetType.CONTENT_EMPTY
                || e.target.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS
                || e.target.type === monaco.editor.MouseTargetType.GUTTER_LINE_DECORATIONS;
            showPlus(overLine ? line : -1);
        }));
        // While a button is held Monaco doesn't report moves; follow the pointer ourselves.
        const onDrag = (e: MouseEvent) => {
            const line = control.getTargetAtClientPoint(e.clientX, e.clientY)?.position?.lineNumber;
            if (dragFrom !== undefined && line) {
                dragTo = line;
                showPlus(line);
            }
        };
        toDispose.push(control.onMouseLeave(() => dragFrom === undefined && showPlus(-1)));
        toDispose.push(control.onMouseDown(e => {
            if (e.target.type === monaco.editor.MouseTargetType.GUTTER_LINE_DECORATIONS && e.target.element?.className.includes('co-review-add')
                && e.target.position) {
                e.event.preventDefault();
                e.event.stopPropagation();
                dragFrom = dragTo = e.target.position.lineNumber;
                showPlus(dragTo);
                // Capture phase: Monaco's own drag monitor stops these events from bubbling.
                window.addEventListener('mousemove', onDrag, true);
                window.addEventListener('mouseup', () => {
                    window.removeEventListener('mousemove', onDrag, true);
                    finish();
                }, { once: true, capture: true });
            }
        }));
    }

    /**
     * Opens a draft for the given lines. When the click is inside a non-empty selection, the
     * selection is used. Several lines default to asking the agent.
     */
    protected async addDraftForLines(editor: MonacoEditor, from: number, to: number): Promise<void> {
        const control = editor.getControl();
        const model = control.getModel();
        if (!model) {
            return;
        }
        const selection = control.getSelection();
        let range: monaco.IRange;
        let kind: CodeLocation['kind'];
        if (from === to && selection && !selection.isEmpty() && selection.startLineNumber <= from && from <= lastLine(selection)) {
            const end = lastLine(selection);
            range = new monaco.Range(selection.startLineNumber, selection.startColumn, end, end === selection.endLineNumber ? selection.endColumn : model.getLineMaxColumn(end));
            kind = 'range';
        } else {
            range = new monaco.Range(from, 1, to, model.getLineMaxColumn(to));
            kind = from === to ? 'line' : 'range';
        }
        if (!this.reviews.activeReview && !await this.commands.executeCommand<boolean>(ReviewCommands.CREATE_REVIEW.id)) {
            return;
        }
        const symbol = await this.locations.findEnclosingSymbol(model, range.startLineNumber);
        const intent: ThreadIntent = lastLine(range) > range.startLineNumber ? 'question' : 'comment';
        this.reviews.addDraft({
            kind, uri: editor.uri.toString(), range: fromMonacoRange(range), symbol: symbol?.path,
            anchor: await this.locations.captureAnchor(model, range)
        }, intent);
    }

    protected renderThread(thread: ReviewThread, state: AnchorState): React.ReactElement {
        return <ThreadView manager={this.reviews} thread={thread} inline
            label={this.reviews.locationLabel(thread.location)}
            anchorState={state}
            onHide={() => this.setShownInline(thread, false)}
            onOpenReference={(path, line, end) => this.navigator.openReference(path, line, end)} />;
    }

    protected renderDraft(draft: ReviewDraft): React.ReactElement {
        return <DraftEditor manager={this.reviews} draft={draft} acceptFocus
            label={this.reviews.locationLabel(draft.location)} />;
    }

    protected threadAtLine(editor: MonacoEditor, line: number): ReviewThread | undefined {
        const model = editor.getControl().getModel();
        if (!model) {
            return undefined;
        }
        const ids = model.getLineDecorations(line)
            .map(d => d.options.glyphMarginClassName?.match(/co-review-thread-([\w-]+)/)?.[1])
            .filter(Boolean);
        return this.reviews.activeReview?.threads.find(t => ids.includes(t.id));
    }

    protected decorationsFor(thread: ReviewThread, range: monaco.IRange, state: AnchorState): monaco.editor.IModelDeltaDecoration[] {
        const resolved = thread.status === 'resolved';
        const parts = [`**Thread #${thread.number}** · ${thread.status}${state !== 'exact' ? ` · ${state}` : ''}`];
        for (const message of thread.messages.slice(-3)) {
            const body = message.body.length > 300 ? message.body.slice(0, 300) + '…' : message.body;
            parts.push(`**${escapeMarkdown(message.author.name)}**: ${escapeMarkdown(body)}`);
        }
        if (thread.messages.length > 3) {
            parts.unshift(`_${thread.messages.length - 3} earlier message(s)_`);
        }
        parts.push(this.isShownInline(thread) ? '_Click the gutter icon to collapse_' : '_Click the gutter icon to show the conversation_');
        const hover: monaco.IMarkdownString = { value: parts.join('\n\n') };
        const glyphClass = `co-review-glyph codicon ${resolved ? 'codicon-pass' : 'codicon-comment-discussion'} co-review-thread-${thread.id}`;
        return [
            {
                range: new monaco.Range(range.startLineNumber, 1, range.startLineNumber, 1),
                options: { glyphMarginClassName: glyphClass, glyphMarginHoverMessage: hover, stickiness: monaco.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges }
            },
            ...(resolved ? [] : [{
                range,
                options: {
                    className: thread.location.kind === 'range' ? 'co-review-range' : undefined,
                    isWholeLine: thread.location.kind !== 'range',
                    linesDecorationsClassName: 'co-review-lines',
                    hoverMessage: hover,
                    overviewRuler: { color: 'rgba(230, 170, 50, 0.8)', position: monaco.editor.OverviewRulerLane.Left }
                }
            }])
        ];
    }

    /** Persist a relocated thread so that the comment follows the code across sessions. */
    protected async persistMove(thread: ReviewThread, model: monaco.editor.ITextModel, range: monaco.IRange): Promise<void> {
        if (this.pendingMoves.has(thread.id)) {
            return;
        }
        this.pendingMoves.add(thread.id);
        try {
            const anchor = thread.location.kind === 'symbol' ? thread.location.anchor : await this.locations.captureAnchor(model, range);
            await this.reviews.relocate(thread, { ...thread.location, range: fromMonacoRange(range), anchor });
        } finally {
            this.pendingMoves.delete(thread.id);
        }
    }
}

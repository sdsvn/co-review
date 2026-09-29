import * as React from '@theia/core/shared/react';
import { Message, Navigatable, codicon } from '@theia/core/lib/browser';
import { ReactWidget } from '@theia/core/lib/browser/widgets/react-widget';
import URI from '@theia/core/lib/common/uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import * as markdownit from '@theia/core/shared/markdown-it';
import * as DOMPurify from '@theia/core/shared/dompurify';
import { FileService } from '@theia/filesystem/lib/browser/file-service';
import { PatchAnchor, ReviewThread } from '../../common/review-model';
import { parsePatch, parsePatchMeta, PatchFile, PatchMeta, PatchRow } from '../../common/patch';
import { DraftEditor, ThreadView } from '../review-components';
import { ReviewDraft, ReviewManager } from '../review-manager';
import { ReviewNavigator } from '../review-navigator';
import { CodeHover, CodeNavigation } from '../review-code';

export const PatchReviewWidgetOptions = Symbol('PatchReviewWidgetOptions');
export interface PatchReviewWidgetOptions {
    uri: string;
}

const md = markdownit({ html: false, linkify: true });
const LARGE_FILE_ROWS = 300;

type Item = { key: string; anchor: PatchAnchor; thread?: ReviewThread; draft?: ReviewDraft };

function rowSide(row: PatchRow): 'new' | 'old' {
    return row.t === 'del' ? 'old' : 'new';
}

function rowLine(row: PatchRow): number | undefined {
    return row.t === 'del' ? row.o : row.n;
}

/** Whether a patch anchor ends on this row (threads are shown after the last line they cover). */
function endsAt(anchor: PatchAnchor, path: string, row: PatchRow): boolean {
    if (anchor.path !== path || row.t === 'hunk' || (anchor.side ?? 'new') !== rowSide(row)) {
        return false;
    }
    const line = rowLine(row);
    return anchor.type === 'code-line' ? anchor.line === line : anchor.type === 'code-range' && anchor.endLine === line;
}

function covers(anchor: PatchAnchor, path: string, row: PatchRow): boolean {
    if (anchor.path !== path || row.t === 'hunk' || (anchor.side ?? 'new') !== rowSide(row)) {
        return false;
    }
    const line = rowLine(row) ?? -1;
    return anchor.type === 'code-line' ? anchor.line === line
        : anchor.type === 'code-range' && (anchor.startLine ?? 0) <= line && line <= (anchor.endLine ?? 0);
}

/**
 * Review page for a `.patch` / `.diff`: files with line, range, file
 * and patch comments inline, PR metadata, and pre-seeded findings to accept or dismiss.
 */
@injectable()
export class PatchReviewWidget extends ReactWidget implements Navigatable {

    static readonly FACTORY_ID = 'co-review:patch';

    @inject(PatchReviewWidgetOptions) protected readonly options: PatchReviewWidgetOptions;
    @inject(FileService) protected readonly fileService: FileService;
    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(ReviewNavigator) protected readonly navigator: ReviewNavigator;
    @inject(CodeNavigation) protected readonly code: CodeNavigation;

    protected files: PatchFile[] = [];
    protected meta: PatchMeta | undefined;
    protected error: string | undefined;
    protected collapsed = new Set<string>();
    protected rangeStart: { path: string; row: PatchRow } | undefined;
    /** The language server's hover for the name under the pointer (new-side lines of a review with code). */
    protected hover: (CodeHover & { x: number; y: number }) | undefined;
    protected hoverTimer: number | undefined;

    get uri(): URI {
        return new URI(this.options.uri);
    }

    getResourceUri(): URI {
        return this.uri;
    }

    createMoveToUri(resourceUri: URI): URI {
        return resourceUri;
    }

    get slug(): string {
        return this.uri.path.base.replace(/\.(patch|diff)$/, '');
    }

    init(): void {
        this.id = `${PatchReviewWidget.FACTORY_ID}:${this.options.uri}`;
        this.title.label = `Review: ${this.uri.path.base}`;
        this.title.caption = this.uri.path.fsPath();
        this.title.iconClass = codicon('git-pull-request');
        this.title.closable = true;
        this.addClass('co-review-patch');
        this.node.tabIndex = 0;
        const dir = this.uri.parent;
        this.toDispose.push(this.fileService.onDidFilesChange(e => {
            if (e.contains(this.uri) || e.contains(dir.resolve('PR.md')) || e.contains(dir.resolve(`${this.slug}.md`))) {
                this.load();
            }
        }));
        // A review directory outside the window's folder (a review with code) isn't under the workspace's watcher.
        this.reviews.ready.then(() => {
            const root = this.reviews.root;
            if (!this.isDisposed && (!root || !new URI(root).isEqualOrParent(this.uri))) {
                this.toDispose.push(this.fileService.watch(dir));
            }
        });
        this.toDispose.push(this.reviews.onDidChange(() => this.update()));
        this.toDispose.push(this.reviews.onDidChangeDrafts(() => this.update()));
        this.toDispose.push(this.reviews.onDidChangeCollapsed(() => this.update()));
        this.load();
    }

    protected override onActivateRequest(msg: Message): void {
        super.onActivateRequest(msg);
        if (!this.node.contains(document.activeElement)) {
            this.node.focus();
        }
    }

    protected async load(): Promise<void> {
        try {
            this.files = parsePatch((await this.fileService.read(this.uri)).value);
            this.error = undefined;
        } catch (e) {
            this.error = `Cannot read ${this.uri.path.base}: ${e}`;
        }
        this.meta = undefined;
        for (const name of [`${this.slug}.md`, 'PR.md']) {
            try {
                this.meta = parsePatchMeta((await this.fileService.read(this.uri.parent.resolve(name))).value);
                break;
            } catch {
                /* no metadata file */
            }
        }
        const items = this.items();
        for (const file of this.files) {
            if (file.rows.length > LARGE_FILE_ROWS && !items.some(i => i.anchor.path === file.path)) {
                this.collapsed.add(file.path);
            }
        }
        this.reviews.syncFindings();
        this.update();
    }

    protected items(): Item[] {
        const items: Item[] = [];
        for (const thread of this.reviews.activeReview?.threads ?? []) {
            if (thread.location.kind === 'patch' && thread.location.uri === this.options.uri && thread.location.patchAnchor) {
                items.push({ key: thread.id, anchor: thread.location.patchAnchor, thread });
            }
        }
        for (const draft of this.reviews.draftsForUri(this.options.uri)) {
            if (draft.location.kind === 'patch' && draft.location.patchAnchor) {
                items.push({ key: draft.id, anchor: draft.location.patchAnchor, draft });
            }
        }
        return items;
    }

    /** The review's code folder is there to step into (see CodeNavigation). */
    protected get withCode(): boolean {
        return this.code.has(this.reviews.activeReview);
    }

    /** Where in a row's code the pointer is: the 0-based column in the file's line, from the rendered text. */
    protected columnAt(e: React.MouseEvent): number | undefined {
        const caret = document.caretRangeFromPoint?.(e.clientX, e.clientY);
        const span = (e.target as HTMLElement).closest('.co-review-patch-code');
        if (!caret || !span || !span.contains(caret.startContainer)) {
            return undefined;
        }
        // The rendered text starts with the row's sign (+, − or a space).
        return Math.max(0, caret.startOffset - 1);
    }

    /** A line number: new side opens the head file there, old side the base version (read-only). */
    protected onLineNumber(path: string, row: PatchRow, side: 'old' | 'new'): void {
        const review = this.reviews.activeReview;
        const line = side === 'new' ? row.n : row.o;
        if (!this.code.has(review) || !line) {
            return;
        }
        if (side === 'new') {
            this.code.openHead(review, path, line);
        } else {
            this.code.openBase(review, path, line);
        }
    }

    /** Cmd/Ctrl-click on a name in a new-side line: its definition. */
    protected onCodeClick(e: React.MouseEvent, path: string, row: PatchRow): void {
        const review = this.reviews.activeReview;
        const column = this.columnAt(e);
        if (!(e.metaKey || e.ctrlKey) || !this.code.has(review) || row.t === 'del' || row.t === 'hunk' || !row.n || column === undefined) {
            return;
        }
        e.preventDefault();
        this.code.goToDefinition(review, path, row.n, column).then(async found => {
            if (!found) {
                // No language server answer: the name in its file, where the editor can try.
                await this.code.openHead(review, path, row.n!, column);
            }
        });
    }

    /** Resting on a name in a new-side line shows what the language server knows about it. */
    protected onCodeHover(e: React.MouseEvent, path: string, row: PatchRow): void {
        window.clearTimeout(this.hoverTimer);
        const review = this.reviews.activeReview;
        const column = this.columnAt(e);
        const { clientX: x, clientY: y } = e;
        if (!this.code.has(review) || row.t === 'del' || row.t === 'hunk' || !row.n || column === undefined || !/\w/.test(row.s[column] ?? '')) {
            return;
        }
        this.hoverTimer = window.setTimeout(async () => {
            const found = await this.code.hover(review, path, row.n!, column);
            this.hover = found && { ...found, x, y };
            this.update();
        }, 700);
    }

    protected hideHover(): void {
        window.clearTimeout(this.hoverTimer);
        if (this.hover) {
            this.hover = undefined;
            this.update();
        }
    }

    protected addDraft(anchor: PatchAnchor): void {
        const multiLine = anchor.type === 'code-range';
        this.reviews.addDraft({ kind: 'patch', uri: this.options.uri, patchAnchor: anchor, anchor: { text: anchor.source ?? '' } }, multiLine ? 'question' : 'comment');
    }

    protected onRowClick(e: React.MouseEvent, path: string, row: PatchRow): void {
        const line = rowLine(row);
        if (line === undefined) {
            return;
        }
        const start = this.rangeStart;
        if (e.shiftKey && start && start.path === path && rowSide(start.row) === rowSide(row) && rowLine(start.row) !== line) {
            const file = this.files.find(f => f.path === path)!;
            const [a, b] = [rowLine(start.row)!, line].sort((x, y) => x - y);
            const source = file.rows.filter(r => r.t !== 'hunk' && rowSide(r) === rowSide(row) && (rowLine(r) ?? -1) >= a && (rowLine(r) ?? -1) <= b).map(r => r.s).join('\n');
            this.addDraft({ type: 'code-range', path, startLine: a, endLine: b, side: rowSide(row), source });
            this.rangeStart = undefined;
        } else {
            this.rangeStart = { path, row };
            this.addDraft({ type: 'code-line', path, line, side: rowSide(row), source: row.s });
        }
    }

    protected renderItem(item: Item): React.ReactNode {
        if (item.draft) {
            return <div key={item.key} className='co-review-inline-content co-review-patch-slot'>
                <DraftEditor manager={this.reviews} draft={item.draft} acceptFocus label={PatchAnchor.describe(item.anchor)} />
            </div>;
        }
        const thread = item.thread!;
        if (thread.status === 'resolved' || (thread.status === 'open' && this.reviews.isCollapsed(thread.id))) {
            return undefined;
        }
        return <div key={item.key} className='co-review-inline-content co-review-patch-slot'>
            <ThreadView manager={this.reviews} thread={thread} inline label={PatchAnchor.describe(item.anchor)}
                onHide={() => this.reviews.setCollapsed(thread.id, true)}
                onOpenReference={(path, line, end) => this.navigator.openReference(path, line, end)} />
        </div>;
    }

    protected render(): React.ReactNode {
        if (this.error) {
            return <div className='co-review-patch-root'><div className='co-review-hint'>{this.error}</div></div>;
        }
        const items = this.items();
        const meta = this.meta;
        const added = this.files.reduce((n, f) => n + f.added, 0);
        const removed = this.files.reduce((n, f) => n + f.removed, 0);
        return <div className='co-review-patch-root'>
            <div className='co-review-patch-head'>
                <div className='co-review-patch-title'>
                    <span className={codicon('git-pull-request')} />
                    <span>{meta?.title ?? this.slug}</span>
                    {meta?.number && <span className='co-review-muted'>#{meta.number}</span>}
                    <span className='co-review-spacer' />
                    <span className={`${codicon('comment')} action-label`} title='Comment on the patch' onClick={() => this.addDraft({ type: 'patch' })} />
                    <span className={`${codicon('collapse-all')} action-label`} title='Collapse all files'
                        onClick={() => { this.files.forEach(f => this.collapsed.add(f.path)); this.update(); }} />
                </div>
                <div className='co-review-muted'>
                    {meta?.branch && <span>{meta.branch}{meta.base ? ` → ${meta.base}` : ''} · </span>}
                    {meta?.commits && <span>{meta.commits} commit{meta.commits === 1 ? '' : 's'} · </span>}
                    <span>{this.files.length} file{this.files.length === 1 ? '' : 's'} </span>
                    <span className='co-review-add-count'>+{added}</span> <span className='co-review-del-count'>−{removed}</span>
                </div>
                {meta?.body && <div className='co-review-patch-body' dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(md.render(meta.body)) }} />}
            </div>
            {items.filter(i => i.anchor.type === 'patch').map(i => this.renderItem(i))}
            {this.files.map(file => this.renderFile(file, items))}
            {this.hover && <div className='co-review-code-hover' style={{ left: this.hover.x + 8, top: this.hover.y + 14 }}
                onMouseLeave={() => this.hideHover()}
                dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(md.render(this.hover.contents.join('\n\n---\n\n'))) }} />}
        </div>;
    }

    protected renderFile(file: PatchFile, items: Item[]): React.ReactNode {
        const collapsed = this.collapsed.has(file.path);
        const fileItems = items.filter(i => i.anchor.path === file.path);
        const toggle = () => {
            if (collapsed) {
                this.collapsed.delete(file.path);
            } else {
                this.collapsed.add(file.path);
            }
            this.update();
        };
        const withCode = this.withCode;
        return <div key={file.path} className='co-review-patch-file'>
            <div className='co-review-patch-file-head' onClick={toggle}>
                <span className={codicon(collapsed ? 'chevron-right' : 'chevron-down')} />
                <span className='co-review-patch-path'>{file.path}</span>
                {file.tag !== 'modified' && <span className='co-review-badge'>{file.tag}</span>}
                <span className='co-review-add-count'>+{file.added}</span><span className='co-review-del-count'>−{file.removed}</span>
                {fileItems.length > 0 && <span className='co-review-muted'><span className={codicon('comment-discussion')} /> {fileItems.length}</span>}
                <span className='co-review-spacer' />
                {this.withCode && file.tag !== 'deleted' && <span className={`${codicon('go-to-file')} action-label`} title='Open the file'
                    onClick={e => { e.stopPropagation(); this.code.openHead(this.reviews.activeReview!, file.path, file.rows.find(r => r.n)?.n ?? 1); }} />}
                {this.withCode && file.tag === 'modified' && <span className={`${codicon('diff')} action-label`} title='Compare: base and head side by side'
                    onClick={e => { e.stopPropagation(); this.code.compare(this.reviews.activeReview!, file.path); }} />}
                <span className={`${codicon('comment')} action-label`} title='Comment on this file'
                    onClick={e => { e.stopPropagation(); this.addDraft({ type: 'code-file', path: file.path }); }} />
            </div>
            {!collapsed && <>
                {fileItems.filter(i => i.anchor.type === 'code-file').map(i => this.renderItem(i))}
                <div className='co-review-patch-rows'>
                    {file.rows.map((row, index) => {
                        const commented = fileItems.some(i => covers(i.anchor, file.path, row));
                        return <React.Fragment key={index}>
                            <div className={`co-review-patch-row ${row.t} ${commented ? 'commented' : ''}`}>
                                <span className={`co-review-patch-no ${withCode && row.o ? 'link' : ''}`} title={withCode && row.o ? 'Open the base version here' : undefined}
                                    onClick={() => this.onLineNumber(file.path, row, 'old')}>{row.o ?? ''}</span>
                                <span className={`co-review-patch-no ${withCode && row.n ? 'link' : ''}`} title={withCode && row.n ? 'Open the file here' : undefined}
                                    onClick={() => this.onLineNumber(file.path, row, 'new')}>{row.n ?? ''}</span>
                                <span className='co-review-patch-plus'>
                                    {row.t !== 'hunk' && <span className={codicon('add')} title='Comment (shift-click another line for a range)'
                                        onClick={e => this.onRowClick(e, file.path, row)} />}
                                </span>
                                <span className='co-review-patch-code'
                                    onClick={e => this.onCodeClick(e, file.path, row)}
                                    onMouseMove={e => this.onCodeHover(e, file.path, row)}
                                    onMouseLeave={() => window.clearTimeout(this.hoverTimer)}>
                                    {row.t === 'hunk' ? row.s : `${row.t === 'add' ? '+' : row.t === 'del' ? '−' : ' '}${row.s}`}</span>
                            </div>
                            {fileItems.filter(i => endsAt(i.anchor, file.path, row)).map(i => this.renderItem(i))}
                        </React.Fragment>;
                    })}
                </div>
            </>}
        </div>;
    }

    /** Scrolls a thread of this patch into view (expanding its file). */
    reveal(threadId: string): void {
        const thread = this.reviews.activeReview?.threads.find(t => t.id === threadId);
        const path = thread?.location.patchAnchor?.path;
        if (path) {
            this.collapsed.delete(path);
            this.update();
        }
        setTimeout(() => this.node.querySelector('.co-review-patch-row.commented, .co-review-patch-slot')?.scrollIntoView({ block: 'center', behavior: 'smooth' }), 100);
    }
}

import { DiffUris } from '@theia/core/lib/browser/diff-uris';
import { FrontendApplicationContribution } from '@theia/core/lib/browser/frontend-application-contribution';
import { codicon, Widget } from '@theia/core/lib/browser/widgets/widget';
import { TabBarToolbarContribution, TabBarToolbarRegistry } from '@theia/core/lib/browser/shell/tab-bar-toolbar';
import { Command, CommandContribution, CommandRegistry } from '@theia/core/lib/common/command';
import { OpenHandler, OpenerOptions } from '@theia/core/lib/browser/opener-service';
import { Emitter, Event } from '@theia/core/lib/common/event';
import { StorageService } from '@theia/core/lib/browser/storage-service';
import URI from '@theia/core/lib/common/uri';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { EditorManager } from '@theia/editor/lib/browser/editor-manager';
import { EditorWidget } from '@theia/editor/lib/browser/editor-widget';
import { FileStatNode } from '@theia/filesystem/lib/browser/file-tree/file-tree';
import { FileService } from '@theia/filesystem/lib/browser/file-service';
import { FileNavigatorFilter } from '@theia/navigator/lib/browser/navigator-filter';
import { FILE_NAVIGATOR_ID } from '@theia/navigator/lib/browser/navigator-widget';
import { FileNavigatorPreferences } from '@theia/navigator/lib/common/navigator-preferences';
import { MonacoEditor } from '@theia/monaco/lib/browser/monaco-editor';
import * as monaco from '@theia/monaco-editor-core';
import { parsePatch, PatchFile } from '../common/patch';
import { CodeNavigation } from './review-code';
import { ReviewManager } from './review-manager';

/** The files a change review's patches change, read once per change (a new head), not with every comment. */
@injectable()
export class ReviewChange {

    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(FileService) protected readonly fileService: FileService;

    protected readonly onDidChangeEmitter = new Emitter<void>();
    readonly onDidChange = this.onDidChangeEmitter.event;

    protected loadedFor: string | undefined;
    protected _files: PatchFile[] = [];

    get files(): readonly PatchFile[] {
        return this._files;
    }

    /** Whether the active review is a change with its code folder, so its files are in the explorer. */
    get hasCode(): boolean {
        return this._files.length > 0 && !!this.reviews.activeReview?.bundle?.code;
    }

    file(path: string): PatchFile | undefined {
        return this._files.find(f => f.path === path);
    }

    /** A URI's path relative to the active review's code folder (`''` for the folder itself). */
    relativePath(uri: URI): string | undefined {
        const code = this.reviews.activeReview?.bundle?.code;
        return code ? URI.fromFilePath(code).relative(uri)?.toString() : undefined;
    }

    /** The change's file at a URI (a head file of the active review), and its path relative to the code folder. */
    fileAt(uri: URI): { path: string; file: PatchFile } | undefined {
        const path = this.relativePath(uri);
        const file = path ? this.file(path) : undefined;
        return path && file ? { path, file } : undefined;
    }

    /** Whether a path is a file of the change on disk (not deleted), or a folder holding one. */
    touches(path: string): boolean {
        return this._files.some(f => f.tag !== 'deleted' && (f.path === path || path === '' || f.path.startsWith(path + '/')));
    }

    @postConstruct()
    protected init(): void {
        this.reviews.onDidChange(() => this.load());
        this.load();
    }

    protected async load(): Promise<void> {
        await this.reviews.ready;
        const review = this.reviews.activeReview;
        const key = review?.bundle ? `${review.id}:${review.bundle.head ?? ''}:${review.bundle.base ?? ''}` : undefined;
        if (key === this.loadedFor) {
            return;
        }
        this.loadedFor = key;
        const files: PatchFile[] = [];
        if (review?.bundle) {
            const dir = URI.fromFilePath(review.bundle.dir);
            const children = (await this.fileService.resolve(dir).catch(() => undefined))?.children ?? [];
            for (const child of children.filter(c => /\.(patch|diff)$/.test(c.name))) {
                const text = (await this.fileService.read(child.resource).catch(() => undefined))?.value ?? '';
                files.push(...parsePatch(text));
            }
        }
        if (key === this.loadedFor) {
            this._files = files;
            this.onDidChangeEmitter.fire();
        }
    }
}

/**
 * Shows the change on a changed file opened from the review, so its full text still reads as a diff: added lines on a
 * green background with a bar in the gutter, a red marker where lines were removed (hover for what they were), and both
 * in the scrollbar. The decorations are the patch's, on the head file as it is on disk.
 */
@injectable()
export class ReviewChangeDecorator implements FrontendApplicationContribution {

    @inject(EditorManager) protected readonly editors: EditorManager;
    @inject(ReviewChange) protected readonly change: ReviewChange;

    protected readonly tracked = new WeakSet<EditorWidget>();

    onStart(): void {
        this.editors.all.forEach(widget => this.track(widget));
        this.editors.onCreated(widget => this.track(widget));
    }

    protected track(widget: EditorWidget): void {
        const editor = MonacoEditor.get(widget);
        if (!editor || this.tracked.has(widget)) {
            return;
        }
        this.tracked.add(widget);
        const decorations = editor.getControl().createDecorationsCollection();
        const refresh = () => {
            // A diff editor shows the change itself.
            const file = DiffUris.isDiffUri(editor.uri) ? undefined : this.change.fileAt(editor.uri)?.file;
            decorations.set(file && file.tag !== 'deleted' ? this.decorationsFor(file) : []);
        };
        const listener = this.change.onDidChange(refresh);
        widget.disposed.connect(() => listener.dispose());
        refresh();
    }

    protected decorationsFor(file: PatchFile): monaco.editor.IModelDeltaDecoration[] {
        const result: monaco.editor.IModelDeltaDecoration[] = [];
        let removed: string[] = [];
        let lastNew = 0;
        const flushRemoved = (beforeLine: number) => {
            if (removed.length) {
                // Marked on the line that now follows them (or the last line, when they were at the end).
                const line = Math.max(1, beforeLine);
                const above = beforeLine > lastNew;
                result.push({
                    range: new monaco.Range(line, 1, line, 1),
                    options: {
                        isWholeLine: true,
                        className: above ? 'co-review-change-removed-above' : 'co-review-change-removed-below',
                        hoverMessage: { value: `**${removed.length} line${removed.length === 1 ? '' : 's'} removed**\n\n\`\`\`\n${removed.join('\n')}\n\`\`\`` },
                        overviewRuler: { color: { id: 'editorOverviewRuler.deletedForeground' }, position: monaco.editor.OverviewRulerLane.Left }
                    }
                });
                removed = [];
            }
        };
        for (const row of file.rows) {
            if (row.t === 'hunk') {
                flushRemoved(lastNew);
            } else if (row.t === 'del') {
                removed.push(row.s);
            } else if (row.n) {
                flushRemoved(row.n);
                lastNew = row.n;
                if (row.t === 'add') {
                    result.push({
                        range: new monaco.Range(row.n, 1, row.n, 1),
                        options: {
                            isWholeLine: true,
                            className: 'co-review-change-added',
                            overviewRuler: { color: { id: 'editorOverviewRuler.addedForeground' }, position: monaco.editor.OverviewRulerLane.Left },
                            minimap: { color: { id: 'minimapGutter.addedBackground' }, position: monaco.editor.MinimapPosition.Gutter }
                        }
                    });
                }
            }
        }
        flushRemoved(lastNew);
        return result;
    }
}

/**
 * Opening a file the change modified (from the explorer, Quick Open, a link) shows it as the change: base ↔ head side by
 * side, with the review's comments on the head side. Opening at a position (go to definition, a thread's link) and
 * "open the file" from the diff page still open the file itself.
 */
@injectable()
export class ReviewChangeOpenHandler implements OpenHandler {

    readonly id = 'co-review.change-diff';
    readonly label = 'Change (base ↔ head)';

    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(ReviewChange) protected readonly change: ReviewChange;
    @inject(CodeNavigation) protected readonly code: CodeNavigation;

    canHandle(uri: URI, options?: OpenerOptions & { selection?: unknown }): number {
        if (uri.scheme !== 'file' || options?.selection) {
            return 0;
        }
        const tag = this.change.fileAt(uri)?.file.tag;
        // Above the text editor (100); an added file has no base to compare, a deleted one no head.
        return tag === 'modified' || tag === 'renamed' ? 500 : 0;
    }

    async open(uri: URI, options?: OpenerOptions & { mode?: 'open' | 'reveal' | 'activate'; preview?: boolean }): Promise<object | undefined> {
        const review = this.reviews.activeReview;
        const at = this.change.fileAt(uri);
        if (review && at) {
            const first = at.file.rows.find(r => r.t === 'add' && r.n)?.n ?? at.file.rows.find(r => r.n)?.n;
            // As the text editor would: an explorer click previews (focus stays in the tree), Enter opens.
            return this.code.compare(review, at.path, first, 'new', { mode: options?.mode, preview: options?.preview });
        }
        return undefined;
    }
}

const CHANGED_ONLY_KEY = 'co-review.explorer.changedOnly';

/**
 * The explorer, narrowed to the change: with "Changed files only" on, it shows the files the change adds or modifies
 * and the folders holding them, and nothing else. It is the same tree (decorations, menus, opening as the change), so
 * turning it off just shows the rest again; Quick Open, search and go to definition still reach the whole repository.
 * It starts on for a pull request (its own worktree: the change is the task) and off for your own change (your working
 * copy), and the reviewer's choice is remembered per review.
 */
@injectable()
export class ReviewNavigatorFilter extends FileNavigatorFilter {

    @inject(ReviewChange) protected readonly change: ReviewChange;
    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(StorageService) protected readonly storage: StorageService;

    /** The reviewer's choice per review id. */
    protected choices: Record<string, boolean> = {};
    protected changedOnly = false;

    constructor(@inject(FileNavigatorPreferences) preferences: FileNavigatorPreferences) {
        super(preferences);
    }

    // The base class's @postConstruct `init`, extended: a second @postConstruct method would replace it.
    protected override init(): void {
        super.init();
        this.storage.getData<Record<string, boolean>>(CHANGED_ONLY_KEY, {}).then(choices => {
            this.choices = { ...choices, ...this.choices };
            this.apply();
        });
        this.change.onDidChange(() => this.apply());
    }

    /** The active review's state: its remembered choice, else on for a pull request's worktree. */
    protected apply(): void {
        const review = this.reviews.activeReview;
        const on = !!review && this.change.hasCode && (this.choices[review.id] ?? !!review.bundle?.worktree);
        if (on !== this.changedOnly) {
            this.changedOnly = on;
            this.fireFilterChanged();
        }
    }

    get isChangedOnly(): boolean {
        return this.changedOnly;
    }

    setChangedOnly(on: boolean): void {
        const review = this.reviews.activeReview;
        if (review) {
            this.choices[review.id] = on;
            this.storage.setData(CHANGED_ONLY_KEY, this.choices);
        }
        this.changedOnly = on;
        this.fireFilterChanged();
    }

    protected override filterItem(item: { id: string }): boolean {
        if (!super.filterItem(item)) {
            return false;
        }
        if (!this.changedOnly || !FileStatNode.is(item) || !this.change.hasCode) {
            return true;
        }
        const path = this.change.relativePath(item.uri);
        return path === undefined || this.change.touches(path);
    }
}

export namespace ReviewChangeCommands {
    export const CHANGED_ONLY: Command = { id: 'co-review.explorer.changedOnly', category: 'Co-Review', label: 'Explorer: Show Only Changed Files' };
    export const SHOW_ALL: Command = { id: 'co-review.explorer.showAll', category: 'Co-Review', label: 'Explorer: Show All Files' };
}

/**
 * The explorer's "Changed files only" switch during a change review: an icon in its title bar while off, and while on a
 * labelled "Changed files only" button that stays visible (CSS), so the narrowed tree is never mistaken for missing
 * files. Both are commands too.
 */
@injectable()
export class ReviewChangeExplorerContribution implements CommandContribution, TabBarToolbarContribution {

    @inject(FileNavigatorFilter) protected readonly filter: ReviewNavigatorFilter;
    @inject(ReviewChange) protected readonly change: ReviewChange;

    registerCommands(registry: CommandRegistry): void {
        const inExplorer = (widget?: Widget) => !(widget instanceof Widget) || widget.id === FILE_NAVIGATOR_ID;
        registry.registerCommand(ReviewChangeCommands.CHANGED_ONLY, {
            isEnabled: () => this.change.hasCode,
            isVisible: (widget?: Widget) => this.change.hasCode && !this.filter.isChangedOnly && inExplorer(widget),
            execute: () => this.filter.setChangedOnly(true)
        });
        registry.registerCommand(ReviewChangeCommands.SHOW_ALL, {
            isEnabled: () => this.change.hasCode,
            isVisible: (widget?: Widget) => this.change.hasCode && this.filter.isChangedOnly && inExplorer(widget),
            execute: () => this.filter.setChangedOnly(false)
        });
    }

    registerToolbarItems(registry: TabBarToolbarRegistry): void {
        // Re-rendered when the change is read (shown or not) and when the filter switches (on or off).
        const onDidChange = Event.any(this.change.onDidChange, this.filter.onFilterChanged);
        registry.registerItem({
            id: ReviewChangeCommands.CHANGED_ONLY.id,
            command: ReviewChangeCommands.CHANGED_ONLY.id,
            tooltip: 'Show only the files the change adds or modifies',
            icon: codicon('git-pull-request'),
            priority: -1,
            onDidChange
        });
        registry.registerItem({
            id: ReviewChangeCommands.SHOW_ALL.id,
            command: ReviewChangeCommands.SHOW_ALL.id,
            tooltip: 'Showing only the files the change adds or modifies (deleted files are on the pull-request page). Click to show all files.',
            // Text only: Theia draws a toolbar item's text only when it has no icon (the icon is added in CSS).
            text: 'Changed files only',
            priority: -1,
            onDidChange
        });
    }
}

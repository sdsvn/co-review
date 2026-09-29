import * as React from '@theia/core/shared/react';
import { codicon } from '@theia/core/lib/browser';
import { AbstractViewContribution } from '@theia/core/lib/browser/shell/view-contribution';
import { ReactWidget } from '@theia/core/lib/browser/widgets/react-widget';
import URI from '@theia/core/lib/common/uri';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { FileService } from '@theia/filesystem/lib/browser/file-service';
import { parsePatch, PatchFile } from '../common/patch';
import { Review } from '../common/review-model';
import { CodeNavigation } from './review-code';
import { ReviewManager } from './review-manager';

/**
 * The files of a change review, in the left sidebar in place of the whole tree: the ones the diff changes (with their
 * lines added and removed), then the ones it affects (its blast radius), each opening in the editor. The full explorer,
 * search and outline stay one click away; nothing here expands or indexes the code.
 */
@injectable()
export class ChangedFilesWidget extends ReactWidget {

    static readonly ID = 'co-review:changed-files';
    static readonly LABEL = 'Changed Files';

    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(CodeNavigation) protected readonly code: CodeNavigation;
    @inject(FileService) protected readonly fileService: FileService;

    protected files: PatchFile[] = [];
    protected loadedFor: string | undefined;

    @postConstruct()
    protected init(): void {
        this.id = ChangedFilesWidget.ID;
        this.title.label = ChangedFilesWidget.LABEL;
        this.title.caption = 'The files of the change, and the ones it affects';
        this.title.iconClass = codicon('diff-multiple');
        this.title.closable = true;
        this.addClass('co-review-changed-files');
        this.toDispose.push(this.reviews.onDidChange(() => this.load()));
        this.load();
    }

    protected async load(): Promise<void> {
        await this.reviews.ready;
        const review = this.reviews.activeReview;
        // The patches change when the change does (a new head), not with every comment.
        const key = review?.bundle ? `${review.id}:${review.bundle.head ?? ''}:${review.bundle.base ?? ''}` : undefined;
        if (key !== this.loadedFor) {
            this.loadedFor = key;
            this.files = [];
            if (review?.bundle) {
                const dir = URI.fromFilePath(review.bundle.dir);
                const children = (await this.fileService.resolve(dir).catch(() => undefined))?.children ?? [];
                for (const child of children.filter(c => /\.(patch|diff)$/.test(c.name))) {
                    const text = (await this.fileService.read(child.resource).catch(() => undefined))?.value ?? '';
                    this.files.push(...parsePatch(text));
                }
            }
        }
        this.update();
    }

    protected render(): React.ReactNode {
        const review = this.reviews.activeReview;
        if (!this.code.has(review)) {
            return <div className='co-review-hint'>A review of a change or pull request lists its files here.</div>;
        }
        const related = review.bundle.related ?? [];
        return <div className='co-review-changed-files-root'>
            {review.bundle.checkout === 'neighbourhood' && <div className='co-review-muted co-review-changed-files-note'>
                <span className={codicon('loading') + ' codicon-modifier-spin'} /> The change and what it affects are checked out; the rest is on its way.
            </div>}
            <div className='co-review-changed-files-group'>Changed ({this.files.length})</div>
            {this.files.map(file => this.renderChanged(review, file))}
            {related.length > 0 && <>
                <div className='co-review-changed-files-group' title='Code that uses, or is used by, what the change changes (from the Graphify graph)'>
                    Related ({related.length})
                </div>
                {related.map(path => <div key={path} className='co-review-changed-file' title={path} onClick={() => this.code.openHead(review, path, 1)}>
                    <span className={codicon('references')} /><span className='co-review-changed-file-name'>{path}</span>
                </div>)}
            </>}
        </div>;
    }

    protected renderChanged(review: Review, file: PatchFile): React.ReactNode {
        const first = file.rows.find(r => r.t === 'add' && r.n)?.n ?? file.rows.find(r => r.n)?.n ?? 1;
        const deleted = file.tag === 'deleted';
        return <div key={file.path} className='co-review-changed-file' title={deleted ? `${file.path} (deleted)` : file.path}
            onClick={() => deleted ? this.code.openBase(review, file.path, 1) : this.code.openHead(review, file.path, first)}>
            <span className={codicon(deleted ? 'diff-removed' : file.tag === 'added' ? 'diff-added' : 'diff-modified')} />
            <span className='co-review-changed-file-name'>{file.path}</span>
            <span className='co-review-add-count'>+{file.added}</span><span className='co-review-del-count'>−{file.removed}</span>
            {file.tag === 'modified' && <span className={`${codicon('diff')} action-label`} title='Compare: base and head side by side'
                onClick={e => { e.stopPropagation(); this.code.compare(review, file.path); }} />}
        </div>;
    }
}

@injectable()
export class ChangedFilesContribution extends AbstractViewContribution<ChangedFilesWidget> {
    constructor() {
        super({
            widgetId: ChangedFilesWidget.ID,
            widgetName: ChangedFilesWidget.LABEL,
            defaultWidgetOptions: { area: 'left', rank: 50 },
            toggleCommandId: 'co-review.changedFiles.toggle'
        });
    }
}

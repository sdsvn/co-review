import { Emitter, Event } from '@theia/core/lib/common/event';
import { DepthFirstTreeIterator, Tree, TreeDecoration, TreeDecorator } from '@theia/core/lib/browser';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { FileStatNode } from '@theia/filesystem/lib/browser';
import { ReviewManager } from './review-manager';

/**
 * The review in the file explorer: a check on files the reviewer viewed (unless changed since), and the number
 * of open threads on each file and under each folder, so coverage and discussion show while browsing.
 */
@injectable()
export class ReviewNavigatorDecorator implements TreeDecorator {

    readonly id = 'co-review-navigator-decorator';

    @inject(ReviewManager) protected readonly reviews: ReviewManager;

    protected readonly onDidChangeDecorationsEmitter = new Emitter<(tree: Tree) => Map<string, TreeDecoration.Data>>();
    readonly onDidChangeDecorations: Event<(tree: Tree) => Map<string, TreeDecoration.Data>> = this.onDidChangeDecorationsEmitter.event;

    @postConstruct()
    protected init(): void {
        this.reviews.onDidChange(() => this.onDidChangeDecorationsEmitter.fire(tree => this.collect(tree)));
    }

    decorations(tree: Tree): Map<string, TreeDecoration.Data> {
        return this.collect(tree);
    }

    protected collect(tree: Tree): Map<string, TreeDecoration.Data> {
        const result = new Map<string, TreeDecoration.Data>();
        const review = this.reviews.activeReview;
        if (!tree.root || !review || review.bundle) {
            return result;
        }
        // Open threads per file, keyed by repository-relative path.
        const threads = new Map<string, number>();
        for (const thread of review.threads) {
            if (thread.status !== 'resolved' && thread.location.uri && thread.location.kind !== 'repository') {
                const file = this.reviews.relativePath(thread.location.uri);
                threads.set(file, (threads.get(file) ?? 0) + 1);
            }
        }
        const viewed = review.viewed ?? {};
        for (const node of new DepthFirstTreeIterator(tree.root)) {
            if (!FileStatNode.is(node)) {
                continue;
            }
            const file = this.reviews.relativePath(node.fileStat.resource.toString());
            const tails: TreeDecoration.Data['tailDecorations'] & object = [];
            const open = node.fileStat.isDirectory
                ? [...threads].reduce((n, [p, count]) => p === file || p.startsWith(`${file}/`) ? n + count : n, 0)
                : threads.get(file) ?? 0;
            if (open) {
                // One text decoration: Theia separates tail decorations with commas.
                tails.push({ data: `${open} open`, tooltip: `${open} open review thread${open === 1 ? '' : 's'}` });
            }
            const at = viewed[file];
            if (at && !node.fileStat.isDirectory) {
                const changed = (node.fileStat.mtime ?? 0) > at;
                tails.push(changed
                    ? { iconClass: ['codicon', 'codicon-history'], tooltip: 'Changed since you viewed it' }
                    : { iconClass: ['codicon', 'codicon-pass'], tooltip: 'Viewed' });
            }
            if (tails.length) {
                result.set(node.id, { tailDecorations: tails });
            }
        }
        return result;
    }
}

import { WidgetOpenHandler } from '@theia/core/lib/browser/widget-open-handler';
import URI from '@theia/core/lib/common/uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { ReviewManager } from '../review-manager';
import { DocumentReviewWidget, DocumentReviewWidgetOptions } from './document-review-widget';

/**
 * Opens Markdown documents in the review view. Review artifacts (`index.markdown`, `*.pseudocode.md`, and
 * every page of the active review directory, such as an OKF bundle's concept pages) open there by default;
 * other Markdown files offer it via "Open With".
 */
@injectable()
export class DocumentReviewOpenHandler extends WidgetOpenHandler<DocumentReviewWidget> {

    readonly id = DocumentReviewWidget.FACTORY_ID;
    readonly label = 'Review (rendered)';

    @inject(ReviewManager) protected readonly reviews: ReviewManager;

    canHandle(uri: URI): number {
        const name = uri.path.base.toLowerCase();
        if (name === 'index.markdown' || name.endsWith('.pseudocode.md')) {
            return 200;
        }
        const dir = this.reviews.activeReview?.bundle?.dir;
        if (dir && /\.(md|markdown)$/.test(name) && URI.fromFilePath(dir).isEqualOrParent(uri)) {
            return 200;
        }
        return /\.(md|markdown)$/.test(name) ? 50 : 0;
    }

    protected createWidgetOptions(uri: URI): DocumentReviewWidgetOptions {
        return { uri: uri.withoutFragment().toString() };
    }
}

import { WidgetOpenHandler } from '@theia/core/lib/browser/widget-open-handler';
import URI from '@theia/core/lib/common/uri';
import { injectable } from '@theia/core/shared/inversify';
import { DocumentReviewWidget, DocumentReviewWidgetOptions } from './document-review-widget';

/**
 * Opens Markdown documents in the review view. Review artifacts (`index.markdown`,
 * `*.pseudocode.md`) open there by default; other Markdown files offer it via "Open With".
 */
@injectable()
export class DocumentReviewOpenHandler extends WidgetOpenHandler<DocumentReviewWidget> {

    readonly id = DocumentReviewWidget.FACTORY_ID;
    readonly label = 'Review (rendered)';

    canHandle(uri: URI): number {
        const name = uri.path.base.toLowerCase();
        if (name === 'index.markdown' || name.endsWith('.pseudocode.md')) {
            return 200;
        }
        return /\.(md|markdown)$/.test(name) ? 50 : 0;
    }

    protected createWidgetOptions(uri: URI): DocumentReviewWidgetOptions {
        return { uri: uri.withoutFragment().toString() };
    }
}

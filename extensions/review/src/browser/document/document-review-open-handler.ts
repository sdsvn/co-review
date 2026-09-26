import { WidgetOpenHandler } from '@theia/core/lib/browser/widget-open-handler';
import URI from '@theia/core/lib/common/uri';
import { injectable } from '@theia/core/shared/inversify';
import { OpenerOptions } from '@theia/core/lib/browser/opener-service';
import { DocumentReviewWidget, DocumentReviewWidgetOptions } from './document-review-widget';

/**
 * Opens Markdown documents rendered, in the review view (Mermaid, pseudocode trees, inline threads).
 * The source is one click away; opening at a line (search results, `path:line` references) goes to the editor.
 */
@injectable()
export class DocumentReviewOpenHandler extends WidgetOpenHandler<DocumentReviewWidget> {

    readonly id = DocumentReviewWidget.FACTORY_ID;
    readonly label = 'Review (rendered)';

    canHandle(uri: URI, options?: OpenerOptions & { selection?: unknown }): number {
        if (!/\.(md|markdown)$/i.test(uri.path.base)) {
            return 0;
        }
        return options?.selection ? 50 : 200;
    }

    protected createWidgetOptions(uri: URI): DocumentReviewWidgetOptions {
        return { uri: uri.withoutFragment().toString() };
    }
}

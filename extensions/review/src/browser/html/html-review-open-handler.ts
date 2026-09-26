import { WidgetOpenHandler } from '@theia/core/lib/browser/widget-open-handler';
import { OpenerOptions } from '@theia/core/lib/browser/opener-service';
import URI from '@theia/core/lib/common/uri';
import { injectable } from '@theia/core/shared/inversify';
import { HtmlReviewWidget, HtmlReviewWidgetOptions } from './html-review-widget';

/** Opens HTML pages rendered, for review; the source is one click away, and opening at a line goes to the editor. */
@injectable()
export class HtmlReviewOpenHandler extends WidgetOpenHandler<HtmlReviewWidget> {

    readonly id = HtmlReviewWidget.FACTORY_ID;
    readonly label = 'Review (rendered)';

    canHandle(uri: URI, options?: OpenerOptions & { selection?: unknown }): number {
        if (!/\.html?$/i.test(uri.path.base)) {
            return 0;
        }
        return options?.selection ? 50 : 200;
    }

    protected createWidgetOptions(uri: URI): HtmlReviewWidgetOptions {
        return { uri: uri.withoutFragment().toString() };
    }
}

import { WidgetOpenHandler } from '@theia/core/lib/browser/widget-open-handler';
import URI from '@theia/core/lib/common/uri';
import { injectable } from '@theia/core/shared/inversify';
import { PatchReviewWidget, PatchReviewWidgetOptions } from './patch-review-widget';

/** `.patch` / `.diff` files open as review pages. */
@injectable()
export class PatchReviewOpenHandler extends WidgetOpenHandler<PatchReviewWidget> {

    readonly id = PatchReviewWidget.FACTORY_ID;
    readonly label = 'Review (patch)';

    canHandle(uri: URI): number {
        return /\.(patch|diff)$/i.test(uri.path.base) ? 200 : 0;
    }

    protected createWidgetOptions(uri: URI): PatchReviewWidgetOptions {
        return { uri: uri.withoutFragment().toString() };
    }
}

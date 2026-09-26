import '../../src/browser/style/review.css';

import { FrontendApplicationContribution } from '@theia/core/lib/browser/frontend-application-contribution';
import { bindViewContribution } from '@theia/core/lib/browser/shell/view-contribution';
import { WidgetFactory } from '@theia/core/lib/browser/widget-manager';
import { RemoteConnectionProvider, ServiceConnectionProvider } from '@theia/core/lib/browser/messaging/service-connection-provider';
import { ContainerModule } from '@theia/core/shared/inversify';
import { REVIEW_SERVICE_PATH, ReviewService } from '../common/review-protocol';
import { SYNTAX_SERVICE_PATH, SyntaxService } from '../common/syntax-protocol';
import { ReviewLocations } from './review-locations';
import { ReviewClientImpl } from './review-client';
import { ReviewContribution } from './review-contribution';
import { ReviewEditorDecorator } from './review-editor-decorator';
import { ReviewManager } from './review-manager';
import { ReviewNavigator } from './review-navigator';
import { ReviewNavigatorDecorator } from './review-navigator-decorator';
import { NavigatorTreeDecorator } from '@theia/navigator/lib/browser/navigator-decorator-service';
import { ReviewSelectionActions } from './review-selection-actions';
import { ReviewWidget } from './review-widget';
import { ReviewShellFilter } from './review-shell';
import { OpenHandler } from '@theia/core/lib/browser/opener-service';
import { DocumentReviewWidget, DocumentReviewWidgetOptions } from './document/document-review-widget';
import { DocumentReviewOpenHandler } from './document/document-review-open-handler';
import { PatchReviewWidget, PatchReviewWidgetOptions } from './patch/patch-review-widget';
import { PatchReviewOpenHandler } from './patch/patch-review-open-handler';
import { FilterContribution } from '@theia/core/lib/common/contribution-filter';
import { ColorContribution } from '@theia/core/lib/browser/color-application-contribution';
import { CoReviewThemeContribution } from './theme/co-review-theme';

export default new ContainerModule(bind => {
    bind(FilterContribution).to(ReviewShellFilter).inSingletonScope();
    bind(ReviewClientImpl).toSelf().inSingletonScope();
    bind(ReviewService).toDynamicValue(ctx => {
        const connection = ctx.container.get<ServiceConnectionProvider>(RemoteConnectionProvider);
        return connection.createProxy<ReviewService>(REVIEW_SERVICE_PATH, ctx.container.get(ReviewClientImpl));
    }).inSingletonScope();

    bind(SyntaxService).toDynamicValue(ctx =>
        ctx.container.get<ServiceConnectionProvider>(RemoteConnectionProvider).createProxy<SyntaxService>(SYNTAX_SERVICE_PATH)
    ).inSingletonScope();
    bind(ReviewLocations).toSelf().inSingletonScope();
    bind(ReviewManager).toSelf().inSingletonScope();
    bind(ReviewNavigator).toSelf().inSingletonScope();
    bind(ReviewNavigatorDecorator).toSelf().inSingletonScope();
    bind(NavigatorTreeDecorator).toService(ReviewNavigatorDecorator);

    bind(ReviewEditorDecorator).toSelf().inSingletonScope();
    bind(FrontendApplicationContribution).toService(ReviewEditorDecorator);
    bind(ReviewSelectionActions).toSelf().inSingletonScope();
    bind(FrontendApplicationContribution).toService(ReviewSelectionActions);

    bind(DocumentReviewOpenHandler).toSelf().inSingletonScope();
    bind(OpenHandler).toService(DocumentReviewOpenHandler);
    bind(WidgetFactory).toDynamicValue(ctx => ({
        id: DocumentReviewWidget.FACTORY_ID,
        createWidget: (options: DocumentReviewWidgetOptions) => {
            const child = ctx.container.createChild();
            child.bind(DocumentReviewWidgetOptions).toConstantValue(options);
            child.bind(DocumentReviewWidget).toSelf();
            const widget = child.get(DocumentReviewWidget);
            widget.init();
            return widget;
        }
    })).inSingletonScope();

    bind(PatchReviewOpenHandler).toSelf().inSingletonScope();
    bind(OpenHandler).toService(PatchReviewOpenHandler);
    bind(WidgetFactory).toDynamicValue(ctx => ({
        id: PatchReviewWidget.FACTORY_ID,
        createWidget: (options: PatchReviewWidgetOptions) => {
            const child = ctx.container.createChild();
            child.bind(PatchReviewWidgetOptions).toConstantValue(options);
            child.bind(PatchReviewWidget).toSelf();
            const widget = child.get(PatchReviewWidget);
            widget.init();
            return widget;
        }
    })).inSingletonScope();

    bind(ReviewWidget).toSelf();
    bind(WidgetFactory).toDynamicValue(ctx => ({
        id: ReviewWidget.ID,
        createWidget: () => ctx.container.get(ReviewWidget)
    })).inSingletonScope();
    bindViewContribution(bind, ReviewContribution);
    bind(FrontendApplicationContribution).toService(ReviewContribution);

    bind(CoReviewThemeContribution).toSelf().inSingletonScope();
    bind(FrontendApplicationContribution).toService(CoReviewThemeContribution);
    bind(ColorContribution).toService(CoReviewThemeContribution);
});

import { ConnectionHandler, RpcConnectionHandler } from '@theia/core/lib/common/messaging';
import { BackendApplicationContribution } from '@theia/core/lib/node/backend-application';
import { AgentPresenceTracker, HumanDecisions } from './agent-coordination';
import { CoReviewerMcp } from './co-reviewer-mcp';
import { BundleService } from './bundle-service';
import { MobileReview } from './mobile-review';
import { ContainerModule } from '@theia/core/shared/inversify';
import { REVIEW_SERVICE_PATH, ReviewClient } from '../common/review-protocol';
import { SYNTAX_SERVICE_PATH, SyntaxService } from '../common/syntax-protocol';
import { SyntaxServiceImpl } from './syntax-service-impl';
import { ReviewServiceImpl } from './review-service-impl';
import { ReviewStore } from './review-store';
import { AcpAgentService } from './acp-agent-service';
import { AgentSetup } from './agent-setup';

export default new ContainerModule(bind => {
    bind(ReviewStore).toSelf().inSingletonScope();
    bind(AcpAgentService).toSelf().inSingletonScope();
    bind(HumanDecisions).toSelf().inSingletonScope();
    bind(BundleService).toSelf().inSingletonScope();
    bind(AgentSetup).toSelf().inSingletonScope();
    bind(AgentPresenceTracker).toSelf().inSingletonScope();
    bind(CoReviewerMcp).toSelf().inSingletonScope();
    bind(BackendApplicationContribution).toService(CoReviewerMcp);
    bind(MobileReview).toSelf().inSingletonScope();
    bind(BackendApplicationContribution).toService(MobileReview);
    bind(ReviewServiceImpl).toSelf();
    bind(ConnectionHandler).toDynamicValue(ctx =>
        new RpcConnectionHandler<ReviewClient>(REVIEW_SERVICE_PATH, client => {
            const service = ctx.container.get(ReviewServiceImpl);
            service.setClient(client);
            client.onDidCloseConnection(() => service.dispose());
            return service;
        })
    ).inSingletonScope();

    bind(SyntaxServiceImpl).toSelf().inSingletonScope();
    bind(SyntaxService).toService(SyntaxServiceImpl);
    bind(ConnectionHandler).toDynamicValue(ctx =>
        new RpcConnectionHandler(SYNTAX_SERVICE_PATH, () => ctx.container.get(SyntaxService))
    ).inSingletonScope();
});

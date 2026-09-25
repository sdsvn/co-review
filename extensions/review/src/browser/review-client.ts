import { Emitter } from '@theia/core/lib/common/event';
import { injectable } from '@theia/core/shared/inversify';
import { AgentPresence, Review } from '../common/review-model';
import { ReviewClient } from '../common/review-protocol';

@injectable()
export class ReviewClientImpl implements ReviewClient {
    protected readonly onReviewChangedEmitter = new Emitter<Review>();
    readonly onDidChangeReview = this.onReviewChangedEmitter.event;
    protected readonly onReviewDeletedEmitter = new Emitter<{ reviewId: string; workspaceRoot: string }>();
    readonly onDidDeleteReview = this.onReviewDeletedEmitter.event;

    protected readonly onAgentPresenceEmitter = new Emitter<AgentPresence>();
    readonly onDidChangeAgentPresence = this.onAgentPresenceEmitter.event;

    onAgentPresence(presence: AgentPresence): void {
        this.onAgentPresenceEmitter.fire(presence);
    }

    onReviewChanged(review: Review): void {
        this.onReviewChangedEmitter.fire(review);
    }

    onReviewDeleted(reviewId: string, workspaceRoot: string): void {
        this.onReviewDeletedEmitter.fire({ reviewId, workspaceRoot });
    }
}

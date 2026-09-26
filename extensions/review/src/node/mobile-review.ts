import { BackendApplicationContribution } from '@theia/core/lib/node/backend-application';
import { FileUri } from '@theia/core/lib/common/file-uri';
import * as express from '@theia/core/shared/express';
import { inject, injectable } from '@theia/core/shared/inversify';
import * as path from 'path';
import { DocAnchor, PatchAnchor, Review, ReviewDecision, ReviewThread, ThreadStatus } from '../common/review-model';
import { BundleService } from './bundle-service';
import { MOBILE_PAGE, MOBILE_MANIFEST, MOBILE_SERVICE_WORKER, MOBILE_ICON } from './mobile-page';
import { currentUser } from './participants';
import { ReviewStore } from './review-store';

/**
 * The phone view (`/m`): an installable, lightweight page to read and answer threads, accept
 * findings and suggestions, and submit — for reviewing away from the desk (e.g. over Tailscale).
 * Local connections only, plus hosts listed in `CO_REVIEW_ALLOWED_HOSTS` (comma-separated).
 */
@injectable()
export class MobileReview implements BackendApplicationContribution {

    @inject(ReviewStore) protected readonly store: ReviewStore;
    @inject(BundleService) protected readonly bundles: BundleService;

    protected allowed(req: express.Request): boolean {
        const host = (req.headers.host ?? '').replace(/:\d+$/, '').toLowerCase();
        const extra = (process.env.CO_REVIEW_ALLOWED_HOSTS ?? '').split(',').map(h => h.trim().toLowerCase()).filter(Boolean);
        return ['localhost', '127.0.0.1', '[::1]', ...extra].includes(host);
    }

    configure(app: express.Application): void {
        const guard: express.RequestHandler = (req, res, next) => this.allowed(req) ? next() : res.status(403).send('Not allowed from this host (set CO_REVIEW_ALLOWED_HOSTS)');
        const serve = (type: string, body: string) => (_req: express.Request, res: express.Response) => res.type(type).set('Cache-Control', 'no-cache').send(body);
        app.get(['/m', '/m/'], guard, serve('html', MOBILE_PAGE));
        app.get('/m/manifest.webmanifest', guard, serve('application/manifest+json', MOBILE_MANIFEST));
        app.get('/m/sw.js', guard, serve('application/javascript', MOBILE_SERVICE_WORKER));
        app.get('/m/icon.svg', guard, serve('image/svg+xml', MOBILE_ICON));

        const api = express.Router();
        api.use(guard, express.json());
        api.get('/reviews', async (_req, res) => {
            const reviews = (await this.store.listAll()).filter(r => !r.archivedAt);
            res.json(reviews.map(r => ({
                id: r.id, title: r.title, repository: path.basename(r.bundle?.dir ?? FileUri.fsPath(r.workspaceRoot)),
                open: r.threads.filter(t => t.status === 'open').length, proposed: r.threads.filter(t => t.status === 'proposed').length,
                updatedAt: r.updatedAt, agent: r.agent?.name
            })));
        });
        // Open reviews of a repository (reviews of it and of review directories inside it), for hooks and scripts.
        // `format=claude-hook` answers with Claude Code SessionStart context, or 204 when there is nothing to say.
        api.get('/status', async (req, res) => {
            const root = path.resolve(String(req.query.root ?? ''));
            const reviews = (await this.store.listAll()).map(r => ({ r, dir: r.bundle?.dir ?? FileUri.fsPath(r.workspaceRoot) }))
                .filter(({ r, dir }) => !r.archivedAt && (dir === root || dir.startsWith(root + path.sep)) && r.threads.some(t => t.status === 'open' || t.status === 'proposed'))
                .map(({ r, dir }) => ({
                    id: r.id, title: r.title, root: dir,
                    open: r.threads.filter(t => t.status === 'open').length,
                    proposed: r.threads.filter(t => t.status === 'proposed').length,
                    needsReply: r.threads.filter(t => t.status === 'open' && t.messages[t.messages.length - 1]?.author.kind === 'human').length
                }));
            if (req.query.format !== 'claude-hook') {
                return res.json(reviews);
            }
            if (!reviews.length) {
                return res.status(204).end();
            }
            const lines = reviews.map(v => `- "${v.title}": ${v.needsReply} question(s) waiting for an answer, ${v.open} open thread(s)`
                + `${v.proposed ? `, ${v.proposed} proposed finding(s)` : ''}${v.root !== root ? ` (review directory ${path.relative(root, v.root)})` : ''}`);
            res.json({ hookSpecificOutput: { hookEventName: 'SessionStart', additionalContext: ['Co-Review has open reviews for this repository:', ...lines,
                'If the user wants to continue reviewing, rejoin with the co-review skill (open_review with this repository, or its `dir`), '
                + 'answer the waiting questions, and wait for their verdict.'].join('\n') } });
        });
        // Long poll: returns when the review changed after `since` (or after 25 s).
        api.get('/reviews/:id', async (req, res) => {
            const since = Number(req.query.since ?? 0);
            let review = await this.store.get(req.params.id);
            if (review && review.updatedAt <= since) {
                await new Promise<void>(resolve => {
                    const timer = setTimeout(done, 25000);
                    const listener = this.store.onDidChange(c => c.kind === 'changed' && c.review.id === review!.id && done());
                    req.on('close', done);
                    function done(): void {
                        clearTimeout(timer);
                        listener.dispose();
                        resolve();
                    }
                });
                review = await this.store.get(req.params.id);
            }
            return review ? res.json(this.view(review)) : res.status(404).json({ error: 'No such review' });
        });
        const withReview = (fn: (review: Review, req: express.Request) => Promise<unknown>): express.RequestHandler => async (req, res) => {
            try {
                const review = await this.store.get(req.params.id);
                if (!review) {
                    return res.status(404).json({ error: 'No such review' });
                }
                await fn(review, req);
                res.json({ ok: true });
            } catch (e) {
                res.status(400).json({ error: String(e) });
            }
        };
        api.post('/reviews/:id/threads/:tid/messages', withReview(async (review, req) => {
            const body = String(req.body?.body ?? '').trim();
            if (body) {
                await this.store.addMessage(review.id, req.params.tid, body, await currentUser(review.workspaceRoot));
            }
        }));
        api.post('/reviews/:id/threads/:tid/status', withReview(async (review, req) => {
            await this.store.setThreadStatus(review.id, req.params.tid, req.body?.status as ThreadStatus, await currentUser(review.workspaceRoot));
        }));
        api.post('/reviews/:id/threads/:tid/proposal', withReview(async (review, req) => {
            await this.bundles.decideProposal(review.id, req.params.tid, !!req.body?.accept);
        }));
        api.post('/reviews/:id/submit', withReview(async (review, req) => {
            await this.store.submit(review.id, req.body?.decision as ReviewDecision, String(req.body?.summary ?? ''), await currentUser(review.workspaceRoot));
        }));
        app.use('/api/m', api);
    }

    protected label(review: Review, thread: ReviewThread): string {
        const l = thread.location;
        const root = review.bundle?.dir ?? FileUri.fsPath(review.workspaceRoot);
        const rel = l.uri ? path.relative(root, FileUri.fsPath(l.uri)) : '';
        if (l.kind === 'repository' || !l.uri) {
            return 'Repository';
        }
        if (l.docAnchor) {
            return l.docAnchor.type === 'document' ? rel : `${rel} · ${DocAnchor.describe(l.docAnchor)}`;
        }
        if (l.patchAnchor) {
            return l.patchAnchor.type === 'patch' ? rel : `${rel} · ${PatchAnchor.describe(l.patchAnchor)}`;
        }
        if (l.range) {
            const [a, b] = [l.range.start.line + 1, l.range.end.line + 1];
            return `${rel}:${a === b ? a : `${a}-${b}`}`;
        }
        return rel;
    }

    protected view(review: Review): object {
        return {
            id: review.id, title: review.title, updatedAt: review.updatedAt, agent: review.agent?.name, verdict: review.verdict,
            threads: review.threads.map(t => ({
                id: t.id, number: t.number, label: this.label(review, t), symbol: t.location.symbol,
                code: t.location.kind !== 'repository' ? t.location.anchor?.text : undefined,
                status: t.status, intent: t.intent, severity: t.severity, agentState: t.agentState, proposal: t.proposal,
                messages: t.messages.filter(m => m.body.trim()).map(m => ({ author: m.author.name, kind: m.author.kind, body: m.body, at: m.createdAt, status: m.status }))
            }))
        };
    }
}

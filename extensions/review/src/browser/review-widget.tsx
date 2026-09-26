import * as React from '@theia/core/shared/react';
import { CommandService } from '@theia/core/lib/common/command';
import { Message } from '@theia/core/lib/browser/widgets/widget';
import { ReactWidget } from '@theia/core/lib/browser/widgets/react-widget';
import { codicon } from '@theia/core/lib/browser/widgets/widget';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { AgentConfig, CodeLocation, Review, ReviewCoverage, ReviewDecision, ReviewScope, ReviewThread } from '../common/review-model';
import { ReviewManager } from './review-manager';
import { ReviewCommands } from './review-commands';
import { ReviewNavigator } from './review-navigator';
import { DraftEditor, ThreadView } from './review-components';

type Filter = 'open' | 'proposed' | 'resolved' | 'all';

/** Review overview: drafts and every thread of the active review, grouped by location. */
@injectable()
export class ReviewWidget extends ReactWidget {

    static readonly ID = 'co-review:panel';
    static readonly LABEL = 'Review';

    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(ReviewNavigator) protected readonly navigator: ReviewNavigator;
    @inject(CommandService) protected readonly commands: CommandService;

    protected filter: Filter = 'open';
    /** Threads grouped by file (default) or by area: their first label, e.g. the part of the repository a finding is about. */
    protected grouping: 'file' | 'area' = 'file';
    protected selectedThreadId: string | undefined;
    /** How much of the repository the reviewer has viewed; refreshed shortly after the review changes. */
    protected coverage: ReviewCoverage | undefined;
    protected coverageExpanded = true;
    protected coverageTimer: number | undefined;
    protected pendingScroll = false;

    @postConstruct()
    protected init(): void {
        this.id = ReviewWidget.ID;
        this.title.label = ReviewWidget.LABEL;
        this.title.caption = 'Repository Review';
        this.title.closable = true;
        this.title.iconClass = codicon('comment-discussion');
        this.addClass('co-review');
        this.node.tabIndex = 0;
        this.toDispose.push(this.reviews.onDidChange(() => {
            this.update();
            this.refreshCoverage();
        }));
        this.toDispose.push(this.reviews.onDidChangeDrafts(() => this.update()));
        this.toDispose.push(this.reviews.onDidRequestReveal(threadId => {
            this.selectedThreadId = threadId;
            const thread = this.reviews.activeReview?.threads.find(t => t.id === threadId);
            if (thread && this.filter !== 'all' && thread.status !== this.filter) {
                this.filter = 'all';
            }
            this.pendingScroll = true;
            this.update();
        }));
        this.update();
        this.refreshCoverage();
    }

    protected refreshCoverage(): void {
        window.clearTimeout(this.coverageTimer);
        this.coverageTimer = window.setTimeout(async () => {
            this.coverage = await this.reviews.getCoverage().catch(() => undefined);
            this.update();
        }, 400);
    }

    /** Viewed files, overall and per area; an area opens its next file not viewed yet. */
    protected renderCoverage(review: Review): React.ReactNode {
        const coverage = this.coverage;
        if (!coverage?.total || review.bundle) {
            return undefined;
        }
        const bar = (viewed: number, total: number) => <span className='co-review-progress'>
            <span style={{ width: `${Math.round(100 * viewed / Math.max(total, 1))}%` }} />
        </span>;
        return <div className='co-review-coverage'>
            <div className='co-review-coverage-header' onClick={() => { this.coverageExpanded = !this.coverageExpanded; this.update(); }}
                title='Files you marked as viewed (⌘⌥V in the editor); a file changed since counts as not viewed'>
                <span className={codicon(this.coverageExpanded ? 'chevron-down' : 'chevron-right')} />
                <span>Viewed {coverage.viewed} of {coverage.total} files</span>
                {bar(coverage.viewed, coverage.total)}
                <span className='co-review-link' title='Where to start, the areas of the code and how they connect'
                    onClick={e => { e.stopPropagation(); this.commands.executeCommand(ReviewCommands.OPEN_OVERVIEW.id); }}>Overview</span>
            </div>
            {this.coverageExpanded && coverage.areas.length > 1 && <div className='co-review-coverage-areas'>
                {coverage.areas.map(area => <div key={area.path} className={`co-review-coverage-area ${area.next ? '' : 'done'}`}
                    title={area.next ? `Open the next file to review: ${area.next}` : 'All files viewed'}
                    onClick={() => area.next && this.navigator.openReference(area.next)}>
                    <span className={codicon(area.next ? 'folder' : 'pass-filled')} />
                    <span className='co-review-coverage-path'>{area.path === '.' ? '(root)' : area.path}</span>
                    <span className='co-review-coverage-count'>{area.viewed}/{area.total}</span>
                    {bar(area.viewed, area.total)}
                </div>)}
            </div>}
        </div>;
    }

    protected override onUpdateRequest(msg: Message): void {
        super.onUpdateRequest(msg);
        if (this.pendingScroll && this.selectedThreadId) {
            this.pendingScroll = false;
            requestAnimationFrame(() => document.getElementById(`co-review-thread-${this.selectedThreadId}`)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }));
        }
    }

    protected override onActivateRequest(msg: Message): void {
        super.onActivateRequest(msg);
        // Don't steal focus from a quick input opened by a click inside this panel.
        if (!document.activeElement?.closest('.quick-input-widget') && !this.node.contains(document.activeElement)) {
            this.node.focus();
        }
    }

    protected render(): React.ReactNode {
        const review = this.reviews.activeReview;
        return <div className='co-review-root'>
            {this.renderHeader(review)}
            {review ? this.renderReview(review) : this.renderEmpty()}
        </div>;
    }

    protected renderHeader(review: Review | undefined): React.ReactNode {
        const all = this.reviews.reviews;
        return <div className='co-review-header'>
            <select className='theia-select co-review-select' value={review?.id ?? ''} title='Active review'
                onChange={e => this.reviews.setActiveReview(e.currentTarget.value || undefined)}>
                {!review && <option value=''>No active review</option>}
                {all.map(r => <option key={r.id} value={r.id}>{r.title}</option>)}
            </select>
            <span className={`${codicon('add')} action-label`} title='New review' onClick={() => this.commands.executeCommand(ReviewCommands.CREATE_REVIEW.id)} />
            {review && <span className={`${codicon('ellipsis')} action-label`} title='More actions'
                onClick={() => this.commands.executeCommand(ReviewCommands.REVIEW_ACTIONS.id)} />}
        </div>;
    }

    protected renderEmpty(): React.ReactNode {
        return <div className='co-review-empty'>
            <p>No review is active for this repository.</p>
            <p>A review can span the whole repository, a branch, a commit, or selected files and folders.</p>
            <button className='theia-button' onClick={() => this.commands.executeCommand(ReviewCommands.CREATE_REVIEW.id)}>Start a Review</button>
            <p>Want Claude Code, Pi, Oh My Pi or another agent to answer your questions and open reviews of its own work?</p>
            <button className='theia-button secondary' onClick={() => this.commands.executeCommand(ReviewCommands.SETUP_HARNESS.id)}>Connect an Agent</button>
        </div>;
    }

    protected renderReview(review: Review): React.ReactNode {
        const open = review.threads.filter(t => t.status === 'open').length;
        const proposed = review.threads.filter(t => t.status === 'proposed').length;
        const resolved = review.threads.filter(t => t.status === 'resolved').length;
        const threads = review.threads
            .filter(t => this.filter === 'all' || t.status === this.filter)
            .sort((a, b) => this.sortKey(a).localeCompare(this.sortKey(b)) || a.number - b.number);
        const groups = new Map<string, ReviewThread[]>();
        for (const thread of threads) {
            const key = this.grouping === 'area' ? thread.labels?.[0] ?? 'Other' : this.groupKey(thread.location);
            groups.set(key, [...(groups.get(key) ?? []), thread]);
        }
        const drafts = this.reviews.drafts;
        return <>
            {this.renderSummary(review)}
            {this.renderCoverage(review)}
            {(review.bundle || review.agent) && <SubmitReview review={review} manager={this.reviews} />}
            {drafts.length > 0 && <div className='co-review-drafts'>
                <div className='co-review-section-header'>
                    <span>Drafts</span>
                    <span className='co-review-spacer' />
                    {drafts.filter(d => d.body.trim()).length > 1 &&
                        <span className='co-review-link' onClick={() => this.reviews.submitAllDrafts()}>Submit all</span>}
                </div>
                {drafts.map(draft => <DraftEditor key={draft.id}
                    manager={this.reviews}
                    draft={draft}
                    label={this.reviews.locationLabel(draft.location)}
                    acceptFocus={!draft.location.range}
                    onOpen={() => this.openDraft(draft.location, draft.id)} />)}
            </div>}
            <div className='co-review-filters'>
                {this.renderFilter('open', `Open ${open}`)}
                {proposed > 0 && this.renderFilter('proposed', `Proposed ${proposed}`)}
                {this.renderFilter('resolved', `Resolved ${resolved}`)}
                {this.renderFilter('all', `All ${review.threads.length}`)}
                {review.threads.some(t => t.labels?.length) && <>
                    <span className='co-review-spacer' />
                    <span className={`co-review-filter ${this.grouping === 'area' ? 'active' : ''}`}
                        title='Group threads by area (their first label) instead of by file'
                        onClick={() => { this.grouping = this.grouping === 'area' ? 'file' : 'area'; this.update(); }}>
                        <span className={codicon('tag')} /> By area
                    </span>
                </>}
            </div>
            <div className='co-review-threads'>
                {threads.length === 0 && <div className='co-review-hint'>
                    {review.threads.length === 0
                        ? review.agent
                            ? 'Hover a line in the editor and click + to comment (drag for several lines), or select code and choose Ask Agent.'
                            : <>Hover a line in the editor and click + to comment (drag for several lines).
                                To ask an agent about code, <span className='co-review-link'
                                    onClick={() => this.commands.executeCommand(ReviewCommands.CONFIGURE_AGENT.id)}>connect one</span>.</>
                        : `No ${this.filter} threads.`}
                </div>}
                {[...groups.entries()].sort(([a], [b]) => this.grouping === 'area' ? a.localeCompare(b) : 0).map(([key, group]) => <div key={key} className='co-review-group'>
                    {this.grouping === 'area'
                        ? <div className='co-review-group-header' title={`${group.length} thread${group.length === 1 ? '' : 's'} labelled ${key}`}>
                            <span className={codicon('tag')} />
                            <span className='co-review-group-label'>{key}</span>
                            <span className='co-review-group-count'>{group.length}</span>
                        </div>
                        : <div className='co-review-group-header' title={key}>
                            <span className={codicon(group[0].location.kind === 'repository' ? 'repo' : group[0].location.kind === 'directory' ? 'folder' : 'file')} />
                            <span className='co-review-group-label' onClick={() => group[0].location.uri && this.navigator.open({ kind: 'file', uri: group[0].location.uri })}>{key}</span>
                        </div>}
                    {group.map(thread => <ThreadView key={thread.id}
                        manager={this.reviews}
                        thread={thread}
                        label={this.reviews.locationLabel(thread.location)}
                        anchorState={this.reviews.getAnchorState(thread.id)}
                        selected={thread.id === this.selectedThreadId}
                        onSelect={() => { this.selectedThreadId = thread.id; this.update(); }}
                        onOpen={() => this.openThread(thread)}
                        onOpenReference={(path, line, end) => this.navigator.openReference(path, line, end)} />)}
                </div>)}
            </div>
        </>;
    }

    /** One quiet line: scope and agent; the agent part opens the agent picker. */
    protected renderSummary(review: Review): React.ReactNode {
        const agent = review.agent;
        const mcp = agent?.transport === 'mcp';
        const listening = mcp && this.reviews.getAgentPresence(review.id)?.listening;
        const agentTitle = !agent ? 'Connect an agent to ask questions'
            : mcp ? (listening ? `${agent.name} is listening (via MCP)` : `${agent.name} is busy; questions are delivered when it checks in`)
                : `${agent.name} (ACP): ${AgentConfig.commandLine(agent)}`;
        // The model (and effort, mode) chosen for an ACP agent; the agent's defaults when none.
        const chosen = agent?.settingsLabel ?? Object.values(agent?.settings ?? {}).join(' · ');
        return <div className='co-review-summary'>
            <span className={codicon(this.scopeIcon(review.scope))} />
            <span className='co-review-summary-scope'>{ReviewScope.label(review.scope, uri => this.reviews.relativePath(uri))}</span>
            <span className='co-review-spacer' />
            <span className={`co-review-summary-agent ${listening ? 'listening' : ''} ${agent ? '' : 'none'}`} title={agentTitle}
                onClick={() => this.commands.executeCommand(ReviewCommands.CONFIGURE_AGENT.id)}>
                <span className={codicon('hubot')} />{agent ? agent.name : 'Connect agent'}
                {mcp && <span className={`co-review-dot ${listening ? 'on' : ''}`} />}
            </span>
            {AgentConfig.isAcp(agent) && <span className='co-review-summary-agent co-review-summary-model' title={`Model: ${chosen || 'the agent\'s default'} — click to choose`}
                onClick={() => this.commands.executeCommand(ReviewCommands.AGENT_SETTINGS.id)}>
                <span className={codicon('settings-gear')} />{chosen || 'default model'}
            </span>}
        </div>;
    }

    protected async openThread(thread: ReviewThread): Promise<void> {
        this.reviews.setCollapsed(thread.id, false);
        await this.navigator.open(thread.location, thread.id);
    }

    protected async openDraft(location: CodeLocation, draftId: string): Promise<void> {
        await this.navigator.open(location);
        this.reviews.focusDraft(draftId);
    }

    protected renderFilter(filter: Filter, label: string): React.ReactNode {
        return <span className={`co-review-filter ${this.filter === filter ? 'active' : ''}`}
            onClick={() => { this.filter = filter; this.update(); }}>{label}</span>;
    }

    protected groupKey(location: CodeLocation): string {
        if (location.kind === 'repository' || !location.uri) {
            return 'Repository';
        }
        const path = this.reviews.relativePath(location.uri);
        return location.kind === 'directory' ? `${path}/` : path;
    }

    protected sortKey(thread: ReviewThread): string {
        const key = this.groupKey(thread.location);
        const line = String(thread.location.range?.start.line ?? -1).padStart(8, '0');
        return (key === 'Repository' ? '' : key) + '\u0000' + line;
    }

    protected scopeIcon(scope: ReviewScope): string {
        switch (scope.kind) {
            case 'repository': return 'repo';
            case 'paths': return 'files';
            case 'branch': return 'git-compare';
            case 'commit': return 'git-commit';
        }
    }
}

/** Submit a review round to the agent: a decision and a message. */
function SubmitReview({ review, manager }: { review: Review; manager: ReviewManager }): React.ReactElement {
    const [open, setOpen] = React.useState(false);
    const [decision, setDecision] = React.useState<ReviewDecision>('comment');
    const [summary, setSummary] = React.useState('');
    const [busy, setBusy] = React.useState(false);
    const verdict = review.verdict;
    const label: Record<ReviewDecision, string> = { approve: 'Approve', 'request-changes': 'Request changes', comment: 'Comment' };
    if (!open) {
        return <div className='co-review-submit-bar'>
            <span className='co-review-muted'>
                {verdict ? `Round ${verdict.count}: ${label[verdict.decision]}` : 'Not submitted yet'}
            </span>
            <span className='co-review-spacer' />
            <span className='co-review-link' onClick={() => setOpen(true)}>Submit review…</span>
        </div>;
    }
    const submit = async () => {
        setBusy(true);
        try {
            await manager.submitReview(decision, summary.trim());
            setSummary('');
            setOpen(false);
        } finally {
            setBusy(false);
        }
    };
    return <div className='co-review-submit'>
        <div className='co-review-decisions'>
            {(Object.keys(label) as ReviewDecision[]).map(d => <span key={d} className={`co-review-decision ${d} ${decision === d ? 'active' : ''}`}
                onClick={() => setDecision(d)}>{label[d]}</span>)}
        </div>
        <textarea className='theia-input' rows={3} value={summary} placeholder='Message to the agent (optional)'
            onChange={e => setSummary(e.currentTarget.value)} />
        <div className='co-review-composer-actions'>
            <span className='co-review-link' onClick={() => setOpen(false)}>Cancel</span>
            <button className='theia-button' disabled={busy} onClick={submit}>Submit review</button>
        </div>
    </div>;
}

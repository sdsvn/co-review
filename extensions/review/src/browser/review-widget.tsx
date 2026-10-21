import * as React from '@theia/core/shared/react';
import URI from '@theia/core/lib/common/uri';
import { CommandService } from '@theia/core/lib/common/command';
import { Message } from '@theia/core/lib/browser/widgets/widget';
import { ReactWidget } from '@theia/core/lib/browser/widgets/react-widget';
import { codicon } from '@theia/core/lib/browser/widgets/widget';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { AgentConfig, CodeLocation, isOutOfScope, Review, ReviewCoverage, ReviewDecision, ReviewScope, ReviewThread } from '../common/review-model';
import { GitHubEdits, GitHubTarget } from '../common/review-protocol';
import { confirmGitHubPost } from './github-preview';
import { ReviewManager } from './review-manager';
import { ReviewCommands } from './review-commands';
import { ReviewNavigator } from './review-navigator';
import { DraftEditor, FolderMarks, marksOf, ThreadView } from './review-components';
import { agentActivity, agentStatus, ago, severityRank } from './review-insights';

type Filter = 'open' | 'proposed' | 'resolved' | 'all' | 'out-of-scope' | 'files';

/** The panel's single-key shortcuts, for the sheet `?` shows. */
const SHORTCUTS: [string, string][] = [
    ['J / ↓', 'Next card'], ['K / ↑', 'Previous card'], ['Enter', 'Open its code'], ['A', 'Keep the finding (it becomes open)'],
    ['D', 'Dismiss the finding'], ['X', 'Resolve or reopen the thread'], ['R', 'Reply'], ['V', 'Mark its file viewed'],
    ['N', 'Next unviewed file'], ['S', 'Submit review…'], ['?', 'This sheet']
];

/** Review overview: drafts and every thread of the active review, grouped by location. */
@injectable()
export class ReviewWidget extends ReactWidget {

    static readonly ID = 'co-review:panel';
    static readonly LABEL = 'Review';

    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(ReviewNavigator) protected readonly navigator: ReviewNavigator;
    @inject(CommandService) protected readonly commands: CommandService;

    protected filter: Filter = 'open';
    /** Until the reviewer picks a filter, a review with only proposed findings (an agent's first pass) shows those. */
    protected filterChosen = false;
    /** Threads grouped by file (default) or by area: their first label, e.g. the part of the repository a finding is about. */
    protected grouping: 'file' | 'area' = 'file';
    protected selectedThreadId: string | undefined;
    /** How much of the repository the reviewer has viewed; refreshed shortly after the review changes. */
    protected coverage: ReviewCoverage | undefined;
    protected coverageTimer: number | undefined;
    protected pendingScroll = false;
    /** The cards in the order shown, for J/K. */
    protected visible: string[] = [];
    protected submitOpen = false;
    protected sheetOpen = false;
    /** Bumped by R: the selected card's reply box takes the keyboard. */
    protected replySignal = 0;
    /** Folders of the Files tab the reviewer folded. */
    protected readonly folded = new Set<string>();

    @postConstruct()
    protected init(): void {
        this.id = ReviewWidget.ID;
        this.title.label = ReviewWidget.LABEL;
        this.title.caption = 'Repository Review';
        this.title.closable = true;
        this.title.iconClass = codicon('comment-discussion');
        this.addClass('co-review');
        this.node.tabIndex = 0;
        this.node.addEventListener('keydown', e => this.onKey(e));
        this.toDispose.push(this.reviews.onDidChange(() => {
            this.update();
            this.refreshCoverage();
        }));
        this.toDispose.push(this.reviews.onDidChangeDrafts(() => this.update()));
        this.toDispose.push(this.reviews.onDidRequestSubmit(() => {
            this.submitOpen = true;
            this.update();
        }));
        this.toDispose.push(this.reviews.onDidRequestReveal(threadId => {
            this.selectedThreadId = threadId;
            const thread = this.reviews.activeReview?.threads.find(t => t.id === threadId);
            if (thread && (this.filter === 'files' || (this.filter !== 'all' && thread.status !== this.filter))) {
                this.filter = thread.status === 'proposed' ? 'proposed' : thread.status;
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

    /** Files viewed out of all, for the Files tab: a change's files, else the repository's (coverage). */
    protected files(review: Review): { viewed: number; total: number } | undefined {
        const change = this.navigator.changeProgress();
        if (change) {
            return { viewed: change.viewed, total: change.files.length };
        }
        return !review.bundle && this.coverage?.total ? { viewed: this.coverage.viewed, total: this.coverage.total } : undefined;
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

    /** Single keys while the panel has focus and the reviewer isn't typing (see SHORTCUTS). */
    protected onKey(e: KeyboardEvent): void {
        const target = e.target as HTMLElement;
        if (e.metaKey || e.ctrlKey || e.altKey || target.closest('input, textarea, select, [contenteditable="true"]')) {
            return;
        }
        const review = this.reviews.activeReview;
        const thread = review?.threads.find(t => t.id === this.selectedThreadId);
        const move = (by: number) => {
            if (!this.visible.length) {
                return;
            }
            const at = this.selectedThreadId ? this.visible.indexOf(this.selectedThreadId) : -1;
            this.selectedThreadId = this.visible[Math.max(0, Math.min(this.visible.length - 1, at < 0 ? 0 : at + by))];
            this.pendingScroll = true;
            this.update();
        };
        const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
        const handled = () => {
            e.preventDefault();
            e.stopPropagation();
        };
        switch (key) {
            case 'j': case 'ArrowDown': handled(); return move(1);
            case 'k': case 'ArrowUp': handled(); return move(-1);
            case '?': handled(); this.sheetOpen = !this.sheetOpen; this.update(); return;
            case 'Escape': if (this.sheetOpen) { handled(); this.sheetOpen = false; this.update(); } return;
            case 'n': handled(); this.navigator.openNextUnviewed(); return;
            case 's': if (review && review.purpose !== 'explain') { handled(); this.submitOpen = true; this.update(); } return;
        }
        if (!thread || !review) {
            return;
        }
        switch (key) {
            case 'Enter': case 'o': handled(); this.openThread(thread); return;
            case 'a': if (thread.status === 'proposed') { handled(); this.reviews.setStatus(thread, 'open'); move(1); } return;
            case 'd': if (thread.status === 'proposed') { handled(); this.reviews.setStatus(thread, 'resolved'); move(1); } return;
            case 'x': if (thread.status !== 'proposed') { handled(); this.reviews.setStatus(thread, thread.status === 'resolved' ? 'open' : 'resolved'); } return;
            case 'r': if (thread.status !== 'resolved') { handled(); this.replySignal++; this.update(); } return;
            case 'v': {
                const uri = this.fileOf(thread.location);
                if (uri) {
                    handled();
                    this.reviews.setViewed([uri], !this.reviews.isDone(uri));
                }
                return;
            }
        }
    }

    /** The file a thread is on, as a URI the reviewer can mark viewed. */
    protected fileOf(location: CodeLocation): string | undefined {
        const file = this.reviews.patchFile(location);
        const root = this.reviews.root;
        if (file && root) {
            return new URI(root).resolve(file.path).toString();
        }
        return location.kind !== 'repository' && location.kind !== 'directory' && location.uri?.startsWith('file:') ? location.uri : undefined;
    }

    protected render(): React.ReactNode {
        const review = this.reviews.activeReview;
        return <div className='co-review-root'>
            {this.renderHeader(review)}
            {review ? this.renderReview(review) : this.renderEmpty()}
            {this.sheetOpen && <div className='co-review-sheet' onClick={() => { this.sheetOpen = false; this.update(); }}>
                <div className='co-review-sheet-card'>
                    <div className='co-review-sheet-title'>Keyboard, in this panel</div>
                    {SHORTCUTS.map(([k, what]) => <div key={k} className='co-review-sheet-row'><kbd>{k}</kbd><span>{what}</span></div>)}
                    <div className='co-review-muted'>In the editor: ⌘⌥M comment, ⌘⌥A ask the agent, ⌘⌥V viewed, ⌘⌥↓/↑ next/previous comment.</div>
                </div>
            </div>}
        </div>;
    }

    protected renderHeader(review: Review | undefined): React.ReactNode {
        const all = this.reviews.reviews;
        const options = review && !all.some(r => r.id === review.id) ? [...all, review] : all;
        const status = review && agentStatus(review, this.reviews.getAgentPresence(review.id));
        const agent = review?.agent;
        return <div className='co-review-header'>
            <select className='theia-select co-review-select' value={review?.id ?? ''}
                title={review ? `${review.title} — ${ReviewScope.label(review.scope, uri => this.reviews.relativePath(uri))}` : 'Active review'}
                onChange={e => this.reviews.setActiveReview(e.currentTarget.value || undefined)}>
                {!review && <option value=''>No active review</option>}
                {options.map(r => <option key={r.id} value={r.id}>{r.archivedAt ? `${r.title} (archived)` : r.title}</option>)}
            </select>
            {review && <span className={`co-review-agent-pill ${status!.state}`} title={`${status!.title}. Click to choose the agent.`}
                onClick={() => this.commands.executeCommand(ReviewCommands.CONFIGURE_AGENT.id)}>
                <span className='co-review-agent-dot' />{agent ? status!.label.replace(/ since .*/, '') : 'Connect agent'}
            </span>}
            {AgentConfig.isAcp(agent) && <span className={`${codicon('settings-gear')} action-label`}
                title={`Model: ${agent?.settingsLabel ?? (Object.values(agent?.settings ?? {}).join(' · ') || 'the agent\'s default')} — click to choose`}
                onClick={() => this.commands.executeCommand(ReviewCommands.AGENT_SETTINGS.id)} />}
            {this.reviews.windowReviewId && <span className={`${codicon('files')} action-label`} title='Show or hide the explorer (⌘B)'
                onClick={() => this.commands.executeCommand('core.toggle.left.panel')} />}
            {review && <span className={`${codicon('map')} action-label`} title='Overview: what this is, how it works, every finding'
                onClick={() => this.commands.executeCommand(ReviewCommands.OPEN_OVERVIEW.id)} />}
            <span className={`${codicon('add')} action-label`} title='New review' onClick={() => this.commands.executeCommand(ReviewCommands.CREATE_REVIEW.id)} />
            {(review || this.reviews.archivedReviews.length > 0) && <span className={`${codicon('ellipsis')} action-label`} title='More actions'
                onClick={() => this.commands.executeCommand(ReviewCommands.REVIEW_ACTIONS.id)} />}
        </div>;
    }

    protected renderEmpty(): React.ReactNode {
        return <div className='co-review-empty'>
            <span className={`${codicon('comment-discussion')} co-review-empty-icon`} />
            <p><b>No review is open for this repository.</b></p>
            <p>A review can span the whole repository, a branch, a commit, or selected files and folders.</p>
            <button className='theia-button' onClick={() => this.commands.executeCommand(ReviewCommands.CREATE_REVIEW.id)}>Start a Review</button>
            <p>Want Claude Code, Pi, Oh My Pi or another agent to answer your questions and open reviews of its own work?</p>
            <button className='theia-button secondary' onClick={() => this.commands.executeCommand(ReviewCommands.SETUP_HARNESS.id)}>Connect an Agent</button>
        </div>;
    }

    /** The agent's last moves, newest first; a click opens the thread. */
    protected renderActivity(review: Review): React.ReactNode {
        const entries = agentActivity(review, 3);
        if (!entries.length) {
            return undefined;
        }
        return <div className='co-review-feed' aria-label='Agent activity'>
            {entries.map(e => <div key={e.key} className={`co-review-feed-item ${e.threadId ? 'link' : ''}`}
                onClick={() => e.threadId && this.reviews.revealThread(e.threadId)}>
                <span className={codicon('sparkle')} />
                <span className='co-review-feed-text'>{e.text}</span>
                <span className='co-review-feed-time'>{ago(e.at)}</span>
            </div>)}
        </div>;
    }

    protected renderReview(review: Review): React.ReactNode {
        // Out of scope (the commented code removed, or ambiguous): its own tab, like resolved ones, not among the open.
        const inScope = (t: ReviewThread) => !isOutOfScope(t);
        const open = review.threads.filter(t => t.status === 'open' && inScope(t)).length;
        const proposed = review.threads.filter(t => t.status === 'proposed' && inScope(t)).length;
        const resolved = review.threads.filter(t => t.status === 'resolved').length;
        const outOfScope = review.threads.filter(isOutOfScope).length;
        if (!this.filterChosen && this.filter === 'open' && !open && proposed) {
            this.filter = 'proposed';
        }
        if (this.filter === 'proposed' && !proposed && this.filterChosen === false) {
            this.filter = 'open';
        }
        const files = this.files(review);
        if (this.filter === 'files' && !files) {
            this.filter = 'open';
        }
        // To triage: the most severe first; elsewhere by place.
        const threads = review.threads
            .filter(t => this.filter === 'all' || (this.filter === 'out-of-scope' ? isOutOfScope(t) : t.status === this.filter && (this.filter === 'resolved' || inScope(t))))
            .sort((a, b) => this.sortKey(a).localeCompare(this.sortKey(b)) || a.number - b.number);
        const groups = new Map<string, ReviewThread[]>();
        for (const thread of threads) {
            const key = this.grouping === 'area' ? thread.labels?.[0] ?? 'Other' : this.groupKey(thread.location);
            groups.set(key, [...(groups.get(key) ?? []), thread]);
        }
        const ordered = [...groups.entries()]
            .sort(([a, ga], [b, gb]) => this.filter === 'proposed'
                ? Math.max(...gb.map(t => severityRank(t.severity))) - Math.max(...ga.map(t => severityRank(t.severity)))
                : this.grouping === 'area' ? a.localeCompare(b) : 0)
            .map(([key, group]) => [key, this.filter === 'proposed'
                ? [...group].sort((a, b) => severityRank(b.severity) - severityRank(a.severity) || a.number - b.number) : group] as const);
        this.visible = this.filter === 'files' ? [] : ordered.flatMap(([, group]) => group.map(t => t.id));
        const drafts = this.reviews.drafts;
        return <>
            {this.renderActivity(review)}
            {review.purpose === 'explain' && <div className='co-review-submit-bar'>
                <span className='co-review-muted' title='The agent opened this to explain something: there is no verdict to submit'>
                    An explanation: nothing to submit. Ask anything, then close it.</span>
                <span className='co-review-spacer' />
                <button className='theia-button' title='Close this window: the agent knows you are done' onClick={() => this.reviews.finish()}>Done</button>
            </div>}
            {(review.bundle || review.agent) && review.purpose !== 'explain' && <SubmitReview review={review} manager={this.reviews} open={this.submitOpen}
                onOpenChange={o => { this.submitOpen = o; this.update(); }}
                pending={{
                    open, proposed, drafts: drafts.filter(d => d.body.trim()).length,
                    unviewed: files ? files.total - files.viewed : undefined
                }} />}
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
            <div className='co-review-filters' role='tablist'>
                {proposed > 0 && this.renderFilter('proposed', 'To triage', proposed)}
                {this.renderFilter('open', 'Open', open)}
                {this.renderFilter('resolved', 'Resolved', resolved)}
                {outOfScope > 0 && this.renderFilter('out-of-scope', 'Out of scope', outOfScope)}
                {this.renderFilter('all', 'All', review.threads.length)}
                {files && this.renderFilter('files', 'Files', `${files.viewed}/${files.total}`)}
                <span className='co-review-spacer' />
                {this.filter !== 'files' && review.threads.some(t => t.labels?.length) &&
                    <span className={`co-review-filter co-review-grouping ${this.grouping === 'area' ? 'active' : ''}`}
                        title='Group threads by area (their first label) instead of by file'
                        onClick={() => { this.grouping = this.grouping === 'area' ? 'file' : 'area'; this.update(); }}>
                        <span className={codicon('tag')} />
                    </span>}
                <span className={`${codicon('keyboard')} action-label co-review-keys`} title='Keyboard shortcuts (?)'
                    onClick={() => { this.sheetOpen = true; this.update(); }} />
            </div>
            {this.filter === 'files' ? this.renderFiles(review) : <div className='co-review-threads' role='list'>
                {threads.length === 0 && this.renderNoThreads(review, { open, proposed, resolved })}
                {ordered.map(([key, group]) => <div key={key} className='co-review-group'>
                    {this.grouping === 'area'
                        ? <div className='co-review-group-header' title={`${group.length} thread${group.length === 1 ? '' : 's'} labelled ${key}`}>
                            <span className={codicon('tag')} />
                            <span className='co-review-group-label'>{key}</span>
                            <span className='co-review-group-count'>{group.length}</span>
                        </div>
                        : <div className='co-review-group-header' title={key}>
                            <span className={codicon(group[0].location.kind === 'repository' ? 'repo' : group[0].location.kind === 'directory' ? 'folder' : 'file')} />
                            <span className='co-review-group-label' onClick={() => this.openGroup(group[0])}>{key.split('/').pop() || key}</span>
                            {key.includes('/') && <span className='co-review-group-dir'>{key.slice(0, key.lastIndexOf('/') + 1)}</span>}
                        </div>}
                    {group.map(thread => <ThreadView key={thread.id}
                        manager={this.reviews}
                        thread={thread}
                        label={this.reviews.locationLabel(thread.location)}
                        anchorState={this.reviews.getAnchorState(thread.id)}
                        selected={thread.id === this.selectedThreadId}
                        replySignal={thread.id === this.selectedThreadId ? this.replySignal : undefined}
                        underFile={this.grouping === 'file' && thread.location.kind !== 'repository' && thread.location.kind !== 'directory'}
                        onSelect={() => { this.selectedThreadId = thread.id; this.update(); }}
                        onOpen={() => this.openThread(thread)}
                        onOpenReference={(path, line, end) => this.navigator.openReference(path, line, end)} />)}
                </div>)}
            </div>}
        </>;
    }

    /** What an empty tab says: what is going on, and what to do next. */
    protected renderNoThreads(review: Review, counts: { open: number; proposed: number; resolved: number }): React.ReactNode {
        const agent = review.agent;
        if (!review.threads.length) {
            return <div className='co-review-hint co-review-empty-state'>
                {review.purpose === 'explain'
                    ? <><span className={codicon('comment-discussion')} /> <b>Ask {agent?.name ?? 'the agent'} anything.</b> Hover a line and click + to ask
                        about it, or select text and choose Ask Agent. Click Done when you have what you need.</>
                    : agent
                    ? <><span className={agentStatus(review, this.reviews.getAgentPresence(review.id)).state === 'working' ? `${codicon('loading')} codicon-modifier-spin` : codicon('sparkle')} />
                        <b>{agent.name} is on its first pass.</b> Its findings show up here as it adds them. Meanwhile, hover a line in the
                        editor and click + to comment, or select code and choose Ask Agent.</>
                    : <>Hover a line in the editor and click + to comment (drag for several lines).
                        To ask an agent about code, <span className='co-review-link'
                            onClick={() => this.commands.executeCommand(ReviewCommands.CONFIGURE_AGENT.id)}>connect one</span>.</>}
            </div>;
        }
        if (this.filter === 'open' && !counts.open && !counts.proposed) {
            return <div className='co-review-hint co-review-empty-state'>
                <span className={codicon('pass-filled')} /> <b>Everything is resolved.</b>
                {(review.bundle || agent) && review.purpose !== 'explain' && <> Submit your review to tell {agent?.name ?? 'the agent'}. <span className='co-review-link'
                    onClick={() => { this.submitOpen = true; this.update(); }}>Submit review…</span></>}
                {review.purpose === 'explain' && <> Close it when you are done: <span className='co-review-link' onClick={() => this.reviews.finish()}>Done</span></>}
            </div>;
        }
        return <div className='co-review-hint'>{this.filter === 'proposed' ? 'Nothing left to triage.' : `No ${this.filter === 'out-of-scope' ? 'out-of-scope' : this.filter} threads.`}</div>;
    }

    /** The Files tab: a change's files by folder, each with its viewed check; a repository's areas as a folder tree. */
    protected renderFiles(review: Review): React.ReactNode {
        const root = this.reviews.root;
        const change = this.navigator.changeProgress();
        const files = this.files(review);
        const head = files && <div className='co-review-files-head'>
            <span>Viewed {files.viewed} of {files.total} files</span>
            <span className='co-review-progress'><span style={{ width: `${Math.round(100 * files.viewed / Math.max(files.total, 1))}%` }} /></span>
            <span className='co-review-spacer' />
            {files.viewed < files.total && <span className='co-review-link' title='Open the next file you have not viewed (N)'
                onClick={() => this.navigator.openNextUnviewed()}>Next unviewed</span>}
        </div>;
        if (change && root) {
            const stats = new Map(this.navigator.changeFiles().map(f => [f.path, f]));
            // Open comments per file: on its diff, or on its code.
            const comments = new Map<string, number>();
            for (const t of review.threads.filter(th => th.status !== 'resolved')) {
                const path = this.reviews.patchFile(t.location)?.path ?? (t.location.uri?.startsWith('file:') ? this.reviews.relativePath(t.location.uri) : undefined);
                if (path) {
                    comments.set(path, (comments.get(path) ?? 0) + 1);
                }
            }
            const folders = new Map<string, string[]>();
            for (const path of change.files) {
                const dir = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
                folders.set(dir, [...folders.get(dir) ?? [], path]);
            }
            return <div className='co-review-files'>
                {head}
                {[...folders].sort(([a], [b]) => a.localeCompare(b)).map(([dir, paths]) => <div key={dir} className='co-review-files-folder'>
                    <div className='co-review-files-dir' onClick={() => { this.toggleFold(dir); }}>
                        <span className={codicon(this.folded.has(dir) ? 'chevron-right' : 'chevron-down')} />
                        <span className={codicon('folder')} /><span className='co-review-files-dir-name'>{dir || '(root)'}</span>
                        <FolderMarks state={marksOf(paths.map(p => this.reviews.pathState(p)))} onMark={state => this.reviews.setPathState(paths, state)} />
                    </div>
                    {!this.folded.has(dir) && paths.map(path => {
                        const state = this.reviews.pathState(path);
                        const viewed = !!state;
                        const f = stats.get(path);
                        return <div key={path} className={`co-review-files-file ${viewed ? 'viewed' : ''} ${state === 'skipped' ? 'skipped' : ''}`} title={`Open the diff of ${path}`}
                            onClick={() => this.navigator.openChangedFile(path)}>
                            {state === 'skipped'
                                ? <span className={`${codicon('circle-slash')} co-review-files-skipped`} title="Don't care: click to unmark" role='button'
                                    onClick={e => { e.stopPropagation(); this.reviews.setPathState([path], false); }} />
                                : <input type='checkbox' checked={viewed} title={viewed ? 'Viewed: click to unmark' : 'Mark as viewed'}
                                    onClick={e => e.stopPropagation()} onChange={() => this.reviews.setPathState([path], !viewed)} />}
                            <span className='name'>{path.slice(path.lastIndexOf('/') + 1)}</span>
                            {comments.get(path) && <span className='co-review-changed-comments' title={`${comments.get(path)} open comment${comments.get(path) === 1 ? '' : 's'}`}>
                                <span className={codicon('comment-discussion')} />{comments.get(path)}</span>}
                            {f && <span className='co-review-files-count'><span className='add'>+{f.added}</span> <span className='del'>−{f.removed}</span></span>}
                        </div>;
                    })}
                </div>)}
            </div>;
        }
        const coverage = this.coverage;
        if (!coverage?.areas.length) {
            return <div className='co-review-files'>{head}</div>;
        }
        // Areas are folders: drawn as a tree, each folder by its own name, with its viewed count and bar.
        type Node = { name: string; path: string; viewed: number; skipped: number; total: number; next?: string; children: Map<string, Node> };
        const tree: Node = { name: '', path: '', viewed: 0, skipped: 0, total: 0, children: new Map() };
        for (const area of coverage.areas) {
            let node = tree;
            const parts = area.path === '.' ? ['(root)'] : area.path.split('/');
            parts.forEach((part, i) => {
                const path = parts.slice(0, i + 1).join('/');
                if (!node.children.has(part)) {
                    node.children.set(part, { name: part, path, viewed: 0, skipped: 0, total: 0, children: new Map() });
                }
                node = node.children.get(part)!;
                node.viewed += area.viewed;
                node.skipped += area.skipped;
                node.total += area.total;
                node.next = node.next ?? area.next;
            });
        }
        // A folder with a single subfolder and no files of its own reads as one line ("extensions/review/src").
        const squash = (node: Node): Node => {
            const kids = [...node.children.values()].map(squash);
            if (kids.length === 1 && kids[0].total === node.total && node.name) {
                return { ...kids[0], name: `${node.name}/${kids[0].name}` };
            }
            return { ...node, children: new Map(kids.map(k => [k.name, k])) };
        };
        const bar = (viewed: number, total: number) => <span className='co-review-progress'>
            <span style={{ width: `${Math.round(100 * viewed / Math.max(total, 1))}%` }} />
        </span>;
        const draw = (node: Node, depth: number): React.ReactNode => <div key={node.path}>
            <div className={`co-review-coverage-area ${node.next ? '' : 'done'}`} style={{ paddingLeft: 6 + depth * 12 }}
                title={node.next ? `${node.path}: open the next file to review, ${node.next}` : `${node.path}: all files viewed`}
                onClick={() => node.next && this.navigator.openReference(node.next)}>
                {node.children.size > 0
                    ? <span className={codicon(this.folded.has(node.path) ? 'chevron-right' : 'chevron-down')}
                        onClick={e => { e.stopPropagation(); this.toggleFold(node.path); }} />
                    : <span className='co-review-coverage-leaf' />}
                <span className={codicon(node.next ? 'folder' : 'pass-filled')} />
                <span className='co-review-coverage-path'>{node.name}</span>
                <span className='co-review-coverage-count'>{node.viewed}/{node.total}</span>
                {bar(node.viewed, node.total)}
                <FolderMarks state={node.viewed < node.total ? undefined : node.skipped === node.total ? 'skipped' : node.skipped ? undefined : 'viewed'}
                    onMark={state => this.reviews.setPathState([node.path === '(root)' ? './' : `${node.path}/`], state)} />
            </div>
            {!this.folded.has(node.path) && [...node.children.values()].map(child => draw(child, depth + 1))}
        </div>;
        return <div className='co-review-files'>
            {head}
            <div className='co-review-coverage-areas'>{[...squash(tree).children.values()].map(node => draw(node, 0))}</div>
        </div>;
    }

    protected toggleFold(path: string): void {
        if (!this.folded.delete(path)) {
            this.folded.add(path);
        }
        this.update();
    }

    protected async openThread(thread: ReviewThread): Promise<void> {
        this.reviews.setCollapsed(thread.id, false);
        this.reviews.markRead(thread.id);
        await this.navigator.open(thread.location, thread.id);
    }

    /** A group's file: for comments on the diff, the file they are on (as the change); else the location's file. */
    protected async openGroup(thread: ReviewThread): Promise<void> {
        const file = this.reviews.patchFile(thread.location);
        if (file) {
            await this.navigator.openChangedFile(file.path);
        } else if (thread.location.uri) {
            await this.navigator.open({ kind: 'file', uri: thread.location.uri });
        }
    }

    protected async openDraft(location: CodeLocation, draftId: string): Promise<void> {
        await this.navigator.open(location, undefined, true);
        this.reviews.focusDraft(draftId);
    }

    protected renderFilter(filter: Filter, label: string, count: number | string): React.ReactNode {
        return <span role='tab' aria-selected={this.filter === filter} className={`co-review-filter ${this.filter === filter ? 'active' : ''} ${filter === 'proposed' ? 'triage' : ''}`}
            onClick={() => { this.filter = filter; this.filterChosen = true; this.update(); }}>
            {label} <span className='co-review-filter-count'>{count}</span>
        </span>;
    }

    protected groupKey(location: CodeLocation): string {
        if (location.kind === 'repository' || !location.uri) {
            return 'Repository';
        }
        // A comment on a file of the diff is grouped with that file, not under the patch.
        const file = this.reviews.patchFile(location);
        if (file) {
            return file.path;
        }
        const path = this.reviews.relativePath(location.uri);
        return location.kind === 'directory' ? `${path}/` : path;
    }

    protected sortKey(thread: ReviewThread): string {
        const key = this.groupKey(thread.location);
        const start = this.reviews.patchFile(thread.location)?.line ?? (thread.location.range ? thread.location.range.start.line + 1 : 0);
        const line = String(start).padStart(8, '0');
        return (key === 'Repository' ? '' : key) + '\u0000' + line;
    }
}

interface Pending {
    /** Open threads: what the agent gets. */
    open: number;
    /** Findings not triaged: not sent. */
    proposed: number;
    /** Drafts with text, not posted yet. */
    drafts: number;
    /** Files not viewed yet, when known. */
    unviewed?: number;
}

/** Submit a review round to the agent: a decision and a message, after saying what goes and what doesn't. */
function SubmitReview({ review, manager, open, onOpenChange, pending }: {
    review: Review; manager: ReviewManager; open: boolean; onOpenChange(open: boolean): void; pending: Pending
}): React.ReactElement {
    const [decision, setDecision] = React.useState<ReviewDecision>('comment');
    const [summary, setSummary] = React.useState('');
    const [busy, setBusy] = React.useState(false);
    // A pull request's review can also be posted to GitHub; the reviewer opts in each time.
    const [github, setGithub] = React.useState<GitHubTarget | undefined>();
    const [post, setPost] = React.useState(false);
    const [note, setNote] = React.useState<{ text: string; url?: string; error?: boolean } | undefined>();
    const textarea = React.useRef<HTMLTextAreaElement>(null);
    React.useEffect(() => {
        let live = true;
        manager.getGitHubTarget().then(t => live && setGithub(t));
        return () => { live = false; };
    }, [review.id, review.bundle?.dir]);
    React.useEffect(() => {
        if (open) {
            requestAnimationFrame(() => textarea.current?.focus({ preventScroll: false }));
        }
    }, [open]);
    const verdict = review.verdict;
    const agent = review.agent?.name ?? 'the agent';
    const label: Record<ReviewDecision, string> = { approve: 'Approve', 'request-changes': 'Request changes', comment: 'Comment' };
    if (!open) {
        return <div className='co-review-submit-bar'>
            <span className='co-review-muted'>
                {verdict ? `Round ${verdict.count}: ${label[verdict.decision]}` : 'Not submitted yet'}
                {verdict?.toAgent === 'waiting' && ` · waiting for ${agent} to pick it up`}
                {verdict?.toAgent === 'delivered' && ` · ${agent} has it`}
            </span>
            <span className='co-review-spacer' />
            <span className='co-review-link' title='Submit review (S)' onClick={() => onOpenChange(true)}>Submit review…</span>
            {note && <span className={`co-review-github-note ${note.error ? 'error' : ''}`}>
                {note.text} {note.url && <span className='co-review-link' onClick={() => window.open(note.url, '_blank', 'noopener')}>Open on GitHub</span>}
            </span>}
        </div>;
    }
    const submit = async () => {
        if (busy) {
            return;
        }
        setBusy(true);
        try {
            // Posting too: the reviewer sees exactly what goes to GitHub first; cancelling keeps the form open to edit.
            let edits: GitHubEdits | undefined;
            if (post && github) {
                let preview;
                try {
                    preview = await manager.previewGitHubPost(decision, summary.trim());
                } catch (e) {
                    setNote({ text: String(e instanceof Error ? e.message : e).replace(/^Error: /, ''), error: true });
                    return;
                }
                edits = await confirmGitHubPost(github, preview);
                if (!edits) {
                    return;
                }
            }
            await manager.submitReview(decision, summary.trim(), !!(post && github));
            setSummary('');
            onOpenChange(false);
            setNote(undefined);
            if (post && github) {
                setNote({ text: `Posting to ${github.repo}#${github.number}…` });
                try {
                    const posted = await manager.postToGitHub(decision, summary.trim(), edits);
                    setNote({
                        text: `Posted to GitHub: ${posted.comments} line comment${posted.comments === 1 ? '' : 's'}`
                            + `${posted.inBody ? `, ${posted.inBody} in the body` : ''}.${posted.notes.length ? ` ${posted.notes.join(' ')}` : ''}`,
                        url: posted.url
                    });
                } catch (e) {
                    setNote({ text: String(e instanceof Error ? e.message : e).replace(/^Error: /, ''), error: true });
                }
            }
        } finally {
            setBusy(false);
        }
    };
    const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
    return <div className='co-review-submit'>
        <div className='co-review-submit-title'>Submit review</div>
        <ul className='co-review-submit-summary'>
            <li><b>{plural(pending.open, 'open thread', 'open threads')}</b> {pending.open === 1 ? 'goes' : 'go'} to {agent}</li>
            {pending.drafts > 0 && <li className='warn'><b>{plural(pending.drafts, 'draft', 'drafts')}</b> not posted yet: they won't be sent.
                {' '}<span className='co-review-link' onClick={() => manager.submitAllDrafts()}>Post {pending.drafts === 1 ? 'it' : 'them'}</span></li>}
            {pending.proposed > 0 && <li className='warn'><b>{plural(pending.proposed, 'finding', 'findings')}</b> not triaged: {pending.proposed === 1 ? 'it' : 'they'} won't be sent</li>}
            {pending.unviewed !== undefined && pending.unviewed > 0 && <li><b>{plural(pending.unviewed, 'file', 'files')}</b> not viewed yet</li>}
        </ul>
        <div className='co-review-decisions'>
            {(Object.keys(label) as ReviewDecision[]).map(d => <span key={d} className={`co-review-decision ${d} ${decision === d ? 'active' : ''}`}
                onClick={() => setDecision(d)}>{label[d]}</span>)}
        </div>
        <textarea ref={textarea} className='theia-input' rows={3} value={summary}
            placeholder={`${post ? 'Message to the agent and on GitHub' : 'Message to the agent'} (optional) — ⌘/Ctrl+Enter submits`}
            onChange={e => setSummary(e.currentTarget.value)}
            onKeyDown={e => {
                e.stopPropagation();
                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    submit();
                } else if (e.key === 'Escape') {
                    onOpenChange(false);
                }
            }} />
        {github && <label className='co-review-github-post' title='Your open comments on diff lines become line comments; the rest goes into the review. You see exactly what will be posted, and can edit it, before it goes.'>
            <input type='checkbox' checked={post} onChange={e => setPost(e.currentTarget.checked)} />
            Also post to GitHub: {github.repo}#{github.number}
            {github.postedRound !== undefined && <span className='co-review-muted'> (round {github.postedRound} was posted)</span>}
        </label>}
        <div className='co-review-composer-actions'>
            <span className='co-review-link' onClick={() => onOpenChange(false)}>Cancel</span>
            <button className='theia-button' disabled={busy} onClick={submit}>{label[decision]}</button>
        </div>
    </div>;
}

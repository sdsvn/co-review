import * as React from '@theia/core/shared/react';
import * as DOMPurify from '@theia/core/shared/dompurify';
import * as markdownit from '@theia/core/shared/markdown-it';
import { codicon } from '@theia/core/lib/browser/widgets/widget';
import { AgentActivity, PermissionRequest, Proposal, ReviewMessage, ReviewThread, Severity, ThreadIntent } from '../common/review-model';
import { ReviewDraft, ReviewManager } from './review-manager';

/** Shared review UI, rendered both in the review panel and inline in editors. */

export function formatTime(ms: number): string {
    const d = new Date(ms);
    const sameDay = d.toDateString() === new Date().toDateString();
    return sameDay ? d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : d.toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
}

function isSubmitKey(e: React.KeyboardEvent): boolean {
    return e.key === 'Enter' && (e.metaKey || e.ctrlKey);
}

/** Keeps editor/workbench key handling (e.g. typing in a Monaco overlay) out of our text areas. */
function stopKeys(e: React.KeyboardEvent): void {
    if (!isSubmitKey(e) && e.key !== 'Escape') {
        e.stopPropagation();
    }
}

/** Opens `path:line` references (relative to the repository) in the editor. */
export type OpenReference = (path: string, line?: number, endLine?: number) => void;

const markdown = markdownit({ html: false, linkify: false, breaks: true });
/** `internal/orders/service.go:84`, `service.go:10-12` — not preceded by a path character or `://`. */
const REFERENCE = /(^|[^\w/.:-])((?:[\w.-]+\/)*[\w.-]+\.[A-Za-z][\w]*):(\d+)(?:-(\d+))?/g;

function escapeAttr(text: string): string {
    return text.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
}

/** Renders markdown and turns `path:line` references into links (outside existing links). */
function renderBody(body: string): string {
    const html = markdown.render(body);
    let inLink = 0;
    const linked = html.split(/(<[^>]+>)/).map(part => {
        if (part.startsWith('<')) {
            if (/^<a[\s>]/i.test(part)) { inLink++; }
            if (/^<\/a>/i.test(part)) { inLink = Math.max(0, inLink - 1); }
            return part;
        }
        return inLink ? part : part.replace(REFERENCE, (_m, lead: string, file: string, line: string, end?: string) =>
            `${lead}<a class="co-review-ref" href="#" data-path="${escapeAttr(file)}" data-line="${line}"${end ? ` data-end="${end}"` : ''}>${file}:${line}${end ? `-${end}` : ''}</a>`);
    }).join('');
    return DOMPurify.sanitize(linked, { ALLOW_DATA_ATTR: true });
}

function MarkdownBody({ body, onOpenReference }: { body: string; onOpenReference?: OpenReference }): React.ReactElement {
    const html = React.useMemo(() => renderBody(body), [body]);
    return <div className='co-review-message-body markdown' dangerouslySetInnerHTML={{ __html: html }}
        onClick={e => {
            const link = (e.target as HTMLElement).closest('a');
            if (!link) {
                return;
            }
            e.preventDefault();
            e.stopPropagation();
            if (link.classList.contains('co-review-ref') && onOpenReference) {
                const end = link.getAttribute('data-end');
                onOpenReference(link.getAttribute('data-path')!, Number(link.getAttribute('data-line')), end ? Number(end) : undefined);
            } else if (/^https?:/.test(link.getAttribute('href') ?? '')) {
                window.open(link.getAttribute('href')!, '_blank', 'noopener');
            }
        }} />;
}

const STATUS_ICON: Record<AgentActivity['status'], string> = {
    pending: 'circle-large-outline', in_progress: 'loading codicon-modifier-spin', completed: 'check', failed: 'error'
};

/** Concise agent activity ("✓ Reading service.go"); the full list folds away once the answer is done. */
function ActivityList({ activity, streaming }: { activity: AgentActivity[]; streaming: boolean }): React.ReactElement | null {
    const [open, setOpen] = React.useState(false);
    if (!activity.length) {
        return null;
    }
    const shown = streaming ? activity.slice(-4) : open ? activity : [];
    return <div className='co-review-activity'>
        {!streaming && <div className='co-review-activity-summary' onClick={e => { e.stopPropagation(); setOpen(!open); }}>
            <span className={codicon(open ? 'chevron-down' : 'chevron-right')} />
            {activity.length} step{activity.length === 1 ? '' : 's'}
            {activity.some(a => a.status === 'failed') && <span className='co-review-activity-failed'> · {activity.filter(a => a.status === 'failed').length} failed</span>}
        </div>}
        {streaming && activity.length > 4 && <div className='co-review-activity-more'>… {activity.length - 4} earlier steps</div>}
        {shown.map(a => <div key={a.id} className={`co-review-activity-item ${a.status}`}>
            <span className={codicon(STATUS_ICON[a.status] ?? 'circle-large-outline')} />
            <span className='co-review-activity-title'>{a.title}</span>
        </div>)}
    </div>;
}

function PermissionView({ request, onAnswer }: { request: PermissionRequest; onAnswer?(requestId: string, optionId: string): void }): React.ReactElement {
    if (request.outcome) {
        const chosen = request.options.find(o => o.id === request.outcome);
        const allowed = chosen?.kind.startsWith('allow');
        return <div className={`co-review-permission answered ${allowed ? 'allowed' : 'rejected'}`}>
            <span className={codicon(allowed ? 'check' : 'circle-slash')} />
            {chosen ? chosen.name : 'Cancelled'}: {request.title}
        </div>;
    }
    return <div className='co-review-permission'>
        <div className='co-review-permission-title'><span className={codicon('shield')} /> The agent asks: <b>{request.title}</b></div>
        <div className='co-review-permission-options'>
            {request.options.map(o => <button key={o.id} className={`theia-button ${o.kind.startsWith('allow') ? '' : 'secondary'}`}
                onClick={e => { e.stopPropagation(); onAnswer?.(request.id, o.id); }}>{o.name}</button>)}
        </div>
    </div>;
}

/** A suggested edit as before/after lines, with Accept / Reject while pending. */
function ProposalView({ proposal, onDecide }: { proposal: Proposal; onDecide?(accept: boolean): void }): React.ReactElement {
    return <div className={`co-review-proposal ${proposal.status}`}>
        <div className='co-review-proposal-head'>
            <span className={codicon('edit')} /> Suggested edit
            {proposal.path && <span className='co-review-muted'> · {proposal.path}{proposal.startLine ? `:${proposal.startLine}` : ''}</span>}
            <span className='co-review-spacer' />
            {proposal.status === 'pending' && onDecide ? <>
                <span className='co-review-link' onClick={e => { e.stopPropagation(); onDecide(false); }}>Reject</span>
                <button className='theia-button' onClick={e => { e.stopPropagation(); onDecide(true); }}>Accept edit</button>
            </> : <span className={`co-review-badge ${proposal.status === 'accepted' ? 'resolved' : ''}`}>{proposal.status}</span>}
        </div>
        <pre className='co-review-proposal-before'>{proposal.before.split('\n').map(l => `− ${l}`).join('\n')}</pre>
        <pre className='co-review-proposal-after'>{proposal.after.split('\n').map(l => `+ ${l}`).join('\n')}</pre>
    </div>;
}

export interface MessageViewProps {
    message: ReviewMessage;
    onOpenReference?: OpenReference;
    onAnswerPermission?(requestId: string, optionId: string): void;
}

export function MessageView({ message, onOpenReference, onAnswerPermission }: MessageViewProps): React.ReactElement {
    const streaming = message.status === 'streaming';
    return <div className={`co-review-message ${message.author.kind} ${message.status ?? ''}`}>
        <div className='co-review-message-meta'>
            <span className={codicon(message.author.kind === 'agent' ? 'hubot' : message.author.kind === 'system' ? 'info' : 'account')} />
            <span className='co-review-author'>{message.author.name}</span>
            {streaming && <span className='co-review-working'><span className={codicon('loading') + ' codicon-modifier-spin'} /> working…</span>}
            {message.status === 'cancelled' && <span className='co-review-badge'>stopped</span>}
            {message.status === 'error' && <span className='co-review-badge outdated'>error</span>}
            <span className='co-review-time'>{formatTime(message.createdAt)}</span>
        </div>
        {message.activity && <ActivityList activity={message.activity} streaming={streaming} />}
        {message.body.trim() && <MarkdownBody body={message.body} onOpenReference={onOpenReference} />}
        {message.permission && <PermissionView request={message.permission} onAnswer={onAnswerPermission} />}
    </div>;
}

export function ReplyBox(props: { onSubmit(body: string): Promise<unknown> }): React.ReactElement {
    const [body, setBody] = React.useState('');
    const [busy, setBusy] = React.useState(false);
    const [active, setActive] = React.useState(false);
    const submit = async () => {
        if (!body.trim() || busy) {
            return;
        }
        setBusy(true);
        try {
            await props.onSubmit(body);
            setBody('');
            setActive(false);
        } finally {
            setBusy(false);
        }
    };
    if (!active && !body) {
        return <input className='theia-input co-review-reply-placeholder' placeholder='Reply…' onFocus={() => setActive(true)} readOnly />;
    }
    return <div className='co-review-composer'>
        <textarea className='theia-input' rows={3} value={body} autoFocus placeholder='Reply… (⌘/Ctrl+Enter to send, Esc to cancel)'
            onChange={e => setBody(e.currentTarget.value)}
            onBlur={() => !body && setActive(false)}
            onKeyDown={e => {
                stopKeys(e);
                if (isSubmitKey(e)) {
                    e.preventDefault();
                    e.stopPropagation();
                    submit();
                } else if (e.key === 'Escape') {
                    setBody('');
                    setActive(false);
                }
            }} />
        {body.trim() && <div className='co-review-composer-actions'>
            <button className='theia-button' disabled={busy} onClick={submit}>Reply</button>
        </div>}
    </div>;
}

/** A small icon action; the label is the tooltip. */
function IconAction(props: { icon: string; title: string; onClick(): void; className?: string }): React.ReactElement {
    return <span className={`${codicon(props.icon)} action-label co-review-icon ${props.className ?? ''}`} title={props.title} role='button'
        onClick={e => { e.stopPropagation(); props.onClick(); }} />;
}

const SEVERITIES: (Severity | undefined)[] = [undefined, 'low', 'medium', 'high'];

function useDraft(manager: ReviewManager, draft: ReviewDraft): void {
    const [, force] = React.useReducer((n: number) => n + 1, 0);
    React.useEffect(() => {
        const listener = manager.onDidChangeDraftBody(d => d.id === draft.id && force());
        return () => listener.dispose();
    }, [manager, draft.id]);
}

export interface DraftEditorProps {
    manager: ReviewManager;
    draft: ReviewDraft;
    label: string;
    /** Whether this instance takes focus when the draft asks for it (inline editors do; the panel only for drafts without a range). */
    acceptFocus: boolean;
    onOpen?(): void;
}

/** Editor for a draft; bound to the draft model, so the panel and the inline widget stay in sync. */
export function DraftEditor(props: DraftEditorProps): React.ReactElement {
    const { manager, draft } = props;
    useDraft(manager, draft);
    const [busy, setBusy] = React.useState(false);
    const textarea = React.useRef<HTMLTextAreaElement>(null);
    React.useEffect(() => {
        if (!props.acceptFocus) {
            return;
        }
        const focus = () => requestAnimationFrame(() => textarea.current?.focus({ preventScroll: true }));
        const listener = manager.onDidRequestFocusDraft(id => id === draft.id && focus());
        return () => listener.dispose();
    }, [manager, draft.id, props.acceptFocus]);

    const asking = draft.defaultIntent === 'question';
    const primary = draft.defaultIntent;
    const secondary: ThreadIntent = asking ? 'comment' : 'question';
    const nextSeverity = SEVERITIES[(SEVERITIES.indexOf(draft.severity) + 1) % SEVERITIES.length];
    // Suggestions are possible where the original text is known: document text and patch lines.
    const original = draft.location.docAnchor?.type === 'text' ? draft.location.docAnchor.exact
        : draft.location.patchAnchor?.source !== undefined && (draft.location.patchAnchor.type === 'code-line' || draft.location.patchAnchor.type === 'code-range') ? draft.location.patchAnchor.source : undefined;
    const suggesting = !!draft.proposal;
    const canSubmit = !!draft.body.trim() || (suggesting && draft.proposal!.after !== draft.proposal!.before);
    const toggleSuggestion = () => manager.updateDraft(draft.id, {
        proposal: suggesting ? undefined : {
            before: original ?? '', after: original ?? '',
            path: draft.location.patchAnchor?.path, startLine: draft.location.patchAnchor?.line ?? draft.location.patchAnchor?.startLine
        }
    });
    const submit = async (intent: ThreadIntent) => {
        if (!canSubmit || busy) {
            return;
        }
        setBusy(true);
        try {
            await manager.submitDraft(draft.id, intent);
        } finally {
            setBusy(false);
        }
    };
    return <div className={`co-review-thread co-review-draft ${asking ? 'asking' : ''}`}>
        <div className='co-review-thread-header'>
            <span className={codicon(asking ? 'hubot' : 'comment')} />
            <span className={props.onOpen ? 'co-review-location' : 'co-review-location static'} onClick={props.onOpen}>{props.label}</span>
            {draft.location.symbol && <span className='co-review-symbol'>{draft.location.symbol}</span>}
            <span className='co-review-spacer' />
            <span className={`co-review-badge severity ${draft.severity ? `severity-${draft.severity}` : 'unset'}`} role='button'
                title='Severity (click to change)' onClick={() => manager.updateDraft(draft.id, { severity: nextSeverity })}>
                {draft.severity ?? 'severity'}
            </span>
            <IconAction icon='close' title='Discard draft' onClick={() => manager.discardDraft(draft.id)} />
        </div>
        <div className='co-review-composer'>
            <textarea ref={textarea} className='theia-input' rows={4} value={draft.body} autoFocus={props.acceptFocus && !draft.body}
                placeholder={suggesting ? 'Why this change? (optional)' : asking ? 'Ask the agent about this code… (⌘/Ctrl+Enter)' : 'Leave a comment… (⌘/Ctrl+Enter)'}
                onChange={e => manager.updateDraft(draft.id, { body: e.currentTarget.value })}
                onKeyDown={e => {
                    stopKeys(e);
                    if (isSubmitKey(e)) {
                        e.preventDefault();
                        e.stopPropagation();
                        submit(primary);
                    } else if (e.key === 'Escape' && !draft.body.trim()) {
                        manager.discardDraft(draft.id);
                    }
                }} />
            {suggesting && <textarea className='theia-input co-review-suggestion' rows={Math.min(10, Math.max(2, draft.proposal!.after.split('\n').length + 1))}
                value={draft.proposal!.after} title='Suggested replacement'
                onChange={e => manager.updateDraft(draft.id, { proposal: { ...draft.proposal!, after: e.currentTarget.value } })}
                onKeyDown={e => {
                    stopKeys(e);
                    if (isSubmitKey(e)) {
                        e.preventDefault();
                        e.stopPropagation();
                        submit('comment');
                    }
                }} />}
            <div className='co-review-composer-actions'>
                {original !== undefined && <span className='co-review-link' onClick={toggleSuggestion}>{suggesting ? 'Remove suggestion' : 'Suggest a change'}</span>}
                <span className='co-review-spacer' />
                {!suggesting && manager.hasAgent && <span className={`co-review-link ${!canSubmit || busy ? 'disabled' : ''}`} onClick={() => submit(secondary)}>
                    {asking ? 'Post as comment' : 'Ask agent instead'}
                </span>}
                <button className='theia-button' disabled={!canSubmit || busy} onClick={() => submit(suggesting ? 'comment' : primary)}>
                    {suggesting ? 'Suggest edit' : asking ? 'Ask Agent' : 'Comment'}
                </button>
            </div>
        </div>
    </div>;
}

export interface ThreadViewProps {
    manager: ReviewManager;
    thread: ReviewThread;
    label: string;
    anchorState?: string;
    selected?: boolean;
    /** Inline in an editor: always expanded, with a hide button instead of the expand toggle. */
    inline?: boolean;
    onSelect?(): void;
    onOpen?(): void;
    onHide?(): void;
    onOpenReference?: OpenReference;
}

export function ThreadView(props: ThreadViewProps): React.ReactElement {
    const { thread, manager } = props;
    const [expandedState, setExpanded] = React.useState(thread.status === 'open');
    React.useEffect(() => {
        if (props.selected) {
            setExpanded(true);
        }
    }, [props.selected]);
    const expanded = props.inline || expandedState;
    const resolved = thread.status === 'resolved';
    const proposed = thread.status === 'proposed';
    const agent = manager.activeReview?.agent;
    const agentBusy = thread.agentState === 'working' || thread.agentState === 'waiting_for_human';
    // ACP agents post as `agent:<id>`; MCP agents' ids already are participant ids.
    const agentEngaged = !!agent && thread.messages.some(m => m.author.id === agent.id || m.author.id === `agent:${agent.id}`);
    const canAsk = !!agent && !resolved && !agentBusy && !agentEngaged;
    return <div id={props.inline ? undefined : `co-review-thread-${thread.id}`}
        className={`co-review-thread ${resolved ? 'resolved' : ''} ${props.selected ? 'selected' : ''} ${props.inline ? 'inline' : ''}`}
        onClick={props.onSelect}>
        <div className='co-review-thread-header'>
            {!props.inline && <span className={`${codicon(expanded ? 'chevron-down' : 'chevron-right')} co-review-toggle`}
                onClick={e => { e.stopPropagation(); setExpanded(!expanded); }} />}
            <span className='co-review-number'>#{thread.number}</span>
            {props.inline && thread.location.kind === 'document' && <span className='co-review-symbol' title={props.label}>{props.label}</span>}
            {!props.inline && <span className='co-review-location' title='Open location'
                onClick={e => { if (props.onOpen) { e.stopPropagation(); props.onOpen(); } }}>{props.label}</span>}
            {thread.location.symbol && <span className='co-review-symbol' title={thread.location.symbol}>{thread.location.symbol}</span>}
            {thread.intent === 'question' && <span className={`${codicon('hubot')} co-review-mark`} title='Question for the agent' />}
            {thread.severity && <span className={`co-review-badge severity-${thread.severity}`}>{thread.severity}</span>}
            {thread.labels?.map(label => <span key={label} className='co-review-badge label'>{label}</span>)}
            {props.anchorState && props.anchorState !== 'exact' &&
                <span className={`co-review-badge ${props.anchorState}`}
                    title={props.anchorState === 'moved' ? 'The commented code moved; the comment followed it.' : 'The commented code no longer exists.'}>
                    {props.anchorState}
                </span>}
            {thread.agentState === 'working' && <span className='co-review-badge working'>working…</span>}
            {thread.agentState === 'waiting_for_human' && <span className='co-review-badge waiting'>needs your decision</span>}
            {resolved && <span className='co-review-badge resolved'>resolved</span>}
            {proposed && <span className='co-review-badge proposed' title='Automated finding: accept to keep it, dismiss to drop it'>proposed finding</span>}
            <span className='co-review-spacer' />
            <span className={`co-review-actions ${proposed ? 'always' : ''}`}>
                {canAsk && <IconAction icon='hubot' title={`Ask ${agent!.name}`} onClick={() => manager.askAgent(thread)} />}
                {proposed ? <>
                    <IconAction icon='pass' title='Accept finding' onClick={() => manager.setStatus(thread, 'open')} />
                    <IconAction icon='circle-slash' title='Dismiss finding' onClick={() => manager.setStatus(thread, 'resolved')} />
                </> : <IconAction icon={resolved ? 'issue-reopened' : 'check'} title={resolved ? 'Reopen' : 'Resolve'}
                    onClick={() => manager.setStatus(thread, resolved ? 'open' : 'resolved')} />}
                {props.inline && props.onHide && <IconAction icon='chevron-up' title='Collapse' onClick={props.onHide} />}
            </span>
        </div>
        {!expanded && <div className='co-review-preview'>{thread.messages[0]?.body} {thread.messages.length > 1 && <em>(+{thread.messages.length - 1})</em>}</div>}
        {expanded && <>
            {!props.inline && thread.location.anchor?.text && thread.location.kind !== 'symbol' &&
                <pre className='co-review-snippet'>{snippet(thread.location.anchor.text)}</pre>}
            {thread.proposal && <ProposalView proposal={thread.proposal} onDecide={accept => manager.decideProposal(thread, accept)} />}
            {thread.messages.map(m => <MessageView key={m.id} message={m} onOpenReference={props.onOpenReference}
                onAnswerPermission={(requestId, optionId) => manager.answerPermission(requestId, optionId)} />)}
            {agentBusy && <span className='co-review-link' onClick={e => { e.stopPropagation(); manager.cancelAgent(thread); }}>Stop the agent</span>}
            {!resolved && <ReplyBox onSubmit={body => manager.reply(thread, body)} />}
        </>}
    </div>;
}

function snippet(text: string): string {
    const lines = text.split('\n');
    return lines.length > 8 ? [...lines.slice(0, 8), `… ${lines.length - 8} more lines`].join('\n') : text;
}

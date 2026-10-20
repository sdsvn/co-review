import * as React from '@theia/core/shared/react';
import { codicon } from '@theia/core/lib/browser/widgets/widget';
import { isOutOfScope, Review, ReviewCoverage, Severity } from '../common/review-model';
import { ReviewManager } from './review-manager';
import { ReviewNavigator } from './review-navigator';
import { agentStatus, firstFinding } from './review-insights';

export interface BriefingProps {
    manager: ReviewManager;
    navigator: ReviewNavigator;
    review: Review;
    coverage?: ReviewCoverage;
}

const SEVERITY_COLOR: Record<Severity | 'none', string> = { high: 'high', medium: 'medium', low: 'low', none: 'none' };

/**
 * The top of a review's overview page: what the review is and how big, the agent's state, what is left (findings to
 * triage by severity, open threads, files viewed) and what to do next. Drawn by the app around the page's text, so it
 * takes no comments and is always current.
 */
export function Briefing({ manager, navigator, review, coverage }: BriefingProps): React.ReactElement {
    const inScope = review.threads.filter(t => !isOutOfScope(t));
    const proposed = inScope.filter(t => t.status === 'proposed');
    const open = inScope.filter(t => t.status === 'open');
    const status = agentStatus(review, manager.getAgentPresence(review.id));
    const progress = navigator.changeProgress();
    const files = progress ? { viewed: progress.viewed, total: progress.files.length } : coverage?.total ? { viewed: coverage.viewed, total: coverage.total } : undefined;
    const size = progress && review.bundle ? changeSize(navigator) : undefined;
    const kind = review.bundle ? 'Change review' : 'Repository review';
    const first = firstFinding(review);
    const bySeverity = (['high', 'medium', 'low', 'none'] as const).map(s => ({ s, n: proposed.filter(t => (t.severity ?? 'none') === s).length })).filter(x => x.n);
    const start = async () => {
        if (first) {
            manager.setCollapsed(first.id, false);
            manager.revealThread(first.id);
            await navigator.open(first.location, first.id);
        }
    };
    return <div className='co-review-briefing-inner'>
        <div className='co-review-briefing-kind'>
            <span className={codicon(review.bundle ? 'git-pull-request' : 'repo')} />
            <span>{kind}{size ? ` · ${size}` : files && !review.bundle ? ` · ${files.total} files` : ''}</span>
            <span className='co-review-spacer' />
            {review.agent && <span className={`co-review-agent-pill ${status.state}`} title={status.title}>
                <span className='co-review-agent-dot' />{review.agent.name} {status.state === 'away' ? 'is away' : `is ${status.label}`}
            </span>}
        </div>
        <div className='co-review-tiles'>
            <div className={`co-review-tile ${proposed.length ? 'attention' : ''}`}>
                <b>{proposed.length}</b>
                <span>{proposed.length === 1 ? 'finding to triage' : 'findings to triage'}</span>
                {proposed.length > 0 && <span className='co-review-severity-bar' title={bySeverity.map(x => `${x.n} ${x.s === 'none' ? 'without severity' : x.s}`).join(', ')}>
                    {bySeverity.map(x => <span key={x.s} className={`sev-${SEVERITY_COLOR[x.s]}`} style={{ flex: x.n }} />)}
                </span>}
                {!review.threads.length && review.agent && <span className='co-review-tile-note'>{review.agent.name} is on its first pass</span>}
            </div>
            <div className='co-review-tile'>
                <b>{open.length}</b>
                <span>{open.length === 1 ? 'open thread' : 'open threads'}</span>
            </div>
            {files && <div className='co-review-tile'>
                <b>{files.viewed} / {files.total}</b>
                <span>files viewed</span>
                <span className='co-review-severity-bar progress'><span style={{ flex: files.viewed }} /><span className='rest' style={{ flex: Math.max(files.total - files.viewed, 0) }} /></span>
            </div>}
        </div>
        <div className='co-review-briefing-actions'>
            {first && <button className='theia-button' onClick={start}
                title={`Open #${first.number}${first.severity ? ` (${first.severity})` : ''} and its thread`}>
                {first.status === 'proposed' ? 'Start with the first finding' : 'Go to the first open thread'}
            </button>}
            {progress && progress.files.length > 0 && <button className='theia-button secondary' onClick={() => navigator.openNextUnviewed()}>
                {progress.viewed ? 'Next unviewed file' : 'Open the diff'}
            </button>}
            {!progress && files && files.viewed < files.total && <button className='theia-button secondary' onClick={() => navigator.openNextUnviewed()}>
                Next file to read
            </button>}
            {review.purpose === 'explain'
                ? <button className='theia-button secondary' title='Close this window: the agent knows you are done' onClick={() => manager.finish()}>Done</button>
                : (review.bundle || review.agent) && <button className='theia-button secondary' onClick={() => manager.requestSubmit()}>Submit review…</button>}
        </div>
    </div>;
}

/** "39 files · +357 −151", from the change's files. */
function changeSize(navigator: ReviewNavigator): string | undefined {
    const files = navigator.changeFiles();
    if (!files.length) {
        return undefined;
    }
    const added = files.reduce((n, f) => n + f.added, 0);
    const removed = files.reduce((n, f) => n + f.removed, 0);
    return `${files.length} file${files.length === 1 ? '' : 's'} · +${added} −${removed}`;
}

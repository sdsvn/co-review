import { ConfirmDialog } from '@theia/core/lib/browser/dialogs';
import { GitHubEdits, GitHubPreview, GitHubTarget } from '../common/review-protocol';

const EVENT_LABEL: Record<GitHubPreview['event'], string> = { APPROVE: 'Approve', REQUEST_CHANGES: 'Request changes', COMMENT: 'Comment' };

/**
 * Shows the reviewer exactly what will be posted to GitHub (the review's decision and body, then each line comment
 * where it lands), each part editable in place, and resolves to what they confirmed (undefined: cancelled). Their edits
 * are what is posted; a line comment they empty is left out.
 */
export async function confirmGitHubPost(github: GitHubTarget, preview: GitHubPreview): Promise<GitHubEdits | undefined> {
    const root = document.createElement('div');
    root.className = 'co-review-github-preview';
    const add = <K extends keyof HTMLElementTagNameMap>(parent: HTMLElement, tag: K, text: string, className?: string) => {
        const el = document.createElement(tag);
        el.textContent = text;
        if (className) {
            el.className = className;
        }
        parent.appendChild(el);
        return el;
    };
    add(root, 'div', `To ${github.repo}#${github.number} as "${EVENT_LABEL[preview.event]}". Edit any of it before it goes; empty a line comment to leave it out.`
        + (github.postedRound !== undefined ? ` Round ${github.postedRound} was already posted.` : ''), 'co-review-muted');
    const section = (title: string, body: string, placeholder: string) => {
        const part = add(root, 'div', '', 'co-review-github-preview-part');
        add(part, 'div', title, 'co-review-github-preview-where');
        const box = add(part, 'textarea', '', 'theia-input co-review-github-preview-body');
        box.value = body;
        box.placeholder = placeholder;
        box.rows = Math.min(12, Math.max(2, body.split('\n').length + 1));
        // Typing in the dialog must not reach the dialog's Enter-to-accept.
        box.addEventListener('keydown', e => e.stopPropagation());
        return box;
    };
    const body = section('Review', preview.body, 'No message');
    const comments = preview.comments.map(c => ({
        threadId: c.threadId,
        box: section(`${c.path}:${c.startLine ? `${c.startLine}-` : ''}${c.line}${c.side === 'LEFT' ? ' (old side)' : ''}`, c.body, 'Left out')
    }));
    if (!comments.length) {
        add(root, 'div', 'No line comments.', 'co-review-muted');
    }
    add(root, 'div', 'Agents\' replies, questions you asked an agent, resolved threads and findings you didn\'t accept stay in Co-Review.',
        'co-review-muted');
    const ok = await new ConfirmDialog({ title: 'Post to GitHub', msg: root, ok: 'Post to GitHub' }).open();
    return ok ? { body: body.value, comments: Object.fromEntries(comments.map(c => [c.threadId, c.box.value])) } : undefined;
}

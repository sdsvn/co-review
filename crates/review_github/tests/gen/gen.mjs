#!/usr/bin/env node
/**
 * Generate test fixtures for review_github by driving the compiled TypeScript GitHubReviews class.
 *
 * Each fixture produces:
 *   <name>.review.json   – the synthetic Review
 *   <name>.preview.json  – the GitHubPreview the TS compose+preview produces
 *   <name>.preview.md    – the preview as Markdown
 *
 * For publish-retry fixtures, also:
 *   <name>.payloads.json – the JSON payloads that would be sent to `gh api` (in order)
 *
 * Run from the repo root:
 *   node crates/review_github/tests/gen/gen.mjs
 */

import { createRequire } from 'module';
import { writeFileSync, mkdirSync } from 'fs';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const fixturesDir = resolve(__dirname, '..', 'fixtures');
mkdirSync(fixturesDir, { recursive: true });

// Load the compiled TS. It uses CommonJS, so use createRequire.
const require = createRequire(resolve(__dirname, '..', '..', '..', '..', 'extensions', 'review', 'lib', 'node', 'github.js'));
const { GitHubReviews, previewMarkdown } = require('../../../../extensions/review/lib/node/github.js');

// ---------------------------------------------------------------------------
// Helpers to build synthetic reviews
// ---------------------------------------------------------------------------

let threadCounter = 0;

function makeParticipant(kind, name, id) {
    return { id: id || name, kind, name };
}

const reviewer = makeParticipant('human', 'Reviewer', 'reviewer-1');
const agent = makeParticipant('agent', 'Agent', 'agent-1');
const system = makeParticipant('system', 'System', 'system-1');

function makeMessage(author, body) {
    return {
        id: `msg-${++threadCounter}`,
        author,
        body,
        createdAt: 1700000000000 + threadCounter,
    };
}

function makeThread(overrides) {
    const num = ++threadCounter;
    return {
        id: overrides.id || `thread-${num}`,
        number: overrides.number ?? num,
        location: overrides.location || { kind: 'patch' },
        messages: overrides.messages || [],
        status: overrides.status || 'open',
        intent: overrides.intent || 'comment',
        origin: overrides.origin,
        proposal: overrides.proposal,
        createdAt: 1700000000000,
        updatedAt: 1700000000000,
    };
}

function makeReview(threads, verdict) {
    return {
        id: 'review-1',
        title: 'Test Review',
        workspaceRoot: '/workspace',
        scope: { kind: 'repository' },
        participants: [reviewer, agent],
        threads,
        activity: [],
        nextThreadNumber: threads.length + 1,
        createdAt: 1700000000000,
        updatedAt: 1700000000000,
        verdict: verdict || undefined,
    };
}

function patchLocation(type, path, opts = {}) {
    return {
        kind: 'patch',
        patchAnchor: {
            type,
            path,
            ...opts,
        },
    };
}

// ---------------------------------------------------------------------------
// Fixture definitions
// ---------------------------------------------------------------------------

const fixtures = [];

// 1. Basic line comment
{
    const thread = makeThread({
        id: 'basic-1',
        number: 1,
        location: patchLocation('code-line', 'src/main.rs', { line: 42, side: 'new' }),
        messages: [makeMessage(reviewer, 'This should use a constant.')],
    });
    const review = makeReview([thread], { decision: 'request-changes', summary: 'A few issues found.', submittedAt: 1700000000000, count: 1 });
    fixtures.push({ name: 'basic_line', review, decision: 'request-changes', summary: 'A few issues found.' });
}

// 2. Code range (multi-line and single-line collapse)
{
    const multiLine = makeThread({
        id: 'range-multi',
        number: 1,
        location: patchLocation('code-range', 'src/lib.rs', { startLine: 10, endLine: 20, side: 'new' }),
        messages: [makeMessage(reviewer, 'This block needs refactoring.')],
    });
    const singleLine = makeThread({
        id: 'range-single',
        number: 2,
        location: patchLocation('code-range', 'src/lib.rs', { startLine: 30, endLine: 30, side: 'new' }),
        messages: [makeMessage(reviewer, 'Simplify this line.')],
    });
    const review = makeReview([multiLine, singleLine], { decision: 'comment', summary: 'Range comments.', submittedAt: 1700000000000, count: 1 });
    fixtures.push({ name: 'code_range', review, decision: 'comment', summary: 'Range comments.' });
}

// 3. Reworded thread
{
    const thread = makeThread({
        id: 'reworded-1',
        number: 1,
        location: patchLocation('code-line', 'src/handler.rs', { line: 15, side: 'new' }),
        messages: [makeMessage(agent, 'The error handling here is incorrect.'), makeMessage(reviewer, 'Yeah, fix this.')],
    });
    const review = makeReview([thread], { decision: 'request-changes', summary: 'Needs error handling fix.', submittedAt: 1700000000000, count: 1 });
    const reworded = { 'reworded-1': 'Please improve the error handling in this function.' };
    fixtures.push({ name: 'reworded', review, decision: 'request-changes', summary: 'Needs error handling fix.', reworded });
}

// 4. Suggestion (accepted proposal) and rejected proposal
{
    const accepted = makeThread({
        id: 'suggest-accepted',
        number: 1,
        location: patchLocation('code-line', 'src/config.rs', { line: 8, side: 'new' }),
        messages: [makeMessage(agent, 'Use a constant here.')],
        origin: 'finding',
        proposal: { before: 'let x = 42;', after: 'const X: i32 = 42;\n', status: 'accepted' },
    });
    const rejected = makeThread({
        id: 'suggest-rejected',
        number: 2,
        location: patchLocation('code-line', 'src/config.rs', { line: 12, side: 'new' }),
        messages: [makeMessage(agent, 'Consider using an enum.')],
        origin: 'finding',
        status: 'resolved',
        proposal: { before: 'let mode = "fast";', after: 'let mode = Mode::Fast;\n', status: 'rejected' },
    });
    // Accepted suggestion on old side: no suggestion block
    const oldSide = makeThread({
        id: 'suggest-old',
        number: 3,
        location: patchLocation('code-line', 'src/config.rs', { line: 5, side: 'old' }),
        messages: [makeMessage(reviewer, 'This was wrong before.')],
        proposal: { before: 'old code', after: 'new code\n', status: 'accepted' },
    });
    const review = makeReview([accepted, rejected, oldSide], { decision: 'comment', summary: '', submittedAt: 1700000000000, count: 1 });
    fixtures.push({ name: 'suggestion', review, decision: 'comment', summary: '' });
}

// 5. Elsewhere: threads not on diff lines go into the body
{
    const docThread = makeThread({
        id: 'elsewhere-doc',
        number: 1,
        location: { kind: 'document', docAnchor: { type: 'document' } },
        messages: [makeMessage(reviewer, 'The description is unclear.')],
    });
    const patchLevel = makeThread({
        id: 'elsewhere-patch',
        number: 2,
        location: { kind: 'patch', patchAnchor: { type: 'patch' } },
        messages: [makeMessage(reviewer, 'Overall the patch is too large.')],
    });
    const review = makeReview([docThread, patchLevel], { decision: 'comment', summary: 'Some general notes.', submittedAt: 1700000000000, count: 1 });
    fixtures.push({ name: 'elsewhere', review, decision: 'comment', summary: 'Some general notes.' });
}

// 6. Filtered out: question, resolved, agent-only
{
    const questionThread = makeThread({
        id: 'filtered-question',
        number: 1,
        location: patchLocation('code-line', 'src/main.rs', { line: 5, side: 'new' }),
        messages: [makeMessage(reviewer, 'What does this do?')],
        intent: 'question',
    });
    const resolvedThread = makeThread({
        id: 'filtered-resolved',
        number: 2,
        location: patchLocation('code-line', 'src/main.rs', { line: 10, side: 'new' }),
        messages: [makeMessage(reviewer, 'Fixed.')],
        status: 'resolved',
    });
    const agentOnly = makeThread({
        id: 'filtered-agent',
        number: 3,
        location: patchLocation('code-line', 'src/main.rs', { line: 15, side: 'new' }),
        messages: [makeMessage(agent, 'I found something.')],
        origin: 'finding',
        status: 'proposed',
    });
    // One thread that SHOULD be posted (control)
    const validThread = makeThread({
        id: 'filtered-valid',
        number: 4,
        location: patchLocation('code-line', 'src/main.rs', { line: 20, side: 'new' }),
        messages: [makeMessage(reviewer, 'Good point, keeping this.')],
    });
    const review = makeReview([questionThread, resolvedThread, agentOnly, validThread],
        { decision: 'comment', summary: 'Filtering test.', submittedAt: 1700000000000, count: 1 });
    fixtures.push({ name: 'filtered_out', review, decision: 'comment', summary: 'Filtering test.' });
}

// 7. Approve with no summary or threads: default body "Looks good."
{
    const review = makeReview([], { decision: 'approve', summary: '', submittedAt: 1700000000000, count: 1 });
    fixtures.push({ name: 'approve_default', review, decision: 'approve', summary: '' });
}

// 8. Own PR fallback (for publish test)
{
    const thread = makeThread({
        id: 'own-pr-1',
        number: 1,
        location: patchLocation('code-line', 'src/main.rs', { line: 5, side: 'new' }),
        messages: [makeMessage(reviewer, 'I noticed this issue.')],
    });
    const review = makeReview([thread], { decision: 'approve', summary: 'Looks good overall.', submittedAt: 1700000000000, count: 1 });
    fixtures.push({ name: 'own_pr', review, decision: 'approve', summary: 'Looks good overall.', publishTest: 'own_pr' });
}

// 9. Diff-line fallback (for publish test)
{
    const thread = makeThread({
        id: 'diff-fb-1',
        number: 1,
        location: patchLocation('code-line', 'src/old.rs', { line: 100, side: 'new' }),
        messages: [makeMessage(reviewer, 'This line has a bug.')],
    });
    const review = makeReview([thread], { decision: 'comment', summary: 'Found a bug.', submittedAt: 1700000000000, count: 1 });
    fixtures.push({ name: 'diff_fallback', review, decision: 'comment', summary: 'Found a bug.', publishTest: 'diff_fallback' });
}

// 10. Edits: reviewer changes summary, edits a comment body, empties another
{
    const threadKept = makeThread({
        id: 'edits-kept',
        number: 1,
        location: patchLocation('code-line', 'src/api.rs', { line: 10, side: 'new' }),
        messages: [makeMessage(reviewer, 'Original comment kept.')],
    });
    const threadEdited = makeThread({
        id: 'edits-changed',
        number: 2,
        location: patchLocation('code-line', 'src/api.rs', { line: 25, side: 'new' }),
        messages: [makeMessage(reviewer, 'Original comment to edit.')],
    });
    const threadEmptied = makeThread({
        id: 'edits-emptied',
        number: 3,
        location: patchLocation('code-line', 'src/api.rs', { line: 40, side: 'new' }),
        messages: [makeMessage(reviewer, 'This comment will be emptied.')],
    });
    const review = makeReview([threadKept, threadEdited, threadEmptied],
        { decision: 'request-changes', summary: 'Original summary.', submittedAt: 1700000000000, count: 1 });
    const edits = {
        body: 'Edited summary by reviewer.',
        comments: {
            'edits-changed': 'Reviewer rewrote this comment.',
            'edits-emptied': '',   // emptied: should be left out
        },
    };
    fixtures.push({ name: 'edits', review, decision: 'request-changes', summary: 'Original summary.', publishTest: 'edits', edits });
}

// ---------------------------------------------------------------------------
// Drive the TS class
// ---------------------------------------------------------------------------

// Create an instance, bypassing DI.
const instance = new GitHubReviews();

for (const fixture of fixtures) {
    const { name, review, decision, summary, reworded, publishTest, edits } = fixture;

    // Write the review
    writeFileSync(resolve(fixturesDir, `${name}.review.json`), JSON.stringify(review, null, 2) + '\n');

    // Compute preview using compose
    const rewordedMap = reworded ? new Map(Object.entries(reworded)) : new Map();
    const composed = instance.compose(review, decision, summary, rewordedMap);
    const previewResult = {
        event: { approve: 'APPROVE', 'request-changes': 'REQUEST_CHANGES', comment: 'COMMENT' }[decision],
        body: composed.body,
        comments: composed.comments.map(c => ({
            threadId: c.threadId,
            path: c.path,
            line: c.line,
            startLine: c.start_line,
            side: c.side,
            body: c.body,
        })),
    };

    writeFileSync(resolve(fixturesDir, `${name}.preview.json`), JSON.stringify(previewResult, null, 2) + '\n');

    // Write reworded map if any
    if (reworded) {
        writeFileSync(resolve(fixturesDir, `${name}.reworded.json`), JSON.stringify(reworded, null, 2) + '\n');
    }

    // Preview markdown
    const md = previewMarkdown(previewResult);
    writeFileSync(resolve(fixturesDir, `${name}.preview.md`), md + '\n');

    // Publish-test payloads: simulate the retry by calling compose and building the payloads ourselves
    // (we can't call publish because it spawns gh, but the payload logic is deterministic from compose).
    if (publishTest === 'own_pr') {
        // First attempt: APPROVE event
        const firstPayload = {
            event: 'APPROVE',
            body: composed.body,
            comments: composed.comments.map(({ threadId, ...c }) => c),
        };
        // Second attempt: COMMENT event with prepended label
        const label = { approve: 'Approve', 'request-changes': 'Request changes', comment: 'Comment' }[decision];
        const secondPayload = {
            event: 'COMMENT',
            body: `**${label}.** ${composed.body}`.trim(),
            comments: composed.comments.map(({ threadId, ...c }) => c),
        };
        writeFileSync(resolve(fixturesDir, `${name}.payloads.json`), JSON.stringify([firstPayload, secondPayload], null, 2) + '\n');
    }

    if (publishTest === 'diff_fallback') {
        // First attempt: with line comments
        const firstPayload = {
            event: previewResult.event,
            body: composed.body,
            comments: composed.comments.map(({ threadId, ...c }) => c),
        };
        // Second attempt: comments moved to body
        const commentLines = composed.comments.map(c => {
            const prefix = c.start_line ? `${c.start_line}-` : '';
            return `- **${c.path}:${prefix}${c.line}:** ${c.body.replace(/\n/g, '\n  ')}`;
        });
        const parts = [composed.body, ...commentLines].filter(Boolean);
        const secondPayload = {
            event: previewResult.event,
            body: parts.join('\n'),
            comments: [],
        };
        writeFileSync(resolve(fixturesDir, `${name}.payloads.json`), JSON.stringify([firstPayload, secondPayload], null, 2) + '\n');
    }

    if (publishTest === 'edits') {
        // The edits fixture: apply edits to the composed result, same as publish() does.
        // Body comes from edits.body; comments use edits.comments[threadId] when present.
        const editedComments = composed.comments
            .map(c => ({ ...c, body: (edits.comments[c.threadId] ?? c.body).trim() }))
            .filter(c => c.body);
        const payload = {
            event: previewResult.event,
            body: edits.body.trim(),
            comments: editedComments.map(({ threadId, ...c }) => c),
        };
        writeFileSync(resolve(fixturesDir, `${name}.edits.json`), JSON.stringify(edits, null, 2) + '\n');
        writeFileSync(resolve(fixturesDir, `${name}.payloads.json`), JSON.stringify([payload], null, 2) + '\n');
    }

    console.log(`Generated: ${name}`);
}

console.log(`\nAll ${fixtures.length} fixtures generated in ${fixturesDir}`);

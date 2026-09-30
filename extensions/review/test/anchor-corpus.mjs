// Tunes and checks anchor tracking on real edits: replays commits of a git repository, carries comment targets from
// each changed file's old version to its new one, and scores the result against git's own line mapping (what
// GitHub uses to move review comments). Not a unit test: `node extensions/review/test/anchor-corpus.mjs [repo] [commits]`.
//
// Last run (TRACKING.modified 0.7, margin 0.1): co-review, 120 commits, 14,638 targets: 14,054 of 14,056 untouched
// lines active (2 lost, 0 wrong); 582 changed lines: 264 kept, 301 removed, 17 "wrong" (on inspection all the same
// sentence rewrapped or moved, where git's hunks differ). careconnect (Go), 150 commits, 101,836 targets: 90,718 of
// 90,722 untouched lines active (3 lost, 1 wrong); 580 changed: 317 kept, 256 removed, 7 wrong.
//
// For a target line outside every changed hunk, git says where it went: tracking must say `active` there.
// For a line inside a changed hunk, `modified` must land inside that hunk's new lines, `active` only on identical text,
// and `removed` or `ambiguous` is fine. Anything else is a wrong placement: the one outcome to avoid.
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const lib = process.env.CO_REVIEW_LIB ?? new URL('../lib', import.meta.url).pathname;
const tracking = require(`${lib}/common/anchor-tracking.js`);
const { editsBetween, rangeIn, track, TRACKING } = tracking;

const repo = process.argv[2] ?? process.cwd();
const commits = Number(process.argv[3] ?? 200);
const git = (...args) => execFileSync('git', ['-C', repo, ...args], { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 });

/** Old line (1-based) → new line, or undefined when git counts it as changed; plus the hunks, from `git diff -U0`. */
function lineMap(parent, commit, file) {
    const hunks = [];
    for (const m of git('diff', '-U0', '--no-color', parent, commit, '--', file).matchAll(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/gm)) {
        hunks.push({ oldStart: +m[1], oldCount: m[2] === undefined ? 1 : +m[2], newStart: +m[3], newCount: m[4] === undefined ? 1 : +m[4] });
    }
    return {
        hunks,
        map(line) {
            let shift = 0;
            for (const h of hunks) {
                const oldEnd = h.oldStart + h.oldCount - 1;
                if (h.oldCount && line >= h.oldStart && line <= oldEnd) {
                    return { changed: h };
                }
                if ((h.oldCount ? oldEnd : h.oldStart) < line) {
                    shift += h.newCount - h.oldCount;
                }
            }
            return { line: line + shift };
        }
    };
}

const lineOf = (text, offset) => text.slice(0, offset).split('\n').length;

/** The cases: every 4th non-trivial line of each changed source file, over the last `commits` commits. */
function cases() {
    const out = [];
    const log = git('log', `-${commits}`, '--no-merges', '--format=%H %P').trim().split('\n').map(l => l.split(' ')).filter(p => p.length === 2);
    for (const [commit, parent] of log) {
        const files = git('diff', '--name-only', '--diff-filter=M', parent, commit).split('\n').filter(f => /\.(ts|tsx|js|mjs|go|py|md)$/.test(f));
        for (const file of files) {
            let before;
            let after;
            try {
                before = git('show', `${parent}:${file}`);
                after = git('show', `${commit}:${file}`);
            } catch {
                continue;
            }
            if (before.length > 400_000) {
                continue;
            }
            const map = lineMap(parent, commit, file);
            // One diff per file version pair, for all its targets (as the backend does).
            const edits = editsBetween(before, after);
            const lines = before.split('\n');
            let offset = 0;
            lines.forEach((text, i) => {
                const from = offset + text.length - text.trimStart().length;
                offset += text.length + 1;
                if (i % 4 === 0 && text.trim().length >= 12) {
                    out.push({ file, before, after, edits, from, to: from + text.trim().length, line: i + 1, map });
                }
            });
        }
    }
    return out;
}

function score(all) {
    const counts = { unchangedActive: 0, unchangedLost: 0, unchangedWrong: 0, changedModified: 0, changedGone: 0, changedWrong: 0, unchanged: 0, changed: 0 };
    for (const c of all) {
        const t = rangeIn(c.before, 'v1', c.from, c.to);
        const r = track(t, c.before, c.after, 'v2', c.edits);
        const truth = c.map.map(c.line);
        // git's mapping, where it holds: a line it keeps reads the same (otherwise the case says nothing).
        if (truth.line !== undefined && c.after.split('\n')[truth.line - 1]?.trim() !== t.text) {
            continue;
        }
        const placed = r.status === 'active' || r.status === 'modified';
        const at = placed ? lineOf(c.after, r.from) : undefined;
        if (truth.line !== undefined) {
            counts.unchanged++;
            if (!placed) {
                counts.unchangedLost++;
            } else if (at === truth.line || (r.status === 'active' && c.after.slice(r.from, r.to) === t.text)) {
                counts.unchangedActive++;
            } else {
                counts.unchangedWrong++;
            }
        } else {
            counts.changed++;
            const h = truth.changed;
            const inHunk = at !== undefined && at >= h.newStart && at < h.newStart + Math.max(1, h.newCount);
            if (!placed) {
                counts.changedGone++;
            } else if (inHunk || (r.status === 'active' && c.after.slice(r.from, r.to) === t.text)) {
                counts.changedModified++;
            } else {
                counts.changedWrong++;
            }
        }
    }
    return counts;
}

const all = cases();
console.log(`${all.length} targets from ${repo} (last ${commits} commits)`);
const results = [];
for (const modified of [0.4, 0.5, 0.6, 0.7, 0.8]) {
    for (const margin of [0.05, 0.1, 0.2]) {
        TRACKING.modified = modified;
        TRACKING.ambiguousMargin = margin;
        const c = score(all);
        results.push({ modified, margin, ...c });
    }
}
results.sort((a, b) => (a.unchangedWrong + a.changedWrong) - (b.unchangedWrong + b.changedWrong) || a.unchangedLost - b.unchangedLost);
console.log('modified margin | unchanged: active lost WRONG | changed: kept gone WRONG');
for (const r of results) {
    console.log(`${r.modified.toFixed(1)}      ${r.margin.toFixed(2)}   | ${r.unchanged}: ${r.unchangedActive} ${r.unchangedLost} ${r.unchangedWrong} | ${r.changed}: ${r.changedModified} ${r.changedGone} ${r.changedWrong}`);
}

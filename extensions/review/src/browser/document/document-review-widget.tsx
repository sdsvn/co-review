import * as React from '@theia/core/shared/react';
import { createRoot, Root } from '@theia/core/shared/react-dom/client';
import { BaseWidget, Message, Navigatable, codicon } from '@theia/core/lib/browser';
import { Disposable, DisposableCollection } from '@theia/core/lib/common/disposable';
import URI from '@theia/core/lib/common/uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { FileService } from '@theia/filesystem/lib/browser/file-service';
import { OpenerService, open } from '@theia/core/lib/browser/opener-service';
import { DocAnchor, isOutOfScope, isOverviewPage, ReviewThread, ThreadIntent } from '../../common/review-model';
import { DraftEditor, ThreadView } from '../review-components';
import { ReviewDraft, ReviewManager } from '../review-manager';
import { ReviewNavigator } from '../review-navigator';
import { MermaidBlock, render } from './document-render';
import { splitFrontmatter } from '../../common/design-format';
import { markText, textAnchor, unmark } from './text-anchor';
import { openDiagramZoom } from './diagram-zoom';
import { Briefing } from '../review-briefing';
import { ReviewCoverage } from '../../common/review-model';

const escape = (text: string) => text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;');

export const DocumentReviewWidgetOptions = Symbol('DocumentReviewWidgetOptions');
export interface DocumentReviewWidgetOptions {
    uri: string;
}

/** Not part of the document text: inline threads, rendered diagrams and the overview's briefing. */
const SKIP = '.co-review-slot, .co-review-dgm svg, .co-review-briefing, .co-review-file-bar';

type Anchored = { key: string; anchor: DocAnchor; element: (root: Root) => void };

/** Which rendered element a document anchor belongs to (for highlighting and thread placement). */
function locate(content: HTMLElement, anchor: DocAnchor): { target?: Element; marks?: HTMLElement[] } {
    switch (anchor.type) {
        case 'document':
            return { target: undefined };
        case 'mermaid-block':
        case 'mermaid-edge':
        case 'mermaid-node': {
            const host = content.querySelector(`.co-review-dgm[data-block-id="${CSS.escape(anchor.blockId ?? '')}"]`);
            if (host && anchor.type === 'mermaid-node') {
                host.querySelector(`[data-node-id="${CSS.escape(anchor.nodeId ?? '')}"]`)?.classList.add('co-review-dgm-marked');
            }
            return { target: host ?? undefined };
        }
        case 'tree-node': {
            const step = content.querySelector(`.co-review-step[data-node-id="${CSS.escape(anchor.nodeId ?? '')}"]`);
            step?.classList.add('co-review-step-marked');
            return { target: step?.querySelector(':scope > .co-review-step-line, :scope > details > summary') ?? undefined };
        }
        case 'text': {
            const marks = markText(content, anchor, SKIP);
            return { target: marks[marks.length - 1]?.closest('p, li, h1, h2, h3, h4, h5, h6, pre, blockquote, table, .co-review-step-line') ?? undefined, marks };
        }
    }
}

/**
 * Review view of a Markdown document: rendered Markdown / pseudocode tree / Mermaid, with
 * comments on text, diagrams, diagram nodes and edges, and design steps — shown inline.
 */
@injectable()
export class DocumentReviewWidget extends BaseWidget implements Navigatable {

    static readonly FACTORY_ID = 'co-review:document';

    @inject(DocumentReviewWidgetOptions) protected readonly options: DocumentReviewWidgetOptions;
    @inject(FileService) protected readonly fileService: FileService;
    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(ReviewNavigator) protected readonly navigator: ReviewNavigator;
    @inject(OpenerService) protected readonly openers: OpenerService;

    protected readonly header = document.createElement('div');
    protected readonly content = document.createElement('div');
    protected readonly popup = document.createElement('div');
    protected readonly roots: Root[] = [];
    protected readonly toDisposeOnPaint = new DisposableCollection();
    protected blocks: MermaidBlock[] = [];
    protected source = '';
    /** L1: top steps only; L2 (default): their children too; L3: everything. */
    protected level = 2;
    protected paintTimer: number | undefined;
    /** Increments with each load, so a slower earlier load stops instead of overwriting a newer one. */
    protected loadSeq = 0;
    /** Object URLs of the images of the current render, revoked on the next render. */
    protected imageUrls: string[] = [];
    /** The diagram open in the zoom overlay, if any. */
    protected zoomed: Disposable | undefined;

    get uri(): URI {
        return new URI(this.options.uri);
    }

    getResourceUri(): URI | undefined {
        // The overview page is Co-Review's own file, outside the repository: no breadcrumb path to it.
        return this.overview ? undefined : this.uri;
    }

    protected get overview(): boolean {
        return isOverviewPage(this.options.uri);
    }

    /** The overview page's briefing (above its text) and what it shows of the repository's coverage. */
    protected briefing: { host: HTMLElement; root: Root } | undefined;
    protected coverage: ReviewCoverage | undefined;
    protected coverageTimer: number | undefined;

    createMoveToUri(resourceUri: URI): URI {
        return resourceUri;
    }

    init(): void {
        this.id = `${DocumentReviewWidget.FACTORY_ID}:${this.options.uri}`;
        this.title.label = this.uri.path.base;
        this.title.caption = `Review: ${this.uri.path.fsPath()}`;
        this.title.iconClass = codicon('comment-discussion');
        this.title.closable = true;
        this.addClass('co-review-document');
        if (this.overview) {
            this.addClass('overview');
        }
        this.node.tabIndex = 0;
        this.header.className = 'co-review-doc-header';
        this.content.className = 'co-review-doc-content';
        this.popup.className = 'co-review-selection-actions co-review-doc-popup';
        this.popup.style.display = 'none';
        this.node.append(this.header, this.content, this.popup);
        this.renderHeader();
        this.toDispose.push(this.fileService.onDidFilesChange(e => e.contains(this.uri) && this.load()));
        // Pages Co-Review writes outside the repository (the overview page) aren't under the workspace's
        // file watcher: watch them directly, so they reload when they change.
        this.reviews.ready.then(() => {
            const root = this.reviews.root;
            if (!this.isDisposed && (!root || !new URI(root).isEqualOrParent(this.uri))) {
                this.toDispose.push(this.fileService.watch(this.uri));
            }
        });
        this.toDispose.push(this.reviews.onDidChange(() => {
            this.schedulePaint();
            this.renderBriefing();
            this.refreshCoverage();
            this.refreshChangedFiles();
        }));
        this.toDispose.push(this.reviews.onDidChangeDrafts(() => this.schedulePaint()));
        this.toDispose.push(this.reviews.onDidChangeCollapsed(() => this.schedulePaint()));
        this.content.addEventListener('mouseup', () => setTimeout(() => this.showSelectionActions()));
        // The step comment icon sits inside <summary>: don't let it fold the step.
        this.content.addEventListener('click', e => (e.target as Element).closest('.co-review-step-comment') && e.preventDefault(), true);
        this.content.addEventListener('click', e => this.onContentClick(e));
        this.node.addEventListener('scroll', () => this.hidePopup(), true);
        this.toDispose.push(this.toDisposeOnPaint);
        this.load();
    }

    protected override onActivateRequest(msg: Message): void {
        super.onActivateRequest(msg);
        if (!this.node.contains(document.activeElement)) {
            this.node.focus();
        }
    }

    protected renderHeader(): void {
        this.header.innerHTML = '';
        const button = (icon: string, title: string, run: () => void) => {
            const b = document.createElement('span');
            b.className = `${codicon(icon)} action-label`;
            b.title = title;
            b.onclick = run;
            return b;
        };
        const spacer = document.createElement('span');
        spacer.className = 'co-review-spacer';
        const path = document.createElement('span');
        path.className = 'co-review-doc-path';
        path.textContent = this.overview ? '' : this.reviews.relativePath(this.options.uri);
        const waiting = document.createElement('span');
        waiting.className = 'co-review-muted';
        waiting.textContent = this.pendingReload ? 'The document changed; it updates when you finish your comment.' : '';
        this.header.append(path, spacer, waiting,
            button('comment', 'Comment on the document', () => this.addDraft({ type: 'document' }, 'comment')),
            button('go-to-file', 'Open the source', () => this.navigator.open({ kind: 'file', uri: this.options.uri })));
    }

    /** A reload that waits while the reviewer writes a comment in the page. */
    protected pendingReload = false;

    protected typing(): boolean {
        const active = document.activeElement;
        return !!active && this.content.contains(active) && !!active.closest('.co-review-slot') && /^(TEXTAREA|INPUT)$/.test(active.tagName);
    }

    protected reloadWhenFree(): void {
        if (this.typing()) {
            this.pendingReload = true;
            this.renderHeader();
            return;
        }
        this.pendingReload = false;
        this.load();
    }

    /** Which block is at the top of the view, and how far into it: to stay on it when the page is rendered again. */
    protected rememberPlace(): { index: number; text: string; offset: number } | undefined {
        const top = this.node.getBoundingClientRect().top;
        const blocks = Array.from(this.content.children).filter(e => !e.classList.contains('co-review-slot'));
        const index = blocks.findIndex(e => e.getBoundingClientRect().bottom > top);
        if (index < 0 || this.node.scrollTop === 0) {
            return undefined;
        }
        return { index, text: (blocks[index].textContent ?? '').trim().slice(0, 120), offset: blocks[index].getBoundingClientRect().top - top };
    }

    protected restorePlace(place: { index: number; text: string; offset: number } | undefined): void {
        if (!place) {
            return;
        }
        const blocks = Array.from(this.content.children).filter(e => !e.classList.contains('co-review-slot'));
        // The same block (by its text, the nearest one with it), else the block now at that position.
        const same = blocks.map((e, i) => ({ e, i })).filter(({ e }) => (e.textContent ?? '').trim().slice(0, 120) === place.text)
            .sort((a, b) => Math.abs(a.i - place.index) - Math.abs(b.i - place.index))[0]?.e;
        const block = same ?? blocks[Math.min(place.index, blocks.length - 1)];
        if (block) {
            this.node.scrollTop += block.getBoundingClientRect().top - this.node.getBoundingClientRect().top - place.offset;
        }
    }

    protected async load(): Promise<void> {
        const seq = ++this.loadSeq;
        const place = this.content.childElementCount ? this.rememberPlace() : undefined;
        const stale = () => seq !== this.loadSeq || this.isDisposed;
        // Restored widgets load before the reviews do; the OpenSpec section and threads need them.
        await this.reviews.ready;
        let source: string;
        try {
            source = (await this.fileService.read(this.uri)).value;
        } catch (e) {
            if (!stale()) {
                this.content.textContent = `Cannot read ${this.uri.path.base}: ${e}`;
            }
            return;
        }
        if (stale()) {
            return;
        }
        this.source = source;
        this.revokeImages();
        const rendered = render(this.source, this.uri.path.base);
        this.title.label = isOverviewPage(this.options.uri) ? 'Overview' : `Review: ${this.uri.path.base}`;
        this.renderHeader();
        this.blocks = rendered.blocks;
        this.content.innerHTML = rendered.html;
        this.content.classList.toggle('pseudocode', rendered.pseudocode);
        if (rendered.format.warnings.length) {
            const note = document.createElement('div');
            note.className = 'co-review-format-warnings';
            note.innerHTML = `<strong>${rendered.format.declared ? 'Declares <code>co-review: design</code>, but:' : 'Format:'}</strong>`
                + `<ul>${rendered.format.warnings.map(w => `<li>${w.replace(/</g, '&lt;').replace(/`([^`]+)`/g, '<code>$1</code>')}</li>`).join('')}</ul>`;
            this.content.prepend(note);
        }
        this.renderMeta();
        if (this.overview) {
            this.mountBriefing();
            this.decorateChangedFiles();
        }
        this.setLevel(this.level);
        await this.renderOpenSpec();
        if (stale()) {
            return;
        }
        await this.resolveImages();
        if (stale()) {
            return;
        }
        await this.renderDiagrams(stale);
        if (!stale()) {
            this.paint();
            // After the threads are placed (they take room too), back to where the reviewer was.
            setTimeout(() => !stale() && this.restorePlace(place), 40);
        }
    }

    /** The briefing goes right under the page's title; it is mounted again with every load (the content is replaced). */
    protected mountBriefing(): void {
        const old = this.briefing;
        this.briefing = undefined;
        setTimeout(() => old?.root.unmount());
        const host = document.createElement('div');
        host.className = 'co-review-briefing';
        const title = this.content.querySelector(':scope > h1');
        if (title) {
            title.after(host);
        } else {
            this.content.prepend(host);
        }
        this.briefing = { host, root: createRoot(host) };
        this.renderBriefing();
        this.refreshCoverage();
    }

    protected renderBriefing(): void {
        const review = this.reviews.activeReview;
        if (!this.briefing || !review) {
            return;
        }
        this.briefing.root.render(<Briefing manager={this.reviews} navigator={this.navigator} review={review} coverage={this.coverage} />);
    }

    /** A repository review's files viewed, for the briefing; read shortly after the review changes. */
    protected refreshCoverage(): void {
        const review = this.reviews.activeReview;
        if (!this.overview || !review || review.bundle) {
            return;
        }
        window.clearTimeout(this.coverageTimer);
        this.coverageTimer = window.setTimeout(async () => {
            this.coverage = await this.reviews.getCoverage().catch(() => undefined);
            this.renderBriefing();
        }, 400);
    }

    /**
     * The change's files on the overview page, as the page lists them (a folder line with its totals, its files as
     * `path` +added −removed): each file with a bar of its lines, whether it is viewed and its comments, a click
     * opening its diff; the line of generated and version-only files folds away.
     */
    protected decorateChangedFiles(): void {
        const items = Array.from(this.content.querySelectorAll<HTMLLIElement>('li')).map(li => {
            const code = li.firstElementChild;
            const m = code?.tagName === 'CODE' && li.childElementCount === 1 ? /^\s*\+(\d+) −(\d+)\s*$/.exec(li.textContent!.slice(code.textContent!.length)) : null;
            return m ? { li, path: code!.textContent!, added: Number(m[1]), removed: Number(m[2]) } : undefined;
        }).filter((x): x is NonNullable<typeof x> => !!x);
        const most = Math.max(1, ...items.map(i => i.added + i.removed));
        for (const item of items) {
            const slash = item.path.lastIndexOf('/');
            item.li.classList.add('co-review-changed-file');
            item.li.dataset.path = item.path;
            item.li.title = `Open the diff of ${item.path}`;
            item.li.innerHTML = `<span class="co-review-changed-state"></span>`
                + `<span class="co-review-changed-path"><span class="name">${escape(item.path.slice(slash + 1))}</span></span>`
                + `<span class="co-review-changed-comments"></span>`
                + `<span class="co-review-changed-count"><span class="add">+${item.added}</span> <span class="del">−${item.removed}</span></span>`
                + `<span class="co-review-file-bar"><span class="add" style="width:${Math.round(100 * item.added / most)}%"></span>`
                + `<span class="del" style="width:${Math.round(100 * item.removed / most)}%"></span></span>`;
            item.li.addEventListener('click', e => {
                e.preventDefault();
                // The check marks the file viewed (or not) right here; the rest of the line opens its diff.
                if ((e.target as Element).closest('.co-review-changed-state')) {
                    const root = this.reviews.root;
                    if (root) {
                        const uri = new URI(root).resolve(item.path).toString();
                        this.reviews.setViewed([uri], !this.reviews.isViewed(uri));
                    }
                    return;
                }
                this.navigator.openChangedFile(item.path);
            });
            // The folder line above it.
            const folder = item.li.parentElement?.closest('li');
            if (folder && !folder.classList.contains('co-review-changed-folder')) {
                folder.classList.add('co-review-changed-folder');
                folder.parentElement?.classList.add('co-review-changed-tree');
            }
        }
        // Generated and version-only files: one folded line.
        for (const p of Array.from(this.content.querySelectorAll('p'))) {
            const m = /^Also changed, generated or version only \((\d+)\):/.exec(p.textContent ?? '');
            if (m) {
                const details = document.createElement('details');
                details.className = 'co-review-changed-routine';
                details.innerHTML = `<summary>${m[1]} generated, lock or version-only file${m[1] === '1' ? '' : 's'}</summary>`;
                const list = document.createElement('div');
                list.append(...Array.from(p.querySelectorAll('code')).map(c => {
                    const span = document.createElement('span');
                    span.className = 'co-review-changed-routine-file';
                    span.textContent = c.textContent;
                    span.title = `Open the diff of ${c.textContent}`;
                    span.onclick = () => this.navigator.openChangedFile(c.textContent!);
                    return span;
                }));
                details.append(list);
                p.replaceWith(details);
            }
        }
        this.refreshChangedFiles();
    }

    /** Each listed file's viewed state and comment count, as they are now (they change without the page changing). */
    protected refreshChangedFiles(): void {
        const review = this.reviews.activeReview;
        const root = this.reviews.root;
        if (!this.overview || !review || !root) {
            return;
        }
        for (const li of Array.from(this.content.querySelectorAll<HTMLLIElement>('li.co-review-changed-file'))) {
            const path = li.dataset.path!;
            const viewed = this.reviews.isViewed(new URI(root).resolve(path).toString());
            // On the diff (a patch anchor) or on the code (a line of the file in the window's folder).
            const comments = review.threads.filter(t => t.status !== 'resolved' && (this.reviews.patchFile(t.location)?.path
                ?? (t.location.uri?.startsWith('file:') ? this.reviews.relativePath(t.location.uri) : undefined)) === path).length;
            li.classList.toggle('viewed', viewed);
            const state = li.querySelector('.co-review-changed-state');
            if (state) {
                state.className = `co-review-changed-state ${codicon(viewed ? 'pass-filled' : 'circle-large-outline')}`;
                state.setAttribute('title', viewed ? 'Viewed. Click to unmark' : 'Click to mark as viewed');
            }
            const badge = li.querySelector('.co-review-changed-comments');
            if (badge) {
                badge.innerHTML = comments ? `<span class="${codicon('comment-discussion')}"></span>${comments}` : '';
                li.classList.toggle('commented', comments > 0);
                badge.setAttribute('title', comments ? `${comments} open comment${comments === 1 ? '' : 's'}` : '');
            }
        }
    }

    protected revokeImages(): void {
        this.imageUrls.splice(0).forEach(url => URL.revokeObjectURL(url));
    }

    /** The review directory's OpenSpec change as cards: the proposal, then one card per capability spec. */
    protected async renderOpenSpec(): Promise<void> {
        const bundle = this.reviews.activeReview?.bundle;
        if (!bundle?.openspec || this.uri.parent.path.fsPath() !== bundle.dir) {
            return;
        }
        const dir = URI.fromFilePath(bundle.openspec);
        const read = (uri: URI) => this.fileService.read(uri).then(r => r.value, () => undefined);
        const cards: { title: string; markdown: string; open?: boolean }[] = [];
        const proposal = await read(dir.resolve('proposal.md'));
        if (proposal) {
            const heading = proposal.match(/^#\s+(.+)$/m);
            cards.push({ title: heading ? heading[1].trim() : 'Proposal', markdown: proposal.replace(/^#\s+.+\n/, ''), open: true });
        }
        const specs = await this.fileService.resolve(dir.resolve('specs')).catch(() => undefined);
        for (const cap of (specs?.children ?? []).filter(c => c.isDirectory).sort((a, b) => a.name.localeCompare(b.name))) {
            const spec = await read(cap.resource.resolve('spec.md'));
            if (spec) {
                cards.push({ title: `spec: ${cap.name}`, markdown: spec });
            }
        }
        if (!cards.length) {
            return;
        }
        const section = document.createElement('section');
        section.className = 'co-review-openspec';
        section.innerHTML = `<h2><span class="codicon codicon-book"></span> OpenSpec change: ${dir.path.base}</h2>`;
        for (const card of cards) {
            const details = document.createElement('details');
            details.className = 'co-review-openspec-card';
            details.open = !!card.open;
            details.innerHTML = `<summary>${card.title.replace(/</g, '&lt;')}</summary><div class="co-review-openspec-body">${render(card.markdown, 'card.md').html}</div>`;
            section.append(details);
        }
        this.content.append(section);
    }

    /** Relative image sources are read through the file service (works in browser and desktop). */
    protected async resolveImages(): Promise<void> {
        for (const img of Array.from(this.content.querySelectorAll('img'))) {
            const src = img.getAttribute('src') ?? '';
            if (!src || /^[a-z]+:/i.test(src)) {
                continue;
            }
            try {
                const file = await this.fileService.readFile(this.uri.parent.resolve(src));
                const type = src.endsWith('.svg') ? 'image/svg+xml' : '';
                img.src = URL.createObjectURL(new Blob([file.value.buffer as ArrayBuffer], { type }));
                this.imageUrls.push(img.src);
            } catch {
                img.alt = `${img.alt || src} (not found)`;
            }
        }
    }

    protected async renderDiagrams(stale: () => boolean): Promise<void> {
        if (!this.blocks.length) {
            return;
        }
        const mermaid = (await import('mermaid')).default;
        const dark = document.body.classList.contains('theia-dark') || document.body.classList.contains('theia-hc');
        mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: dark ? 'dark' : 'default', fontFamily: 'var(--theia-ui-font-family)' });
        for (const block of this.blocks) {
            const host = this.content.querySelector<HTMLElement>(`.co-review-dgm[data-block-id="${CSS.escape(block.id)}"]`);
            if (!host) {
                continue;
            }
            const head = document.createElement('div');
            head.className = 'co-review-dgm-head';
            head.innerHTML = `<span class="codicon codicon-type-hierarchy-sub"></span><span>${block.id}</span><span class="co-review-spacer"></span>`
                + '<span class="codicon codicon-screen-full action-label" title="Open full size: zoom and pan" data-zoom-diagram="1"></span>'
                + '<span class="codicon codicon-comment action-label" title="Comment on this diagram" data-diagram="1"></span>';
            const body = document.createElement('div');
            body.className = 'co-review-dgm-body';
            host.append(head, body);
            if (stale()) {
                return;
            }
            try {
                const { svg } = await mermaid.render(`co-review-m-${block.id}-${Date.now()}`, block.code);
                body.innerHTML = svg;
                body.title = 'Click to open full size';
                this.wireDiagram(body);
            } catch (e) {
                body.innerHTML = `<div class="co-review-dgm-fail">Diagram failed to render: ${String(e instanceof Error ? e.message : e).slice(0, 200)}</div>`;
            }
        }
    }

    /** Node and edge ids as documented for anchors (llms.txt), so agents can address them. */
    protected wireDiagram(body: HTMLElement): void {
        body.querySelectorAll('.node').forEach(n => {
            // `flowchart-<id>-N`; newer Mermaid prefixes the render id (`<renderId>-flowchart-<id>-N`).
            n.setAttribute('data-node-id', (n.id || '').replace(/^.*?flowchart-/, '').replace(/-\d+$/, ''));
            n.classList.add('co-review-dgm-clickable');
        });
        body.querySelectorAll('.edgePaths path, .messageLine0, .messageLine1').forEach(p => {
            const m = (p.id || '').match(/(?:^|-)L[-_](.+?)[-_](.+?)[-_]\d+$/);
            p.setAttribute('data-edge-id', m ? `${m[1]}→${m[2]}` : (p.id || 'edge'));
            p.classList.add('co-review-dgm-clickable');
        });
        body.querySelectorAll('g.actor').forEach(a => {
            a.setAttribute('data-node-id', (a.textContent || '').trim().toLowerCase().replace(/\s+/g, '-'));
            a.classList.add('co-review-dgm-clickable');
        });
    }

    /**
     * A link in the document: another page (`./x.md`, or `/packages/x.md` from the review directory's root, as in
     * OKF bundles) opens as a review page; source code (`src/a.ts:42`, `src/a.ts#L42-L50`) opens in the editor at
     * that line; a folder is revealed in the explorer. Code paths are tried next to the document, then from the
     * review directory and the folder above it (the repository, for a bundle such as `<repo>/okf`).
     */
    protected async openLink(href: string): Promise<void> {
        const m = href.match(/^(.*?)(?::(\d+)(?:-(\d+))?|#L(\d+)(?:-L?(\d+))?)?$/)!;
        const target = m[1].replace(/#.*$/, '');
        const line = Number(m[2] ?? m[4]) || undefined;
        const endLine = Number(m[3] ?? m[5]) || undefined;
        const dir = this.reviews.activeReview?.bundle?.dir;
        const bases = target.startsWith('/')
            ? dir ? [URI.fromFilePath(dir)] : []
            : [this.uri.parent, ...dir ? [URI.fromFilePath(dir), URI.fromFilePath(dir).parent] : [],
                ...this.reviews.root ? [new URI(this.reviews.root)] : []];
        const rel = target.replace(/^\/+/, '');
        let uri: URI | undefined;
        for (const base of bases) {
            const candidate = rel ? base.resolve(rel) : base;
            if (await this.fileService.exists(candidate)) {
                uri = candidate;
                break;
            }
        }
        if (!uri) {
            open(this.openers, this.uri.parent.resolve(rel));
            return;
        }
        const stat = await this.fileService.resolve(uri);
        if (stat.isDirectory) {
            await this.navigator.open({ kind: 'directory', uri: uri.toString() });
        } else if (/\.(md|markdown|patch|diff)$/i.test(uri.path.base) && !line) {
            open(this.openers, uri);
        } else {
            await this.navigator.openReference(uri.path.fsPath(), line, endLine);
        }
    }

    /** Knowledge-base pages (OKF and similar) describe themselves in frontmatter: show what the page is about. */
    protected renderMeta(): void {
        const { data } = splitFrontmatter(this.source);
        if (!data.type || data['co-review']) {
            return;
        }
        const meta = document.createElement('div');
        meta.className = 'co-review-doc-meta';
        meta.innerHTML = `<span class="co-review-doc-type">${escape(data.type)}</span>`
            + (data.description ? `<span>${escape(data.description)}</span>` : '')
            + (data.resource ? `<a href="${escape(data.resource)}" title="Open in the code"><span class="${codicon('code')}"></span>${escape(data.resource)}</a>` : '');
        this.content.prepend(meta);
    }

    protected onContentClick(e: MouseEvent): void {
        const target = e.target as Element;
        const link = target.closest('a[href]');
        if (link && !link.closest('.co-review-slot')) {
            e.preventDefault();
            const href = link.getAttribute('href')!;
            if (/^https?:/i.test(href)) {
                window.open(href, '_blank', 'noopener');
            } else if (href.startsWith('#/patch/')) {
                // `#/patch/limiter` opens the patch as a review page.
                open(this.openers, this.uri.parent.resolve(`${href.slice('#/patch/'.length)}.patch`));
            } else if (!href.startsWith('#')) {
                this.openLink(decodeURI(href));
            }
            return;
        }
        const level = target.closest('.co-review-altitude button');
        if (level) {
            e.preventDefault();
            this.setLevel(Number(level.getAttribute('data-level')));
            return;
        }
        const step = target.closest('.co-review-step-comment');
        if (step) {
            e.preventDefault();
            e.stopPropagation();
            const id = step.getAttribute('data-step') ?? '';
            const text = step.closest('.co-review-step-line')?.querySelector('.co-review-step-text')?.textContent ?? id;
            this.addDraft({ type: 'tree-node', nodeId: id, source: text }, 'comment');
            return;
        }
        const host = target.closest('.co-review-dgm');
        const blockId = host?.getAttribute('data-block-id') ?? undefined;
        if (host && blockId) {
            if (target.closest('[data-diagram]')) {
                this.addDraft({ type: 'mermaid-block', blockId }, 'comment');
                return;
            }
            if (this.commentOnDiagram(blockId, target)) {
                return;
            }
            const svg = host.querySelector<SVGSVGElement>('.co-review-dgm-body svg');
            // The expand button, or a click on the diagram away from its nodes and edges (not ending a text selection).
            if (svg && (target.closest('[data-zoom-diagram]') || (target.closest('.co-review-dgm-body') && window.getSelection()?.isCollapsed !== false))) {
                this.zoomed?.dispose();
                this.zoomed = openDiagramZoom(svg, blockId, t => this.commentOnDiagram(blockId, t));
                return;
            }
        }
        const mark = target.closest('mark.co-review-mark');
        const threadId = mark?.getAttribute('data-thread');
        if (threadId) {
            const thread = this.reviews.activeReview?.threads.find(t => t.id === threadId);
            if (thread) {
                this.reviews.setCollapsed(thread.id, false);
                this.reviews.revealThread(thread.id);
            }
        }
    }

    /** A click on a diagram's node or edge starts a comment on it. */
    protected commentOnDiagram(blockId: string, target: Element): boolean {
        const node = target.closest('[data-node-id]');
        if (node) {
            const label = (node.textContent || '').trim();
            const nodeId = node.getAttribute('data-node-id')!;
            this.addDraft({ type: 'mermaid-node', blockId, nodeId, source: node.tagName === 'g' && node.classList.contains('actor') ? label : `${nodeId}[${label}]` }, 'comment');
            return true;
        }
        const edge = target.closest('[data-edge-id]');
        if (edge) {
            this.addDraft({ type: 'mermaid-edge', blockId, edgeId: edge.getAttribute('data-edge-id')! }, 'comment');
            return true;
        }
        return false;
    }

    protected setLevel(level: number): void {
        this.level = level;
        this.content.querySelectorAll<HTMLDetailsElement>('.co-review-design details').forEach(d => {
            d.open = Number(d.parentElement?.getAttribute('data-depth') ?? 1) < level;
        });
        this.content.querySelectorAll('.co-review-altitude button').forEach(b => b.classList.toggle('active', Number(b.getAttribute('data-level')) === level));
    }

    /** Text selection → floating Comment / Ask Agent actions (multi-line selections default to asking; no agent, no asking). */
    protected showSelectionActions(): void {
        const selection = window.getSelection();
        if (!selection || selection.isCollapsed || !selection.rangeCount) {
            this.hidePopup();
            return;
        }
        const range = selection.getRangeAt(0);
        if (!this.content.contains(range.commonAncestorContainer) || (range.commonAncestorContainer as Element).parentElement?.closest?.('.co-review-slot')) {
            this.hidePopup();
            return;
        }
        const anchor = textAnchor(this.content, SKIP, range);
        if (!anchor) {
            this.hidePopup();
            return;
        }
        const exact = anchor.exact!;
        const rect = range.getBoundingClientRect();
        const box = this.node.getBoundingClientRect();
        this.popup.innerHTML = '';
        const action = (label: string, icon: string, intent: ThreadIntent, primary: boolean, edit = false) => {
            const el = document.createElement('button');
            el.className = `co-review-selection-action ${primary ? 'primary' : ''}`;
            el.innerHTML = `<span class="${codicon(icon)}"></span><span>${label}</span>`;
            el.addEventListener('mousedown', e => {
                e.preventDefault();
                this.hidePopup();
                this.addDraft(anchor, intent, edit ? { before: exact, after: exact } : undefined);
            });
            return el;
        };
        const multiLine = exact.includes('\n');
        const ask = this.reviews.hasAgent;
        this.popup.append(...ask ? [action('Ask Agent', 'hubot', 'question', multiLine)] : [],
            action('Comment', 'comment', 'comment', !ask || !multiLine), action('Propose Edit', 'edit', 'comment', false, true));
        this.popup.style.display = 'flex';
        this.popup.style.top = `${rect.top - box.top + this.node.scrollTop - 34}px`;
        this.popup.style.left = `${Math.max(8, rect.left - box.left + rect.width / 2 - 80)}px`;
    }

    protected hidePopup(): void {
        this.popup.style.display = 'none';
    }

    protected addDraft(anchor: DocAnchor, intent: ThreadIntent, proposal?: { before: string; after: string }): void {
        if (!this.reviews.activeReview) {
            return;
        }
        window.getSelection()?.removeAllRanges();
        this.reviews.addDraft({ kind: 'document', uri: this.options.uri, docAnchor: anchor, anchor: { text: anchor.exact ?? anchor.source ?? '' } }, intent, proposal);
    }

    protected schedulePaint(): void {
        window.clearTimeout(this.paintTimer);
        this.paintTimer = window.setTimeout(() => !this.isDisposed && this.paint(), 30);
    }

    /** Places threads and drafts after the element they anchor to; unresolved anchors go to the top as outdated. */
    protected paint(): void {
        this.toDisposeOnPaint.dispose();
        const roots = this.roots.splice(0);
        setTimeout(() => roots.forEach(r => r.unmount()));
        this.content.querySelectorAll('.co-review-slot').forEach(s => s.remove());
        unmark(this.content);
        this.content.querySelectorAll('.co-review-dgm-marked, .co-review-step-marked').forEach(e => e.classList.remove('co-review-dgm-marked', 'co-review-step-marked'));

        const items: Anchored[] = [];
        for (const thread of this.reviews.activeReview?.threads ?? []) {
            if (thread.location.uri === this.options.uri && thread.location.kind === 'document' && thread.location.docAnchor) {
                items.push({ key: thread.id, anchor: thread.location.docAnchor, element: root => this.renderThread(root, thread) });
            }
        }
        for (const draft of this.reviews.draftsForUri(this.options.uri)) {
            if (draft.location.kind === 'document' && draft.location.docAnchor) {
                items.push({ key: draft.id, anchor: draft.location.docAnchor, element: root => this.renderDraft(root, draft) });
            }
        }
        const top = document.createElement('div');
        top.className = 'co-review-slot co-review-slot-top';
        this.content.prepend(top);
        for (const item of items) {
            const thread = this.reviews.activeReview?.threads.find(t => t.id === item.key);
            // Its text was removed, or can't be told apart from other text: out of scope, in the Review panel only.
            if (thread && isOutOfScope(thread)) {
                this.reviews.setAnchorState(thread.id, thread.location.tracked!.status === 'removed' ? 'removed' : 'ambiguous');
                continue;
            }
            const { target, marks } = locate(this.content, item.anchor);
            marks?.forEach(m => m.setAttribute('data-thread', item.key));
            const outdated = item.anchor.type !== 'document' && !target;
            if (thread) {
                this.reviews.setAnchorState(thread.id, outdated ? 'outdated' : thread.location.tracked?.status === 'modified' ? 'changed' : 'exact');
            }
            if (thread && !outdated && this.isCollapsed(thread)) {
                continue;
            }
            const slot = document.createElement('div');
            slot.className = 'co-review-slot co-review-inline-content';
            if (target && item.anchor.type !== 'document') {
                // After the paragraph / list item / diagram / step the anchor is in.
                const block = target.closest('li.co-review-step') ?? target;
                block.after(slot);
            } else {
                top.append(slot);
            }
            const root = createRoot(slot);
            item.element(root);
            this.roots.push(root);
        }
    }

    protected isCollapsed(thread: ReviewThread): boolean {
        return thread.status === 'resolved' || this.reviews.isCollapsed(thread.id);
    }

    protected renderThread(root: Root, thread: ReviewThread): void {
        root.render(<ThreadView manager={this.reviews} thread={thread} inline
            label={DocAnchor.describe(thread.location.docAnchor!)}
            anchorState={this.reviews.getAnchorState(thread.id)}
            onHide={() => this.reviews.setCollapsed(thread.id, true)}
            onOpenReference={(path, line, end) => this.navigator.openReference(path, line, end)} />);
    }

    protected renderDraft(root: Root, draft: ReviewDraft): void {
        root.render(<DraftEditor manager={this.reviews} draft={draft} acceptFocus label={DocAnchor.describe(draft.location.docAnchor!)} />);
    }

    /** Scrolls a thread of this document into view. */
    reveal(threadId: string): void {
        const mark = this.content.querySelector(`mark[data-thread="${CSS.escape(threadId)}"]`);
        (mark ?? this.content).scrollIntoView({ block: 'center', behavior: 'smooth' });
    }

    protected override onCloseRequest(msg: Message): void {
        super.onCloseRequest(msg);
        this.dispose();
    }

    override dispose(): void {
        window.clearTimeout(this.paintTimer);
        window.clearTimeout(this.coverageTimer);
        const briefing = this.briefing;
        setTimeout(() => briefing?.root.unmount());
        this.zoomed?.dispose();
        this.revokeImages();
        const roots = this.roots.splice(0);
        setTimeout(() => roots.forEach(r => r.unmount()));
        super.dispose();
    }
}

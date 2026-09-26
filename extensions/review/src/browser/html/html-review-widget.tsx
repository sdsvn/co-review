import * as React from '@theia/core/shared/react';
import { createRoot, Root } from '@theia/core/shared/react-dom/client';
import { BaseWidget, Message, Navigatable, codicon } from '@theia/core/lib/browser';
import URI from '@theia/core/lib/common/uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { FileService } from '@theia/filesystem/lib/browser/file-service';
import { OpenerService, open } from '@theia/core/lib/browser/opener-service';
import { DocAnchor, ReviewThread, ThreadIntent } from '../../common/review-model';
import { DraftEditor, ThreadView } from '../review-components';
import { ReviewDraft, ReviewManager } from '../review-manager';
import { ReviewNavigator } from '../review-navigator';
import { markText, textAnchor, unmark } from '../document/text-anchor';

export const HtmlReviewWidgetOptions = Symbol('HtmlReviewWidgetOptions');
export interface HtmlReviewWidgetOptions {
    uri: string;
}

/** Not part of the page's text. */
const SKIP = 'script, style, noscript, template';

/** Our marks inside the page (the page's own styles apply there, not the workbench's). */
const PAGE_STYLE = 'mark.co-review-mark{background:rgba(230,170,50,.35)!important;color:inherit!important;border-bottom:2px solid #e6aa32;cursor:pointer;padding:0}'
    + '.co-review-el-marked{outline:2px solid #e6aa32!important;outline-offset:2px}'
    + '.co-review-picking *:hover{outline:2px dashed #4f46e5!important;outline-offset:1px;cursor:crosshair!important}';

const MIME: Record<string, string> = {
    png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', avif: 'image/avif', svg: 'image/svg+xml',
    ico: 'image/x-icon', bmp: 'image/bmp', mp4: 'video/mp4', webm: 'video/webm', ogg: 'audio/ogg', mp3: 'audio/mpeg', wav: 'audio/wav'
};

/** A selector for `el` from the page's <body> (or from the nearest element with a unique id). */
function selectorOf(el: Element): string {
    const parts: string[] = [];
    for (let e: Element | null = el; e && e.tagName !== 'BODY' && e.tagName !== 'HTML'; e = e.parentElement) {
        if (e.id && e.ownerDocument.querySelectorAll(`#${CSS.escape(e.id)}`).length === 1) {
            return [`#${CSS.escape(e.id)}`, ...parts].join(' > ');
        }
        const tag = e.tagName.toLowerCase();
        const siblings = Array.from(e.parentElement?.children ?? []).filter(c => c.tagName === e!.tagName);
        parts.unshift(siblings.length > 1 ? `${tag}:nth-of-type(${siblings.indexOf(e) + 1})` : tag);
    }
    return ['body', ...parts].join(' > ');
}

/** How an element reads in a thread label: its alt text, title or first words. */
function labelOf(el: Element): string {
    const tag = el.tagName.toLowerCase();
    const text = (el.getAttribute('alt') || el.getAttribute('aria-label') || el.getAttribute('title') || el.textContent || '').replace(/\s+/g, ' ').trim();
    return text ? `<${tag}> “${text.slice(0, 40)}${text.length > 40 ? '…' : ''}”` : `<${tag}>`;
}

function base64(bytes: Uint8Array): string {
    let out = '';
    for (let i = 0; i < bytes.length; i += 0x8000) {
        out += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    }
    return btoa(out);
}

interface Item {
    key: string;
    anchor: DocAnchor;
    thread?: ReviewThread;
    draft?: ReviewDraft;
    /** Position of the anchor in the page, for ordering; -1 for the page itself. */
    top: number;
    found: boolean;
}

/**
 * Review view of an HTML page: the page rendered in a sandboxed frame, with comments on selected text,
 * on elements (images, charts, sections) and on the page, listed next to it. Scripts are off by default,
 * so the page cannot reach the workbench and its text can be anchored; "Run scripts" renders it
 * interactively, where only page-level comments are possible.
 */
@injectable()
export class HtmlReviewWidget extends BaseWidget implements Navigatable {

    static readonly FACTORY_ID = 'co-review:html';

    @inject(HtmlReviewWidgetOptions) protected readonly options: HtmlReviewWidgetOptions;
    @inject(FileService) protected readonly fileService: FileService;
    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(ReviewNavigator) protected readonly navigator: ReviewNavigator;
    @inject(OpenerService) protected readonly openers: OpenerService;

    protected readonly header = document.createElement('div');
    protected readonly body = document.createElement('div');
    protected readonly side = document.createElement('div');
    protected readonly popup = document.createElement('div');
    protected frame: HTMLIFrameElement | undefined;
    protected sideRoot: Root | undefined;
    protected scripts = false;
    protected picking = false;
    protected selected: string | undefined;
    protected paintTimer: number | undefined;

    get uri(): URI {
        return new URI(this.options.uri);
    }

    getResourceUri(): URI {
        return this.uri;
    }

    createMoveToUri(resourceUri: URI): URI {
        return resourceUri;
    }

    /** The page's document; undefined while scripts run (the frame is then cross-origin). */
    protected get page(): Document | undefined {
        return this.scripts ? undefined : this.frame?.contentDocument ?? undefined;
    }

    init(): void {
        this.id = `${HtmlReviewWidget.FACTORY_ID}:${this.options.uri}`;
        this.title.label = `Review: ${this.uri.path.base}`;
        this.title.caption = `Review: ${this.uri.path.fsPath()}`;
        this.title.iconClass = codicon('globe');
        this.title.closable = true;
        this.addClass('co-review-html');
        this.node.tabIndex = 0;
        this.header.className = 'co-review-doc-header';
        this.body.className = 'co-review-html-body';
        this.side.className = 'co-review-html-side';
        this.popup.className = 'co-review-selection-actions co-review-doc-popup';
        this.popup.style.display = 'none';
        this.body.append(this.side, this.popup);
        this.node.append(this.header, this.body);
        this.sideRoot = createRoot(this.side);
        this.renderHeader();
        this.toDispose.push(this.fileService.onDidFilesChange(e => e.contains(this.uri) && this.load()));
        this.toDispose.push(this.reviews.onDidChange(() => this.schedulePaint()));
        this.toDispose.push(this.reviews.onDidChangeDrafts(() => this.schedulePaint()));
        this.toDispose.push(this.reviews.onDidChangeCollapsed(() => this.schedulePaint()));
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
        const button = (icon: string, title: string, run: () => void, active = false) => {
            const b = document.createElement('span');
            b.className = `${codicon(icon)} action-label${active ? ' active' : ''}`;
            b.title = title;
            b.onclick = run;
            return b;
        };
        const spacer = document.createElement('span');
        spacer.className = 'co-review-spacer';
        const path = document.createElement('span');
        path.className = 'co-review-doc-path';
        path.textContent = this.reviews.relativePath(this.options.uri);
        const note = document.createElement('span');
        note.className = 'co-review-html-note';
        note.textContent = this.scripts ? 'Scripts on: comment on the page as a whole' : '';
        this.header.append(path, spacer, note,
            ...this.scripts ? [] : [button('inspect', this.picking ? 'Stop picking' : 'Comment on an element (click it in the page)', () => this.setPicking(!this.picking), this.picking)],
            button(this.scripts ? 'debug-stop' : 'play', this.scripts ? 'Stop scripts (comment on text and elements again)' : 'Run the page\'s scripts',
                () => this.setScripts(!this.scripts), this.scripts),
            button('refresh', 'Reload', () => this.load()),
            button('comment', 'Comment on the page', () => this.addDraft({ type: 'document' }, 'comment')),
            button('go-to-file', 'Open the source', () => this.navigator.open({ kind: 'file', uri: this.options.uri })));
    }

    protected setScripts(on: boolean): void {
        this.scripts = on;
        this.picking = false;
        this.renderHeader();
        this.load();
    }

    protected setPicking(on: boolean): void {
        this.picking = on;
        this.page?.documentElement.classList.toggle('co-review-picking', on);
        this.renderHeader();
    }

    protected async load(): Promise<void> {
        await this.reviews.ready;
        let html: string;
        try {
            html = await this.inline((await this.fileService.read(this.uri)).value);
        } catch (e) {
            this.side.textContent = `Cannot read ${this.uri.path.base}: ${e}`;
            return;
        }
        const scrollY = this.frame?.contentWindow && !this.scripts ? this.page?.defaultView?.scrollY ?? 0 : 0;
        // The sandbox is fixed when the frame loads: a new frame for each render.
        const frame = document.createElement('iframe');
        frame.className = 'co-review-html-frame';
        frame.setAttribute('sandbox', this.scripts ? 'allow-scripts allow-popups allow-forms allow-modals' : 'allow-same-origin allow-popups');
        frame.srcdoc = html;
        frame.addEventListener('load', () => this.onPageLoad(scrollY), { once: true });
        if (this.frame) {
            this.frame.replaceWith(frame);
        } else {
            this.body.prepend(frame);
        }
        this.frame = frame;
    }

    /**
     * The frame has no base URL: local stylesheets and scripts are inlined, local images and media become
     * data URLs. Paths resolve next to the page, `/…` from the repository.
     */
    protected async inline(source: string): Promise<string> {
        const doc = new DOMParser().parseFromString(source, 'text/html');
        const local = (ref: string | null): ref is string => !!ref && !/^([a-z][a-z0-9+.-]*:|\/\/|#)/i.test(ref);
        const resolve = (ref: string) => {
            const clean = decodeURI(ref.split(/[?#]/)[0]);
            return clean.startsWith('/') && this.reviews.root ? new URI(this.reviews.root).resolve(clean.slice(1)) : this.uri.parent.resolve(clean);
        };
        const text = (ref: string) => this.fileService.read(resolve(ref)).then(r => r.value, () => undefined);
        const tasks: Promise<void>[] = [];
        doc.querySelectorAll('link[rel~="stylesheet"][href]').forEach(link => {
            const href = link.getAttribute('href');
            if (local(href)) {
                tasks.push(text(href).then(css => {
                    if (css !== undefined) {
                        const style = doc.createElement('style');
                        style.textContent = css;
                        link.replaceWith(style);
                    }
                }));
            }
        });
        if (!this.scripts) {
            // They would not run in the sandbox anyway.
            doc.querySelectorAll('script').forEach(script => script.remove());
        }
        doc.querySelectorAll('script[src]').forEach(script => {
            const src = script.getAttribute('src');
            if (local(src)) {
                tasks.push(text(src).then(js => {
                    if (js !== undefined) {
                        script.removeAttribute('src');
                        script.textContent = js;
                    }
                }));
            }
        });
        doc.querySelectorAll('img[src], source[src], video[src], audio[src], video[poster], input[type="image"][src]').forEach(el => {
            for (const attr of ['src', 'poster']) {
                const ref = el.getAttribute(attr);
                if (local(ref)) {
                    tasks.push(this.fileService.readFile(resolve(ref)).then(file => {
                        const ext = ref.split(/[?#]/)[0].split('.').pop()?.toLowerCase() ?? '';
                        el.setAttribute(attr, `data:${MIME[ext] ?? 'application/octet-stream'};base64,${base64(file.value.buffer)}`);
                    }, () => undefined));
                }
            }
        });
        await Promise.all(tasks);
        // Links the page doesn't handle itself open outside the frame.
        const base = doc.createElement('base');
        base.target = '_blank';
        doc.head.prepend(base);
        return `<!doctype html>\n${doc.documentElement.outerHTML}`;
    }

    protected onPageLoad(scrollY: number): void {
        const page = this.page;
        if (page) {
            const style = page.createElement('style');
            style.textContent = PAGE_STYLE;
            page.head.append(style);
            page.addEventListener('mouseup', () => setTimeout(() => this.showSelectionActions()));
            page.addEventListener('click', e => this.onPageClick(e), true);
            page.addEventListener('scroll', () => this.hidePopup());
            page.addEventListener('keydown', e => e.key === 'Escape' && this.picking && this.setPicking(false));
            page.defaultView?.scrollTo(0, scrollY);
        }
        this.paint();
    }

    protected onPageClick(e: MouseEvent): void {
        const target = e.target as Element;
        if (this.picking) {
            e.preventDefault();
            e.stopPropagation();
            this.setPicking(false);
            this.addDraft({ type: 'element', selector: selectorOf(target), source: labelOf(target) }, 'comment');
            return;
        }
        const mark = target.closest('mark.co-review-mark, .co-review-el-marked');
        const key = mark?.getAttribute('data-thread');
        if (key) {
            e.preventDefault();
            this.select(key);
            return;
        }
        const link = target.closest('a[href]');
        const href = link?.getAttribute('href');
        if (!href || href.startsWith('#')) {
            return;
        }
        e.preventDefault();
        if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('//')) {
            window.open(href, '_blank', 'noopener');
        } else {
            // Another page or file of the repository: open it here (HTML and Markdown render, the rest opens in the editor).
            const clean = decodeURI(href.split(/[?#]/)[0]);
            open(this.openers, clean.startsWith('/') && this.reviews.root ? new URI(this.reviews.root).resolve(clean.slice(1)) : this.uri.parent.resolve(clean));
        }
    }

    /** Text selection → floating Comment / Ask Agent actions, positioned over the frame. */
    protected showSelectionActions(): void {
        const page = this.page;
        const selection = page?.getSelection();
        if (!page || !this.frame || this.picking || !selection || selection.isCollapsed || !selection.rangeCount) {
            this.hidePopup();
            return;
        }
        const exact = selection.toString().trim();
        if (exact.length < 3) {
            this.hidePopup();
            return;
        }
        const anchor = textAnchor(page.body, SKIP, exact);
        const rect = selection.getRangeAt(0).getBoundingClientRect();
        const frameBox = this.frame.getBoundingClientRect();
        const box = this.body.getBoundingClientRect();
        this.popup.innerHTML = '';
        const action = (label: string, icon: string, intent: ThreadIntent, primary: boolean) => {
            const el = document.createElement('button');
            el.className = `co-review-selection-action ${primary ? 'primary' : ''}`;
            el.innerHTML = `<span class="${codicon(icon)}"></span><span>${label}</span>`;
            el.addEventListener('mousedown', e => {
                e.preventDefault();
                this.hidePopup();
                selection.removeAllRanges();
                this.addDraft(anchor, intent);
            });
            return el;
        };
        const ask = this.reviews.hasAgent;
        const multiLine = exact.includes('\n');
        this.popup.append(...ask ? [action('Ask Agent', 'hubot', 'question', multiLine)] : [], action('Comment', 'comment', 'comment', !ask || !multiLine));
        this.popup.style.display = 'flex';
        this.popup.style.top = `${Math.max(4, frameBox.top - box.top + rect.top - 34)}px`;
        this.popup.style.left = `${Math.max(8, frameBox.left - box.left + rect.left + rect.width / 2 - 60)}px`;
    }

    protected hidePopup(): void {
        this.popup.style.display = 'none';
    }

    protected addDraft(anchor: DocAnchor, intent: ThreadIntent): void {
        if (!this.reviews.activeReview) {
            return;
        }
        this.reviews.addDraft({ kind: 'document', uri: this.options.uri, docAnchor: anchor, anchor: { text: anchor.exact ?? anchor.source ?? '' } }, intent);
    }

    protected schedulePaint(): void {
        window.clearTimeout(this.paintTimer);
        this.paintTimer = window.setTimeout(() => this.paint(), 30);
    }

    /** Marks each anchor in the page and lists the threads and drafts in page order next to it. */
    protected paint(): void {
        const page = this.page;
        if (page?.body) {
            unmark(page.body);
            page.querySelectorAll('.co-review-el-marked').forEach(e => {
                e.classList.remove('co-review-el-marked');
                e.removeAttribute('data-thread');
            });
        }
        const items: Item[] = [];
        const add = (key: string, anchor: DocAnchor, thread?: ReviewThread, draft?: ReviewDraft) => {
            let top = -1;
            let found = anchor.type === 'document';
            if (page?.body && anchor.type === 'text') {
                const marks = markText(page.body, anchor, SKIP);
                marks.forEach(m => m.setAttribute('data-thread', key));
                found = marks.length > 0;
                top = found ? marks[0].getBoundingClientRect().top + (page.defaultView?.scrollY ?? 0) : Infinity;
            } else if (page?.body && anchor.type === 'element') {
                const el = anchor.selector ? page.querySelector(anchor.selector) : undefined;
                el?.classList.add('co-review-el-marked');
                el?.setAttribute('data-thread', key);
                found = !!el;
                top = el ? el.getBoundingClientRect().top + (page.defaultView?.scrollY ?? 0) : Infinity;
            }
            if (thread && page) {
                this.reviews.setAnchorState(thread.id, found ? 'exact' : 'outdated');
            }
            items.push({ key, anchor, thread, draft, top, found });
        };
        for (const thread of this.reviews.activeReview?.threads ?? []) {
            if (thread.location.uri === this.options.uri && thread.location.kind === 'document' && thread.location.docAnchor) {
                add(thread.id, thread.location.docAnchor, thread);
            }
        }
        for (const draft of this.reviews.draftsForUri(this.options.uri)) {
            if (draft.location.kind === 'document' && draft.location.docAnchor) {
                add(draft.id, draft.location.docAnchor, undefined, draft);
            }
        }
        items.sort((a, b) => a.top - b.top);
        this.sideRoot?.render(this.renderSide(items));
    }

    protected renderSide(items: Item[]): React.ReactNode {
        if (!items.length) {
            return <div className='co-review-html-empty'>
                {this.scripts ? 'Comment on the page with the comment button above.'
                    : 'Select text in the page to comment on it, or use the inspect button to comment on an element.'}
            </div>;
        }
        return items.map(item => {
            const label = DocAnchor.describe(item.anchor);
            const focus = () => this.select(item.key);
            let view: React.ReactNode;
            if (item.draft) {
                view = <DraftEditor manager={this.reviews} draft={item.draft} acceptFocus label={label} onOpen={focus} />;
            } else if (item.thread && (item.thread.status === 'resolved' || this.reviews.isCollapsed(item.thread.id))) {
                const thread = item.thread;
                view = <div className='co-review-html-collapsed' onClick={() => { this.reviews.setCollapsed(thread.id, false); focus(); }}>
                    <span className='co-review-number'>#{thread.number}</span> <span>{label}</span>
                    {thread.status === 'resolved' && <span className='co-review-badge resolved'>resolved</span>}
                </div>;
            } else if (item.thread) {
                const thread = item.thread;
                view = <ThreadView manager={this.reviews} thread={thread} inline label={label}
                    anchorState={this.page ? (item.found ? 'exact' : 'outdated') : undefined}
                    onHide={() => this.reviews.setCollapsed(thread.id, true)}
                    onOpenReference={(path, line, end) => this.navigator.openReference(path, line, end)} />;
            }
            return <div key={item.key} data-key={item.key} className={`co-review-html-item ${this.selected === item.key ? 'selected' : ''}`}
                onMouseDown={e => !(e.target as Element).closest('textarea, input, button') && this.highlight(item.key)}>{view}</div>;
        });
    }

    /** Scrolls the anchor into view in the page and the thread into view in the list. */
    protected select(key: string): void {
        this.selected = key;
        this.highlight(key);
        this.side.querySelector(`[data-key="${CSS.escape(key)}"]`)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        this.paint();
    }

    protected highlight(key: string): void {
        this.page?.querySelector(`[data-thread="${CSS.escape(key)}"]`)?.scrollIntoView({ block: 'center', behavior: 'smooth' });
    }

    /** Scrolls a thread of this page into view. */
    reveal(threadId: string): void {
        this.select(threadId);
    }

    protected override onCloseRequest(msg: Message): void {
        super.onCloseRequest(msg);
        this.dispose();
    }

    override dispose(): void {
        const root = this.sideRoot;
        this.sideRoot = undefined;
        setTimeout(() => root?.unmount());
        super.dispose();
    }
}

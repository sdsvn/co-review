import { Disposable, DisposableCollection } from '@theia/core/lib/common/disposable';
import * as React from '@theia/core/shared/react';
import { createRoot, Root } from '@theia/core/shared/react-dom/client';
import * as monaco from '@theia/monaco-editor-core';

let nextId = 0;

/**
 * A React-rendered block between two editor lines (GitHub-style inline conversation).
 * A view zone reserves the vertical space; an overlay widget positioned over it holds the
 * interactive content, since view zones themselves don't receive input.
 */
export class InlineZone implements Disposable {

    protected readonly node = document.createElement('div');
    protected readonly content = document.createElement('div');
    protected readonly root: Root;
    protected readonly overlay: monaco.editor.IOverlayWidget;
    protected readonly toDispose = new DisposableCollection();
    protected zone: monaco.editor.IViewZone | undefined;
    protected zoneId: string | undefined;
    protected height = 0;
    afterLine = -1;

    constructor(protected readonly editor: monaco.editor.ICodeEditor) {
        const id = `co-review.inline.${nextId++}`;
        this.node.className = 'co-review-inline-zone';
        this.node.style.top = '-1000px';
        this.content.className = 'co-review-inline-content';
        this.node.appendChild(this.content);
        // Keep editor mouse handling (selection, drag) away from the inline content.
        this.node.addEventListener('mousedown', e => e.stopPropagation());
        this.node.addEventListener('wheel', e => e.stopPropagation(), { passive: true });
        this.root = createRoot(this.content);
        this.overlay = { getId: () => id, getDomNode: () => this.node, getPosition: () => null };
        editor.addOverlayWidget(this.overlay);
        this.layoutWidth();
        this.toDispose.push(editor.onDidLayoutChange(info => this.layoutWidth(info)));
        // Workbench keybindings run in the document capture phase and would treat keys typed here
        // as editor keys (Cmd+Enter = insert line, Cmd+Z = undo in the file, …). Intercept earlier:
        // our own keys are marked handled (Theia skips prevented events) and still reach React;
        // everything else is kept away from the workbench while native text editing still works.
        const onKey = (e: KeyboardEvent) => {
            if (!(e.target instanceof Node) || !this.node.contains(e.target)) {
                return;
            }
            if (e.key === 'Escape' || (e.key === 'Enter' && (e.metaKey || e.ctrlKey))) {
                e.preventDefault();
            } else {
                e.stopPropagation();
            }
        };
        window.addEventListener('keydown', onKey, true);
        this.toDispose.push(Disposable.create(() => window.removeEventListener('keydown', onKey, true)));
        const observer = new ResizeObserver(() => this.setHeight(this.content.offsetHeight));
        observer.observe(this.content);
        this.toDispose.push(Disposable.create(() => observer.disconnect()));
    }

    render(element: React.ReactElement): void {
        this.root.render(element);
    }

    /** Places the zone after the given (1-based) line. */
    place(afterLine: number): void {
        if (afterLine === this.afterLine) {
            return;
        }
        this.afterLine = afterLine;
        this.editor.changeViewZones(accessor => {
            if (this.zoneId) {
                accessor.removeZone(this.zoneId);
            }
            this.zone = {
                afterLineNumber: afterLine,
                heightInPx: Math.max(this.height, 60),
                domNode: document.createElement('div'),
                onDomNodeTop: top => this.node.style.top = `${top}px`,
                onComputedHeight: height => this.node.style.height = `${height}px`
            };
            this.zoneId = accessor.addZone(this.zone);
        });
    }

    protected setHeight(contentHeight: number): void {
        const height = Math.ceil(contentHeight) + 8;
        if (height === this.height || !contentHeight) {
            return;
        }
        this.height = height;
        if (this.zone && this.zoneId) {
            this.zone.heightInPx = height;
            const id = this.zoneId;
            this.editor.changeViewZones(accessor => accessor.layoutZone(id));
        }
    }

    protected layoutWidth(info = this.editor.getLayoutInfo()): void {
        const right = info.minimap.minimapWidth + info.verticalScrollbarWidth + 12;
        this.node.style.left = `${info.contentLeft}px`;
        this.node.style.width = `${Math.max(200, info.width - info.contentLeft - right)}px`;
    }

    dispose(): void {
        this.toDispose.dispose();
        if (this.zoneId) {
            const id = this.zoneId;
            this.editor.changeViewZones(accessor => accessor.removeZone(id));
        }
        this.editor.removeOverlayWidget(this.overlay);
        // Unmounting synchronously during a React render is not allowed.
        setTimeout(() => this.root.unmount());
    }
}

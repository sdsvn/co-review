import { codicon } from '@theia/core/lib/browser';
import { Disposable } from '@theia/core/lib/common/disposable';

const MIN = 0.1;
const MAX = 12;

/**
 * A rendered diagram in a window-sized overlay: the wheel and the buttons zoom (toward the pointer), dragging
 * pans, double-click zooms in (shift: out), Esc closes. A click that isn't a drag goes to `onClick` (nodes and edges still take
 * comments); when it handles the click, the overlay closes.
 */
export function openDiagramZoom(svg: SVGSVGElement, title: string, onClick: (target: Element) => boolean): Disposable {
    const clone = svg.cloneNode(true) as SVGSVGElement;
    const vb = svg.viewBox.baseVal;
    const box = vb && vb.width ? { width: vb.width, height: vb.height } : svg.getBoundingClientRect();
    clone.setAttribute('width', String(box.width));
    clone.setAttribute('height', String(box.height));
    clone.style.maxWidth = 'none';

    const overlay = document.createElement('div');
    overlay.className = 'co-review-dgm-zoom';
    overlay.innerHTML = `<div class="co-review-dgm-zoom-bar"><span class="${codicon('type-hierarchy-sub')}"></span>`
        + '<span class="co-review-dgm-zoom-title"></span><span class="co-review-spacer"></span>'
        + '<span class="co-review-dgm-zoom-level"></span>'
        + `<span class="${codicon('zoom-out')} action-label" title="Zoom out (−)" data-zoom="out"></span>`
        + `<span class="${codicon('zoom-in')} action-label" title="Zoom in (+)" data-zoom="in"></span>`
        + `<span class="${codicon('screen-normal')} action-label" title="Fit to window (0)" data-zoom="fit"></span>`
        + `<span class="${codicon('close')} action-label" title="Close (Esc)" data-zoom="close"></span></div>`
        + '<div class="co-review-dgm-zoom-stage"><div class="co-review-dgm-zoom-canvas"></div></div>';
    overlay.querySelector('.co-review-dgm-zoom-title')!.textContent = title;
    const stage = overlay.querySelector<HTMLElement>('.co-review-dgm-zoom-stage')!;
    const canvas = overlay.querySelector<HTMLElement>('.co-review-dgm-zoom-canvas')!;
    const level = overlay.querySelector<HTMLElement>('.co-review-dgm-zoom-level')!;
    canvas.append(clone);
    document.body.append(overlay);

    let scale = 1, x = 0, y = 0;
    const apply = () => {
        canvas.style.transform = `translate(${x}px, ${y}px) scale(${scale})`;
        level.textContent = `${Math.round(scale * 100)}%`;
    };
    const fit = () => {
        const r = stage.getBoundingClientRect();
        scale = Math.min(MAX, Math.max(MIN, Math.min((r.width - 48) / box.width, (r.height - 48) / box.height)));
        x = (r.width - box.width * scale) / 2;
        y = (r.height - box.height * scale) / 2;
        apply();
    };
    /** Zooms by `factor`, keeping the stage point (px, py) where it is; the stage's center by default. */
    const zoom = (factor: number, px?: number, py?: number) => {
        const r = stage.getBoundingClientRect();
        const cx = px ?? r.width / 2, cy = py ?? r.height / 2;
        const next = Math.min(MAX, Math.max(MIN, scale * factor));
        x = cx - (cx - x) * (next / scale);
        y = cy - (cy - y) * (next / scale);
        scale = next;
        apply();
    };

    stage.addEventListener('wheel', e => {
        e.preventDefault();
        const r = stage.getBoundingClientRect();
        // Trackpad pinches arrive as ctrl+wheel with small deltas; mouse wheels with large ones.
        zoom(Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0015)), e.clientX - r.left, e.clientY - r.top);
    }, { passive: false });

    let drag: { id: number; sx: number; sy: number; x: number; y: number; moved: boolean } | undefined;
    let dragged = false;
    stage.addEventListener('pointerdown', e => {
        if (e.button === 0) {
            drag = { id: e.pointerId, sx: e.clientX, sy: e.clientY, x, y, moved: false };
        }
    });
    stage.addEventListener('pointermove', e => {
        if (!drag || e.pointerId !== drag.id) {
            return;
        }
        if (!drag.moved && Math.hypot(e.clientX - drag.sx, e.clientY - drag.sy) > 4) {
            drag.moved = true;
            stage.setPointerCapture(e.pointerId);
            stage.classList.add('dragging');
        }
        if (drag.moved) {
            x = drag.x + e.clientX - drag.sx;
            y = drag.y + e.clientY - drag.sy;
            apply();
        }
    });
    const endDrag = () => {
        dragged = !!drag?.moved;
        drag = undefined;
        stage.classList.remove('dragging');
    };
    stage.addEventListener('pointerup', endDrag);
    stage.addEventListener('pointercancel', endDrag);

    const close = () => disposable.dispose();
    overlay.addEventListener('click', e => {
        const target = e.target as Element;
        const action = target.closest('[data-zoom]')?.getAttribute('data-zoom');
        if (action) {
            ({ in: () => zoom(1.25), out: () => zoom(0.8), fit, close } as Record<string, () => void>)[action]?.();
            return;
        }
        if (dragged) {
            dragged = false;
            return;
        }
        if (clone.contains(target) && onClick(target)) {
            close();
        }
    });
    stage.addEventListener('dblclick', e => {
        if ((e.target as Element).closest('[data-node-id], [data-edge-id]')) {
            return;
        }
        const r = stage.getBoundingClientRect();
        zoom(e.shiftKey ? 0.5 : 2, e.clientX - r.left, e.clientY - r.top);
    });
    const onKey = (e: KeyboardEvent) => {
        const run = ({ 'Escape': close, '+': () => zoom(1.25), '=': () => zoom(1.25), '-': () => zoom(0.8), '0': fit } as Record<string, () => void>)[e.key];
        if (run && !e.metaKey && !e.ctrlKey && !e.altKey) {
            e.preventDefault();
            e.stopPropagation();
            run();
        }
    };
    document.addEventListener('keydown', onKey, true);
    const onResize = () => fit();
    window.addEventListener('resize', onResize);
    fit();

    const disposable = Disposable.create(() => {
        document.removeEventListener('keydown', onKey, true);
        window.removeEventListener('resize', onResize);
        overlay.remove();
    });
    return disposable;
}

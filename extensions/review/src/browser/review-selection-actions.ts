import { FrontendApplicationContribution } from '@theia/core/lib/browser/frontend-application-contribution';
import { codicon } from '@theia/core/lib/browser/widgets/widget';
import { CommandService } from '@theia/core/lib/common/command';
import { DisposableCollection } from '@theia/core/lib/common/disposable';
import { inject, injectable } from '@theia/core/shared/inversify';
import { EditorManager } from '@theia/editor/lib/browser/editor-manager';
import { EditorWidget } from '@theia/editor/lib/browser/editor-widget';
import { MonacoEditor } from '@theia/monaco/lib/browser/monaco-editor';
import * as monaco from '@theia/monaco-editor-core';
import { ReviewCommands } from './review-commands';
import { ReviewManager } from './review-manager';

/**
 * Floating "Ask Agent · Comment" actions shown next to a non-empty selection,
 * so asking about a few selected lines is one click away. Without an agent, only Comment.
 */
@injectable()
export class ReviewSelectionActions implements FrontendApplicationContribution {

    @inject(EditorManager) protected readonly editorManager: EditorManager;
    @inject(CommandService) protected readonly commands: CommandService;
    @inject(ReviewManager) protected readonly reviews: ReviewManager;

    onStart(): void {
        this.editorManager.all.forEach(widget => this.track(widget));
        this.editorManager.onCreated(widget => this.track(widget));
    }

    protected readonly tracked = new WeakSet<EditorWidget>();

    protected track(widget: EditorWidget): void {
        const editor = MonacoEditor.get(widget);
        // Restored editors are both in `all` and reported by `onCreated`.
        if (!editor || this.tracked.has(widget)) {
            return;
        }
        this.tracked.add(widget);
        const control = editor.getControl();
        const node = this.createNode();
        let position: monaco.editor.IContentWidgetPosition | null = null;
        let visible = false;
        const contentWidget: monaco.editor.IContentWidget = {
            allowEditorOverflow: false,
            getId: () => 'co-review.selection-actions',
            getDomNode: () => node,
            getPosition: () => position
        };
        node.addEventListener('co-review-hide', () => {
            if (visible) {
                control.removeContentWidget(contentWidget);
                visible = false;
            }
        });
        const toDispose = new DisposableCollection();
        let timer: number | undefined;

        const update = () => {
            const selection = control.getSelection();
            const model = control.getModel();
            const show = !!selection && !!model && !selection.isEmpty() && control.hasTextFocus()
                && !!model.getValueInRange(selection).trim();
            if (!show) {
                if (visible) {
                    control.removeContentWidget(contentWidget);
                    visible = false;
                }
                return;
            }
            // Anchor to the top of the selection, just past the end of its first line, preferring above.
            const line = selection!.startLineNumber;
            position = {
                position: { lineNumber: line, column: model!.getLineMaxColumn(line) },
                preference: [monaco.editor.ContentWidgetPositionPreference.ABOVE, monaco.editor.ContentWidgetPositionPreference.BELOW]
            };
            const multiLine = selection!.endLineNumber > selection!.startLineNumber && !(selection!.endColumn === 1 && selection!.endLineNumber === line + 1);
            node.classList.toggle('multi-line', multiLine);
            const [ask, comment] = Array.from(node.children) as HTMLElement[];
            ask.hidden = !this.reviews.hasAgent;
            comment.classList.toggle('primary', !this.reviews.hasAgent);
            if (visible) {
                control.layoutContentWidget(contentWidget);
            } else {
                control.addContentWidget(contentWidget);
                visible = true;
            }
        };
        const schedule = () => {
            window.clearTimeout(timer);
            timer = window.setTimeout(update, 200);
        };
        toDispose.push(control.onDidChangeCursorSelection(schedule));
        toDispose.push(control.onDidFocusEditorText(schedule));
        toDispose.push(control.onDidBlurEditorText(() => {
            // Keep the actions while the user is clicking them.
            window.clearTimeout(timer);
            timer = window.setTimeout(() => node.matches(':hover') || update(), 200);
        }));
        toDispose.push({ dispose: () => { window.clearTimeout(timer); if (visible) { control.removeContentWidget(contentWidget); } } });
        widget.disposed.connect(() => toDispose.dispose());
    }

    protected createNode(): HTMLElement {
        const node = document.createElement('div');
        node.className = 'co-review-selection-actions';
        const button = (label: string, icon: string, commandId: string, primary: boolean) => {
            const el = document.createElement('button');
            el.className = `co-review-selection-action ${primary ? 'primary' : ''}`;
            el.title = label;
            el.innerHTML = `<span class="${codicon(icon)}"></span><span>${label}</span>`;
            // Keep the editor selection: act on mousedown and prevent the focus change.
            el.addEventListener('mousedown', e => {
                e.preventDefault();
                e.stopPropagation();
                node.dispatchEvent(new Event('co-review-hide'));
                this.commands.executeCommand(commandId);
            });
            return el;
        };
        node.append(
            button('Ask Agent', 'hubot', ReviewCommands.ASK_SELECTION.id, true),
            button('Comment', 'comment', ReviewCommands.COMMENT_SELECTION.id, false)
        );
        return node;
    }
}

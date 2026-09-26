import { AbstractViewContribution, OpenViewArguments } from '@theia/core/lib/browser/shell/view-contribution';
import { StatusBar, StatusBarAlignment } from '@theia/core/lib/browser/status-bar/status-bar';
import { FrontendApplicationContribution } from '@theia/core/lib/browser/frontend-application-contribution';
import { KeybindingRegistry } from '@theia/core/lib/browser/keybinding';
import { QuickInputService, QuickPickItem } from '@theia/core/lib/browser/quick-input';
import { ConfirmDialog } from '@theia/core/lib/browser/dialogs';
import { CommandRegistry, CommandService } from '@theia/core/lib/common/command';
import { MenuModelRegistry } from '@theia/core/lib/common/menu';
import { MessageService } from '@theia/core/lib/common/message-service';
import { ClipboardService } from '@theia/core/lib/browser/clipboard-service';
import { SelectionService } from '@theia/core/lib/common/selection-service';
import { UriAwareCommandHandler } from '@theia/core/lib/common/uri-command-handler';
import URI from '@theia/core/lib/common/uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { EDITOR_CONTEXT_MENU } from '@theia/editor/lib/browser/editor-menu';
import { EditorManager } from '@theia/editor/lib/browser/editor-manager';
import { FileService } from '@theia/filesystem/lib/browser/file-service';
import { MonacoEditor } from '@theia/monaco/lib/browser/monaco-editor';
import { NavigatorContextMenu } from '@theia/navigator/lib/browser/navigator-contribution';
import * as monaco from '@theia/monaco-editor-core';
import { AgentConfig, AgentSetting, CodeLocation, ReviewScope, ThreadIntent } from '../common/review-model';
import { DOCUMENT_NAMES } from '../common/patch';
import { OpenerService, open } from '@theia/core/lib/browser/opener-service';
import { ReviewService } from '../common/review-protocol';
import { ReviewCommands } from './review-commands';
import { fromMonacoRange, ReviewLocations } from './review-locations';
import { ReviewManager } from './review-manager';
import { ReviewWidget } from './review-widget';
import { ReviewEditorDecorator } from './review-editor-decorator';
import { ReviewNavigator } from './review-navigator';

export const REVIEW_CONTEXT_MENU_GROUP = [...EDITOR_CONTEXT_MENU, '0_co_review'];
export const REVIEW_NAVIGATOR_GROUP = [...NavigatorContextMenu.NAVIGATION, '0_co_review'];

@injectable()
export class ReviewContribution extends AbstractViewContribution<ReviewWidget> implements FrontendApplicationContribution {

    @inject(ReviewManager) protected readonly reviews: ReviewManager;
    @inject(ReviewService) protected readonly service: ReviewService;
    @inject(EditorManager) protected readonly editorManager: EditorManager;
    @inject(QuickInputService) protected readonly quickInput: QuickInputService;
    @inject(MessageService) protected readonly messages: MessageService;
    @inject(SelectionService) protected readonly selectionService: SelectionService;
    @inject(FileService) protected readonly fileService: FileService;
    @inject(ReviewLocations) protected readonly locations: ReviewLocations;
    @inject(StatusBar) protected readonly statusBar: StatusBar;
    @inject(CommandService) protected readonly commandService: CommandService;
    @inject(OpenerService) protected readonly openerService: OpenerService;
    @inject(ReviewEditorDecorator) protected readonly decorator: ReviewEditorDecorator;
    @inject(ReviewNavigator) protected readonly navigator: ReviewNavigator;
    @inject(ClipboardService) protected readonly clipboard: ClipboardService;

    constructor() {
        super({
            widgetId: ReviewWidget.ID,
            widgetName: ReviewWidget.LABEL,
            defaultWidgetOptions: { area: 'right', rank: 100 },
            toggleCommandId: ReviewCommands.TOGGLE_PANEL.id,
            toggleKeybinding: 'ctrlcmd+shift+alt+r'
        });
    }

    /** The panel opens on demand; a status bar entry shows the open threads and toggles it. */
    onStart(): void {
        const update = () => {
            const review = this.reviews.activeReview;
            const open = review?.threads.filter(t => t.status !== 'resolved').length ?? 0;
            this.statusBar.setElement('co-review', {
                text: `$(comment-discussion) ${review ? open : 'Review'}`,
                tooltip: review ? `${review.title}: ${open} open thread${open === 1 ? '' : 's'} — show/hide the Review panel` : 'Start a review',
                alignment: StatusBarAlignment.LEFT,
                priority: 100,
                command: ReviewCommands.TOGGLE_PANEL.id
            });
        };
        this.reviews.onDidChange(update);
        update();
        // Whether the file in the editor is viewed; click to toggle.
        const viewed = () => {
            const uri = this.editorManager.currentEditor?.editor.uri.toString();
            const review = this.reviews.activeReview;
            if (!uri || !review || review.bundle || !uri.startsWith('file:')) {
                this.statusBar.removeElement('co-review-viewed');
                return;
            }
            const done = this.reviews.isViewed(uri);
            this.statusBar.setElement('co-review-viewed', {
                text: done ? '$(pass-filled) Viewed' : '$(circle-large-outline) Not viewed',
                tooltip: done ? 'You marked this file as viewed; click to unmark' : 'Mark this file as viewed (⌘⌥V)',
                alignment: StatusBarAlignment.LEFT,
                priority: 99,
                command: ReviewCommands.TOGGLE_VIEWED.id
            });
        };
        this.reviews.onDidChange(viewed);
        this.editorManager.onCurrentEditorChanged(viewed);
        viewed();
    }

    /** Writes the repository overview (from Graphify's graph when there is one) and opens it as a rendered page. */
    protected async openOverview(): Promise<void> {
        const uri = await this.reviews.writeOverview();
        if (uri) {
            await this.navigator.open({ kind: 'document', uri });
        }
    }

    /** Toggles "viewed" for a file or folder (all of its files), or the file in the editor. */
    protected async toggleViewed(uri: URI | undefined): Promise<void> {
        const target = uri ?? this.editorManager.currentEditor?.editor.uri;
        if (!target) {
            return;
        }
        const stat = await this.fileService.resolve(target).catch(() => undefined);
        if (stat?.isDirectory) {
            const coverage = await this.reviews.getCoverage();
            const folder = this.reviews.relativePath(target.toString());
            const area = coverage?.areas.find(a => a.path === folder || folder.startsWith(`${a.path}/`) || a.path.startsWith(`${folder}/`));
            this.messages.info(area ? `${area.viewed} of ${area.total} files viewed in ${area.path}.` : 'Mark files one by one: open each and press ⌘⌥V.');
            return;
        }
        await this.reviews.setViewed([target.toString()], !this.reviews.isViewed(target.toString()));
    }

    /** After the layout is restored (opening earlier would be undone by the restore). */
    async onDidInitializeLayout(): Promise<void> {
        await this.openBundleDocument();
        await this.welcome();
    }

    /** The first time Co-Review runs, the Review panel opens, so a new user sees where to start. */
    protected async welcome(): Promise<void> {
        const key = 'co-review.welcomed';
        try {
            if (localStorage.getItem(key)) {
                return;
            }
            localStorage.setItem(key, '1');
        } catch {
            return;
        }
        await this.reviews.ready;
        if (!this.reviews.activeReview) {
            await this.openView({ reveal: true });
        }
    }

    /** A review directory opens on its document, or its first patch. */
    protected async openBundleDocument(): Promise<void> {
        await this.reviews.ready;
        const review = this.reviews.activeReview;
        const root = this.reviews.root;
        if (!review?.bundle || !root || this.editorManager.all.length) {
            return;
        }
        const rootUri = new URI(root);
        let doc: string | undefined;
        for (const name of DOCUMENT_NAMES) {
            if (await this.fileService.exists(rootUri.resolve(name))) {
                doc = name;
                break;
            }
        }
        if (!doc) {
            const children = (await this.fileService.resolve(rootUri, { resolveMetadata: false }).catch(() => undefined))?.children ?? [];
            const names = children.map(c => c.name).sort();
            doc = names.find(n => n.endsWith('.pseudocode.md')) ?? names.find(n => /\.(patch|diff)$/.test(n));
        }
        if (doc) {
            await open(this.openerService, new URI(root).resolve(doc));
        }
    }

    override async openView(args?: Partial<OpenViewArguments>): Promise<ReviewWidget> {
        const widget = await super.openView(args);
        // First time on the right: give the panel a readable width.
        if (widget.isVisible && widget.node.clientWidth > 0 && widget.node.clientWidth < 320) {
            this.shell.resize(Math.max(360, Math.round(window.innerWidth * 0.28)), 'right');
        }
        return widget;
    }

    override registerCommands(registry: CommandRegistry): void {
        super.registerCommands(registry);
        registry.registerCommand(ReviewCommands.CREATE_REVIEW, { execute: () => this.createReview() });
        registry.registerCommand(ReviewCommands.RENAME_REVIEW, {
            isEnabled: () => !!this.reviews.activeReview,
            execute: () => this.renameReview()
        });
        registry.registerCommand(ReviewCommands.DELETE_REVIEW, {
            isEnabled: () => !!this.reviews.activeReview,
            execute: () => this.deleteReview()
        });
        registry.registerCommand(ReviewCommands.COMMENT_SELECTION, {
            isEnabled: () => !!this.currentEditor(),
            execute: () => this.commentOnEditor(false)
        });
        registry.registerCommand(ReviewCommands.ASK_SELECTION, {
            isEnabled: () => !!this.currentEditor() && this.reviews.hasAgent,
            isVisible: () => this.reviews.hasAgent,
            execute: () => this.commentOnEditor(false, 'question')
        });
        registry.registerCommand(ReviewCommands.COMMENT_SYMBOL, {
            isEnabled: () => !!this.currentEditor(),
            execute: () => this.commentOnEditor(true)
        });
        registry.registerCommand(ReviewCommands.COLLAPSE_ALL, { isEnabled: () => !!this.reviews.activeReview, execute: () => this.decorator.collapseAll() });
        registry.registerCommand(ReviewCommands.EXPAND_ALL, { isEnabled: () => !!this.reviews.activeReview, execute: () => this.decorator.expandAll() });
        registry.registerCommand(ReviewCommands.NEXT_COMMENT, { isEnabled: () => !!this.currentEditor(), execute: () => this.decorator.goToComment(1) });
        registry.registerCommand(ReviewCommands.PREVIOUS_COMMENT, { isEnabled: () => !!this.currentEditor(), execute: () => this.decorator.goToComment(-1) });
        registry.registerCommand(ReviewCommands.GO_TO_COMMENT, { isEnabled: () => !!this.reviews.activeReview, execute: () => this.goToComment() });
        registry.registerCommand(ReviewCommands.INSTALL_SKILLS, { execute: () => this.installSkills() });
        registry.registerCommand(ReviewCommands.SETUP_HARNESS, { execute: () => this.setupHarness() });
        registry.registerCommand(ReviewCommands.INSTALL_CLI, { execute: () => this.installCli() });
        registry.registerCommand(ReviewCommands.REVIEW_ACTIONS, { isEnabled: () => !!this.reviews.activeReview, execute: () => this.reviewActions() });
        registry.registerCommand(ReviewCommands.CONFIGURE_AGENT, {
            isEnabled: () => !!this.reviews.activeReview,
            execute: () => this.configureAgent()
        });
        // The file in the editor (shortcut, status bar, editor menu); the explorer's selection has its own command.
        registry.registerCommand(ReviewCommands.TOGGLE_VIEWED, {
            isEnabled: () => !!this.reviews.activeReview && !!this.editorManager.currentEditor,
            execute: () => this.toggleViewed(undefined)
        });
        registry.registerCommand(ReviewCommands.TOGGLE_VIEWED_PATH, UriAwareCommandHandler.MonoSelect(this.selectionService, {
            isEnabled: () => !!this.reviews.activeReview,
            execute: uri => this.toggleViewed(uri)
        }));
        registry.registerCommand(ReviewCommands.OPEN_OVERVIEW, {
            isEnabled: () => !!this.reviews.activeReview && !this.reviews.activeReview.bundle,
            execute: () => this.openOverview()
        });
        registry.registerCommand(ReviewCommands.AGENT_SETTINGS, {
            isEnabled: () => AgentConfig.isAcp(this.reviews.activeReview?.agent),
            execute: () => this.chooseAgentSettings()
        });
        registry.registerCommand(ReviewCommands.COMMENT_REPOSITORY, { execute: () => this.startDraft({ kind: 'repository' }) });
        registry.registerCommand(ReviewCommands.COMMENT_PATH, UriAwareCommandHandler.MonoSelect(this.selectionService, {
            execute: uri => this.commentOnPath(uri)
        }));
        registry.registerCommand(ReviewCommands.REVIEW_PATHS, UriAwareCommandHandler.MultiSelect(this.selectionService, {
            execute: uris => this.createReview({ kind: 'paths', uris: uris.map(u => u.toString()) })
        }));
    }

    override registerMenus(menus: MenuModelRegistry): void {
        super.registerMenus(menus);
        menus.registerMenuAction(REVIEW_CONTEXT_MENU_GROUP, { commandId: ReviewCommands.COMMENT_SELECTION.id, order: 'a' });
        menus.registerMenuAction(REVIEW_CONTEXT_MENU_GROUP, { commandId: ReviewCommands.COMMENT_SYMBOL.id, order: 'b' });
        menus.registerMenuAction(REVIEW_NAVIGATOR_GROUP, { commandId: ReviewCommands.COMMENT_PATH.id, order: 'a' });
        menus.registerMenuAction(REVIEW_NAVIGATOR_GROUP, { commandId: ReviewCommands.REVIEW_PATHS.id, order: 'b' });
        menus.registerMenuAction(REVIEW_NAVIGATOR_GROUP, { commandId: ReviewCommands.TOGGLE_VIEWED_PATH.id, order: 'c' });
        menus.registerMenuAction(REVIEW_CONTEXT_MENU_GROUP, { commandId: ReviewCommands.TOGGLE_VIEWED.id, label: 'Mark File as Viewed / Not Viewed', order: 'c' });
    }

    override registerKeybindings(keybindings: KeybindingRegistry): void {
        super.registerKeybindings(keybindings);
        keybindings.registerKeybinding({ command: ReviewCommands.COMMENT_SELECTION.id, keybinding: 'ctrlcmd+alt+m', when: 'editorTextFocus' });
        keybindings.registerKeybinding({ command: ReviewCommands.ASK_SELECTION.id, keybinding: 'ctrlcmd+alt+a', when: 'editorTextFocus' });
        keybindings.registerKeybinding({ command: ReviewCommands.NEXT_COMMENT.id, keybinding: 'ctrlcmd+alt+down', when: 'editorTextFocus' });
        keybindings.registerKeybinding({ command: ReviewCommands.PREVIOUS_COMMENT.id, keybinding: 'ctrlcmd+alt+up', when: 'editorTextFocus' });
        keybindings.registerKeybinding({ command: ReviewCommands.GO_TO_COMMENT.id, keybinding: 'ctrlcmd+alt+o' });
        keybindings.registerKeybinding({ command: ReviewCommands.TOGGLE_VIEWED.id, keybinding: 'ctrlcmd+alt+v', when: 'editorTextFocus' });
    }

    protected currentEditor(): MonacoEditor | undefined {
        return MonacoEditor.get(this.editorManager.currentEditor);
    }

    protected async ensureReview(): Promise<boolean> {
        await this.reviews.ready;
        if (this.reviews.activeReview) {
            return true;
        }
        return !!await this.createReview();
    }

    /** Drafts with a range are edited inline in the editor; others in the review panel. */
    protected async startDraft(location: CodeLocation, defaultIntent: ThreadIntent = 'comment'): Promise<void> {
        if (!await this.ensureReview()) {
            return;
        }
        if (!location.range) {
            await this.openView({ activate: true, reveal: true });
        }
        this.reviews.addDraft(location, defaultIntent);
    }

    /**
     * @param intent explicit intent; when omitted, a multi-line selection defaults to asking the agent.
     */
    protected async commentOnEditor(onSymbol: boolean, intent?: ThreadIntent): Promise<void> {
        const editor = this.currentEditor();
        const model = editor?.getControl().getModel();
        if (!editor || !model) {
            return;
        }
        const selection = editor.getControl().getSelection() ?? new monaco.Selection(1, 1, 1, 1);
        const symbol = await this.locations.findEnclosingSymbol(model, selection.startLineNumber);
        const uri = editor.uri.toString();

        if (onSymbol) {
            if (!symbol) {
                this.messages.warn('No symbol found at the cursor. Language support may still be starting.');
                return;
            }
            const declaration = new monaco.Range(symbol.selectionRange.startLineNumber, 1,
                symbol.selectionRange.startLineNumber, model.getLineMaxColumn(symbol.selectionRange.startLineNumber));
            return this.startDraft({
                kind: 'symbol', uri, symbol: symbol.path,
                range: fromMonacoRange(symbol.range),
                anchor: await this.locations.captureAnchor(model, declaration)
            }, intent);
        }

        let range: monaco.IRange;
        let kind: CodeLocation['kind'];
        if (selection.isEmpty()) {
            const line = selection.startLineNumber;
            range = new monaco.Range(line, 1, line, model.getLineMaxColumn(line));
            kind = 'line';
        } else {
            // A selection ending at column 1 of the next line means "up to the end of the previous line".
            const endLine = selection.endColumn === 1 && selection.endLineNumber > selection.startLineNumber ? selection.endLineNumber - 1 : selection.endLineNumber;
            const endColumn = endLine === selection.endLineNumber ? selection.endColumn : model.getLineMaxColumn(endLine);
            range = new monaco.Range(selection.startLineNumber, selection.startColumn, endLine, endColumn);
            kind = 'range';
        }
        const defaultIntent = intent ?? (range.endLineNumber > range.startLineNumber ? 'question' : 'comment');
        await this.startDraft({ kind, uri, range: fromMonacoRange(range), symbol: symbol?.path, anchor: await this.locations.captureAnchor(model, range) }, defaultIntent);
    }

    protected async commentOnPath(uri: URI): Promise<void> {
        const stat = await this.fileService.resolve(uri).catch(() => undefined);
        if (!stat) {
            return;
        }
        const root = this.reviews.root;
        if (root && uri.toString() === root) {
            return this.startDraft({ kind: 'repository' });
        }
        await this.startDraft({ kind: stat.isDirectory ? 'directory' : 'file', uri: uri.toString() });
    }

    protected async createReview(presetScope?: ReviewScope): Promise<boolean> {
        await this.reviews.ready;
        const root = this.reviews.root;
        if (!root) {
            this.messages.warn('Open a repository folder before starting a review.');
            return false;
        }
        const scope = presetScope ?? await this.pickScope(root);
        if (!scope) {
            return false;
        }
        const defaultTitle = scope.kind === 'repository' ? `Review of ${new URI(root).path.base}` : ReviewScope.label(scope, u => this.reviews.relativePath(u));
        const title = await this.quickInput.input({ prompt: 'Review title', value: defaultTitle });
        if (title === undefined) {
            return false;
        }
        await this.reviews.createReview(title.trim() || defaultTitle, scope);
        return true;
    }

    protected async pickScope(root: string): Promise<ReviewScope | undefined> {
        type ScopeItem = QuickPickItem & { kind: ReviewScope['kind'] };
        const git = await this.service.getGitInfo(root);
        const editorUri = this.editorManager.currentEditor?.editor.uri;
        const items: ScopeItem[] = [
            { kind: 'repository', label: '$(repo) Entire repository', description: 'Review everything, independent of Git' },
            ...(editorUri ? [{ kind: 'paths' as const, label: '$(file) Current file', description: this.reviews.relativePath(editorUri.toString()) }] : []),
            ...(git.isRepository ? [
                { kind: 'branch' as const, label: '$(git-compare) Branch', description: 'Compare a branch against a base' },
                { kind: 'commit' as const, label: '$(git-commit) Commit', description: 'Review a single commit' }
            ] : [])
        ];
        const picked = await this.quickInput.pick(items, { placeHolder: 'What do you want to review?' });
        if (!picked) {
            return undefined;
        }
        switch (picked.kind) {
            case 'repository': return { kind: 'repository' };
            case 'paths': return { kind: 'paths', uris: [editorUri!.toString()] };
            case 'branch': {
                const branchItems = git.branches.map(b => ({ label: b }));
                const head = await this.quickInput.pick(branchItems, { placeHolder: `Branch to review (current: ${git.currentBranch})` });
                if (!head) {
                    return undefined;
                }
                const defaultBase = git.branches.find(b => b === 'main') ?? git.branches.find(b => b === 'master');
                const base = await this.quickInput.pick(
                    branchItems.filter(b => b.label !== head.label).sort((a, b) => (b.label === defaultBase ? 1 : 0) - (a.label === defaultBase ? 1 : 0)),
                    { placeHolder: `Base to compare ${head.label} against` });
                return base ? { kind: 'branch', base: base.label, head: head.label } : undefined;
            }
            case 'commit': {
                const ref = await this.quickInput.input({ prompt: 'Commit to review (SHA or ref)', value: 'HEAD' });
                if (!ref) {
                    return undefined;
                }
                const sha = await this.service.resolveCommit(root, ref.trim());
                if (!sha) {
                    this.messages.error(`'${ref}' is not a commit in this repository.`);
                    return undefined;
                }
                return { kind: 'commit', sha };
            }
        }
        return undefined;
    }

    /** Every open thread of the review, searchable; picking one opens it in the editor. */
    protected async goToComment(): Promise<void> {
        const review = this.reviews.activeReview;
        if (!review) {
            return;
        }
        const threads = review.threads.filter(t => t.status === 'open')
            .sort((a, b) => this.reviews.locationLabel(a.location).localeCompare(this.reviews.locationLabel(b.location)));
        const picked = await this.quickInput.pick(threads.map(thread => ({
            thread,
            label: `#${thread.number} ${thread.messages[0]?.body.split('\n')[0].slice(0, 80) ?? ''}`,
            description: this.reviews.locationLabel(thread.location),
            detail: `${thread.messages.length} message${thread.messages.length === 1 ? '' : 's'} · ${thread.messages[thread.messages.length - 1]?.author.name ?? ''}`
        })), { placeHolder: `Open threads in "${review.title}"`, matchOnDescription: true, matchOnDetail: true });
        if (picked) {
            this.reviews.setCollapsed(picked.thread.id, false);
            await this.navigator.open(picked.thread.location);
            this.reviews.revealThread(picked.thread.id);
        }
    }

    /** The panel's ⋯ menu. */
    protected async reviewActions(): Promise<void> {
        const review = this.reviews.activeReview;
        if (!review) {
            return;
        }
        const actions = [
            ...review.bundle ? [] : [{ label: '$(map) Repository overview', id: ReviewCommands.OPEN_OVERVIEW.id }],
            { label: '$(list-selection) Go to comment…', id: ReviewCommands.GO_TO_COMMENT.id },
            { label: '$(fold) Collapse all comments', id: ReviewCommands.COLLAPSE_ALL.id },
            { label: '$(unfold) Expand all comments', id: ReviewCommands.EXPAND_ALL.id },
            { label: '$(comment) Comment on the repository', id: ReviewCommands.COMMENT_REPOSITORY.id },
            { label: `$(hubot) ${review.agent ? 'Change agent…' : 'Connect agent…'}`, id: ReviewCommands.CONFIGURE_AGENT.id },
            ...AgentConfig.isAcp(review.agent) ? [{ label: '$(settings-gear) Choose agent model…', id: ReviewCommands.AGENT_SETTINGS.id }] : [],
            { label: '$(device-mobile) Open mobile view', id: 'co-review.mobile' },
            { label: '$(plug) Add Co-Review to an agent harness (MCP)…', id: ReviewCommands.SETUP_HARNESS.id },
            { label: '$(book) Install agent skills…', id: ReviewCommands.INSTALL_SKILLS.id },
            { label: '$(edit) Rename review…', id: ReviewCommands.RENAME_REVIEW.id },
            { label: '$(trash) Delete review…', id: ReviewCommands.DELETE_REVIEW.id }
        ];
        const picked = await this.quickInput.pick(actions, { placeHolder: review.title });
        if (picked?.id === 'co-review.mobile') {
            // The phone view of this review (install it from the phone's browser; see docs for Tailscale).
            window.open(`${window.location.origin}/m/?review=${review.id}`, '_blank', 'noopener');
            return;
        }
        if (picked) {
            await this.commandService.executeCommand(picked.id);
        }
    }

    /** Copies the co-review skills into an agent's skills directory. */
    protected async installSkills(): Promise<void> {
        const setup = await this.service.getAgentSetup(this.reviews.workspaceRoot);
        if (!setup.skillsAvailable) {
            this.messages.warn('This build does not include the agent skills.');
            return;
        }
        const target = await this.quickInput.pick(setup.skills.map(t => ({ label: t.label, description: t.dir, id: t.id })),
            { placeHolder: 'Install the co-review and co-review-design skills into…' });
        if (!target) {
            return;
        }
        try {
            const installed = await this.service.installSkills(target.id, this.reviews.workspaceRoot);
            this.messages.info(`Installed ${installed.map(p => p.split('/').pop()).join(', ')} into ${target.description}. Agents pick them up in new sessions.`);
        } catch (e) {
            this.messages.error(`Could not install the skills: ${e instanceof Error ? e.message : e}`);
        }
    }

    /** Adds Co-Review's MCP server to an agent harness: writes its config where that is safe, otherwise copies the snippet. */
    protected async setupHarness(): Promise<void> {
        const setup = await this.service.getAgentSetup(this.reviews.workspaceRoot);
        const picked = await this.quickInput.pick(setup.harnesses.map(h => ({
            label: h.label, id: h.id,
            description: h.kind === 'cli' ? h.snippet.split('\n').map(c => c.split(' ').slice(0, 3).join(' ')).join(' · ') : h.file,
            detail: h.kind === 'manual' ? 'Copies the config to paste in' : undefined
        })), { placeHolder: 'Add Co-Review\'s MCP server to…' });
        const harness = setup.harnesses.find(h => h.id === picked?.id);
        if (!harness) {
            return;
        }
        const copy = async (why?: string) => {
            await this.clipboard.writeText(harness.snippet);
            this.messages.info(`${why ? `${why} ` : ''}Copied the ${harness.label} config${harness.file ? `; paste it into ${harness.file}` : ''}.`);
        };
        if (harness.kind === 'manual') {
            return copy();
        }
        // The plugin's MCP server and the Pi extension launch `co-review` from PATH.
        if (!setup.cliInstalled && (harness.id === 'claude-code-plugin' || harness.id === 'pi')) {
            const install = await this.messages.info(`${harness.label} needs the co-review command on your PATH. Install it now?`, 'Install', 'Skip');
            if (install === 'Install' && !await this.installCli()) {
                return;
            }
        }
        const where = harness.kind === 'cli' ? `by running: ${harness.snippet}` : `to ${harness.file}`;
        const choice = await this.messages.info(`Add Co-Review to ${harness.label} ${where}?`, 'Add', 'Copy config');
        if (choice === 'Copy config') {
            return copy();
        }
        if (choice === 'Add') {
            try {
                this.messages.info(`${await this.service.applyHarnessSetup(harness.id)} Restart ${harness.label} to load it.`);
            } catch (e) {
                await copy(`${e instanceof Error ? e.message : e}.`);
            }
        }
    }

    /** Installs the `co-review` command; resolves to whether it worked. */
    protected async installCli(): Promise<boolean> {
        try {
            this.messages.info(await this.service.installCli());
            return true;
        } catch (e) {
            this.messages.error(`Could not install the co-review command: ${e instanceof Error ? e.message : e}`);
            return false;
        }
    }

    /** Picks the ACP agent for the active review: a detected preset, a custom command, or none. */
    protected async configureAgent(): Promise<void> {
        await this.reviews.ready;
        const review = this.reviews.activeReview;
        if (!review) {
            return;
        }
        type AgentItem = QuickPickItem & { agent?: AgentConfig; custom?: boolean; none?: boolean };
        const presets = await this.reviews.getAgentPresets();
        const items: AgentItem[] = [
            ...presets.map(agent => ({
                agent, label: `$(hubot) ${agent.name}`, description: AgentConfig.commandLine(agent),
                detail: review.agent?.id === agent.id ? 'Connected' : undefined
            })),
            { custom: true, label: '$(terminal) Custom ACP command…', description: 'Any agent that speaks ACP over stdio' },
            ...(review.agent ? [{ none: true, label: '$(debug-disconnect) Disconnect agent' }] : [])
        ];
        const picked = await this.quickInput.pick(items, { placeHolder: 'Agent for this review (Agent Client Protocol)' });
        if (!picked) {
            return;
        }
        if (picked.none) {
            return this.reviews.setAgent(undefined);
        }
        if (picked.agent) {
            // Keep the model choice when reconnecting the same agent.
            await this.reviews.setAgent(review.agent?.id === picked.agent.id ? { ...picked.agent, settings: review.agent.settings } : picked.agent);
            return this.chooseAgentSettings(false);
        }
        const commandLine = await this.quickInput.input({
            prompt: 'Command that starts an ACP agent on stdio (run in the repository root)',
            placeHolder: 'e.g. npx -y @agentclientprotocol/claude-agent-acp',
            value: review.agent?.id === 'custom' ? AgentConfig.commandLine(review.agent) : ''
        });
        const parts = commandLine?.trim().match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g)?.map(p => p.replace(/^["']|["']$/g, ''));
        if (!parts?.length) {
            return;
        }
        const name = await this.quickInput.input({ prompt: 'Display name for this agent', value: parts[0].split('/').pop() });
        await this.reviews.setAgent({ id: 'custom', name: name?.trim() || parts[0], transport: 'acp', command: parts[0], args: parts.slice(1) });
        await this.chooseAgentSettings(false);
    }

    /**
     * Lets the reviewer pick what the agent offers for its sessions over ACP: the model, then reasoning effort.
     * Modes are left alone: some grant permissions (bypassPermissions). Escape keeps the agent's default. `explicit`: asked for, so say when there is nothing to pick.
     */
    protected async chooseAgentSettings(explicit = true): Promise<void> {
        const agent = this.reviews.activeReview?.agent;
        if (!AgentConfig.isAcp(agent)) {
            return;
        }
        const progress = await this.messages.showProgress({ text: `Asking ${agent.name} which models it offers…` });
        let settings: AgentSetting[];
        try {
            settings = await this.reviews.getAgentSettings();
        } catch (e) {
            this.messages.error(`Could not start ${agent.name}: ${e instanceof Error ? e.message : e}`);
            return;
        } finally {
            progress.cancel();
        }
        const order = ['model', 'thought_level'];
        const offered = settings.filter(s => order.includes(s.category ?? '') && s.options.length > 1)
            .sort((a, b) => order.indexOf(a.category!) - order.indexOf(b.category!));
        if (!offered.length) {
            if (explicit) {
                this.messages.info(`${agent.name} doesn't offer a choice of model over ACP; it uses its own default.`);
            }
            return;
        }
        const chosen: Record<string, string> = { ...agent.settings };
        for (const setting of offered) {
            const current = chosen[setting.id] ?? setting.current;
            // The current choice first, then the agent's order.
            const options = [...setting.options].sort((a, b) => Number(b.value === current) - Number(a.value === current));
            const picked = await this.quickInput.pick(options.map(o => ({
                label: `${o.value === current ? '$(check) ' : ''}${o.name}`, description: o.description ?? (o.name !== o.value ? o.value : undefined), value: o.value
            })), { placeHolder: `${agent.name}: ${setting.name} (Escape keeps ${setting.options.find(o => o.value === current)?.name ?? current})` });
            if (!picked) {
                break;
            }
            chosen[setting.id] = picked.value;
        }
        if (JSON.stringify(chosen) !== JSON.stringify(agent.settings ?? {})) {
            const settingsLabel = offered.filter(s => chosen[s.id] !== undefined)
                .map(s => s.options.find(o => o.value === chosen[s.id])?.name ?? chosen[s.id]).join(' · ');
            await this.reviews.setAgent({ ...agent, settings: chosen, settingsLabel });
        }
    }

    protected async renameReview(): Promise<void> {
        const review = this.reviews.activeReview;
        if (!review) {
            return;
        }
        const title = await this.quickInput.input({ prompt: 'Review title', value: review.title });
        if (title?.trim()) {
            await this.reviews.renameReview(review.id, title.trim());
        }
    }

    protected async deleteReview(): Promise<void> {
        const review = this.reviews.activeReview;
        if (!review) {
            return;
        }
        const confirmed = await new ConfirmDialog({
            title: 'Delete Review',
            msg: `Delete "${review.title}" and its ${review.threads.length} thread(s)? This cannot be undone.`,
            ok: 'Delete'
        }).open();
        if (confirmed) {
            await this.reviews.deleteReview(review.id);
        }
    }
}

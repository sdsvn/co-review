import { ContainerModule, injectable } from '@theia/core/shared/inversify';
import { app, BrowserWindow } from '@theia/core/electron-shared/electron';
import { ElectronMainApplication, ElectronMainCommandOptions } from '@theia/core/lib/electron-main/electron-main-application';

// The backend opens a window by launching the app again with a repository (the running app gets it as a second
// instance): it needs the app's command line, as its own executable is Electron's helper. Unpackaged, that is
// Electron plus the app's path; a custom user data directory must be passed on too, so the launch reaches this
// instance. The backend process inherits this environment.
process.env.CO_REVIEW_APP_LAUNCH = JSON.stringify([
    process.execPath,
    ...app.isPackaged ? [] : [app.getAppPath()],
    ...process.argv.filter(arg => arg.startsWith('--user-data-dir='))
]);

/**
 * The desktop app's windows:
 * - `--background` (how `co-review mcp` starts it for an agent): no window until a review is shown or the
 *   reviewer opens the app, so starting an agent session does not put a window in front of them;
 * - a repository that already has a window is focused rather than opened a second time;
 * - activating the app (Dock, Finder) with no window open opens one.
 */
@injectable()
export class ReviewElectronMainApplication extends ElectronMainApplication {

    protected readonly background = process.argv.includes('--background');

    protected override showInitialWindow(urlToOpen: string | undefined): void {
        if (!this.background) {
            super.showInitialWindow(urlToOpen);
        }
    }

    protected override async handleMainCommand(options: ElectronMainCommandOptions): Promise<void> {
        if (this.background && !options.secondInstance) {
            return;
        }
        return super.handleMainCommand(options);
    }

    protected override hookApplicationEvents(): void {
        super.hookApplicationEvents();
        app.on('activate', (_event, hasVisibleWindows) => {
            if (!hasVisibleWindows && !this.windows.size) {
                this.openWindowWithWorkspace('');
            }
        });
    }

    protected override async openWindowWithWorkspace(workspacePath: string): Promise<BrowserWindow> {
        const existing = workspacePath ? this.windowOf(workspacePath) : undefined;
        if (existing) {
            if (existing.isMinimized()) {
                existing.restore();
            }
            existing.show();
            existing.focus();
            return existing;
        }
        return super.openWindowWithWorkspace(workspacePath);
    }

    /** The window showing `workspacePath` (a window's URL fragment is its workspace). */
    protected windowOf(workspacePath: string): BrowserWindow | undefined {
        for (const wrapper of this.windows.values()) {
            const window = wrapper.window;
            try {
                if (!window.isDestroyed() && decodeURI(new URL(window.webContents.getURL()).hash.slice(1)) === workspacePath) {
                    return window;
                }
            } catch {
                /* not loaded yet */
            }
        }
        return undefined;
    }
}

export default new ContainerModule((_bind, _unbind, _isBound, rebind) => {
    rebind(ElectronMainApplication).to(ReviewElectronMainApplication).inSingletonScope();
});

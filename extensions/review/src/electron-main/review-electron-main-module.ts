import { ContainerModule, injectable } from '@theia/core/shared/inversify';
import { app, BrowserWindow, powerMonitor } from '@theia/core/electron-shared/electron';
import { ElectronMainApplication, ElectronMainCommandOptions } from '@theia/core/lib/electron-main/electron-main-application';
import * as fs from 'fs';
import * as path from 'path';
import { captureOutput, DebugLog, logsDir, rateLimit, watchEventLoop } from '../node/debug-log';

// The desktop app's own log (`co-review logs`): started detached, nobody sees its output otherwise.
const log = new DebugLog('main');
captureOutput(log);
watchEventLoop(log);
log.info(`Co-Review ${app.getVersion()} started: ${process.argv.slice(1).join(' ')}`);
try {
    fs.writeFileSync(path.join(logsDir(), 'app.json'), JSON.stringify({ pid: process.pid, version: app.getVersion(), started: new Date().toISOString() }));
} catch {
    /* no logs dir */
}

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
        // What a window does, in the log: a window that hangs or whose page crashes leaves a trace to go by.
        app.on('browser-window-created', (_event, window) => {
            const name = () => {
                try {
                    return decodeURI(new URL(window.webContents.getURL()).hash.slice(1)) || 'a window';
                } catch {
                    return 'a window';
                }
            };
            window.on('unresponsive', () => log.warn(`window of ${name()} is not responding`));
            window.on('responsive', () => log.info(`window of ${name()} responds again`));
            window.on('closed', () => log.info('a window closed'));
            // A page that crashed is loaded again (the review is saved; left as it is, the window stays blank for
            // hours). One that crashes right again stays, to be looked at: reloading it would just loop.
            let reloaded = 0;
            window.webContents.on('render-process-gone', (_e, details) => {
                log.error(`window of ${name()}: its page is gone (${details.reason}, exit code ${details.exitCode})`);
                if (details.reason !== 'clean-exit' && !window.isDestroyed() && Date.now() - reloaded > 60_000) {
                    reloaded = Date.now();
                    log.info(`window of ${name()}: loading its page again`);
                    window.webContents.reload();
                }
            });
            window.webContents.on('did-fail-load', (_e, code, description, url) => log.error(`window failed to load ${url}: ${description} (${code})`));
            // A window's console warnings and errors, at most 20 a second: one caught in a loop can't flood the log.
            const console = rateLimit(log);
            window.webContents.on('console-message', event => {
                if (event.level === 'warning' || event.level === 'error') {
                    console(event.level === 'error' ? 'error' : 'warn', `window of ${name()}: ${event.message} (${event.sourceId}:${event.lineNumber})`);
                }
            });
        });
        app.on('child-process-gone', (_event, details) => log.error(`${details.type} process gone: ${details.reason} (exit code ${details.exitCode})`));
        app.on('second-instance', (_event, argv) => log.info(`launched again: ${argv.slice(1).join(' ')}`));
        app.on('window-all-closed', () => log.info('all windows closed'));
        app.on('will-quit', () => log.info('quitting'));
        // Sleep and wake, so the gaps every process shows then (see watchEventLoop) are read as what they are.
        app.whenReady().then(() => {
            powerMonitor.on('suspend', () => log.info('the computer is going to sleep'));
            powerMonitor.on('resume', () => log.info('the computer woke up'));
            this.watchBackend();
        });
        app.on('activate', (_event, hasVisibleWindows) => {
            if (!hasVisibleWindows && !this.windows.size) {
                this.openWindowWithWorkspace('');
            }
        });
    }

    /**
     * Quits when this app can no longer serve, so the next launch starts a whole, current app. Theia does nothing
     * when its backend exits after starting, and an app whose files were replaced under it by an update lives on
     * (macOS kills the helper processes, not the app), holding the single-instance lock: every launch is handed to
     * it, nothing starts, and the agents' `co-review mcp` report that Co-Review did not start, again and again,
     * until it is quit by hand. Every 10 seconds:
     * - the backend, which registers itself with its process id (see CoReviewerMcp): once that process is dead;
     * - the version on disk (the app's own package.json): once it is not the one running, an update replaced it.
     */
    protected watchBackend(): void {
        const registry = path.join(path.dirname(logsDir()), 'server.json');
        const manifest = path.join(app.getAppPath(), 'package.json');
        let backendPid: number | undefined;
        let quitting = false;
        const quit = (why: string) => {
            if (quitting) {
                return;
            }
            quitting = true;
            log.error(`${why}: quitting, so the next launch starts a whole, current app`);
            app.quit();
            setTimeout(() => app.exit(1), 5000).unref();
        };
        setInterval(() => {
            try {
                const onDisk = JSON.parse(fs.readFileSync(manifest, 'utf8')).version;
                if (onDisk && onDisk !== app.getVersion()) {
                    quit(`Co-Review ${app.getVersion()} was replaced on disk by ${onDisk}`);
                    return;
                }
            } catch {
                /* being replaced right now, or not a packaged app: nothing to tell yet */
            }
            if (backendPid === undefined) {
                try {
                    backendPid = JSON.parse(fs.readFileSync(registry, 'utf8')).pid;
                } catch {
                    /* not registered yet */
                }
                return;
            }
            try {
                process.kill(backendPid, 0);
            } catch (e) {
                if ((e as NodeJS.ErrnoException).code === 'ESRCH') {
                    quit(`the backend (process ${backendPid}) is gone`);
                }
            }
        }, 10_000).unref();
    }

    protected override async openWindowWithWorkspace(workspacePath: string): Promise<BrowserWindow> {
        const existing = workspacePath ? this.windowOf(workspacePath) : undefined;
        log.info(`${existing ? 'focusing' : 'opening'} the window of ${workspacePath || 'no workspace'}`);
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

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

function alive(pid: number): boolean {
    try {
        process.kill(pid, 0);
        return true;
    } catch (e) {
        return (e as NodeJS.ErrnoException).code === 'EPERM';
    }
}

/** Whether a Co-Review backend answers at `url` (its MCP endpoint, as `co-review mcp` checks it). */
async function answers(url: string): Promise<boolean> {
    try {
        const res = await fetch(`${url}/mcp`, { method: 'GET', signal: AbortSignal.timeout(3000) });
        return res.status === 400 || res.status === 404 || res.ok;
    } catch {
        return false;
    }
}

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
        app.on('second-instance', (_event, argv) => {
            log.info(`launched again: ${argv.slice(1).join(' ')}`);
            this.relaunchIfStale(argv);
        });
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
        const manifest = path.join(app.getAppPath(), 'package.json');
        let backendPid: number | undefined;
        setInterval(() => {
            try {
                const onDisk = JSON.parse(fs.readFileSync(manifest, 'utf8')).version;
                if (onDisk && onDisk !== app.getVersion()) {
                    this.quitStale(`Co-Review ${app.getVersion()} was replaced on disk by ${onDisk}`);
                    return;
                }
            } catch {
                /* being replaced right now, or not a packaged app: nothing to tell yet */
            }
            if (backendPid === undefined) {
                backendPid = this.registeredBackend()?.pid;
                return;
            }
            if (!alive(backendPid)) {
                this.quitStale(`the backend (process ${backendPid}) is gone`);
            }
        }, 10_000).unref();
    }

    protected readonly startedAt = Date.now();
    protected quitting = false;

    protected quitStale(why: string): void {
        if (this.quitting) {
            return;
        }
        this.quitting = true;
        log.error(`${why}: quitting, so the next launch starts a whole, current app`);
        app.quit();
        setTimeout(() => app.exit(1), 5000).unref();
    }

    /**
     * The backend this app started, as it registered itself in server.json: only a registration written since this
     * app started is its own (an older one is a previous run's, whose process may be dead or another's).
     */
    protected registeredBackend(): { url: string, pid: number } | undefined {
        const registry = path.join(path.dirname(logsDir()), 'server.json');
        try {
            if (fs.statSync(registry).mtimeMs < this.startedAt - 1000) {
                return undefined;
            }
            const { url, pid } = JSON.parse(fs.readFileSync(registry, 'utf8'));
            return typeof url === 'string' && typeof pid === 'number' ? { url, pid } : undefined;
        } catch {
            return undefined;
        }
    }

    /**
     * A launch handed to this app (the single-instance lock) is only served if this app still has a backend. One
     * without (its files replaced under it, its backend crashed) would swallow every launch: the `co-review mcp`
     * of every agent session waits for a backend that never comes. Then the launch is not lost: this app quits and
     * starts again with the launch's arguments (`app.relaunch`, from the executable on disk, so after an update the
     * new version), which takes the lock once this one is gone. Restarting is simpler and surer than starting a
     * backend again from here: Theia starts its backend once, while the app starts, and windows connect to it.
     * Checked right away rather than at the next 10-second watch, as the launching command is waiting.
     */
    protected async relaunchIfStale(argv: string[]): Promise<void> {
        // Still starting: its backend registers within seconds of the start, and a launch now is just early.
        if (this.quitting || Date.now() - this.startedAt < 30_000) {
            return;
        }
        const backend = this.registeredBackend();
        let why: string | undefined;
        if (!backend) {
            why = 'no backend registered by this app';
        } else if (!alive(backend.pid)) {
            why = `the backend (process ${backend.pid}) is gone`;
        } else if (!await answers(backend.url)) {
            why = `the backend at ${backend.url} does not answer`;
        }
        if (why) {
            log.error(`launched again without a backend (${why}): starting again with that launch's arguments`);
            app.relaunch({ args: argv.slice(1) });
            this.quitStale(why);
        }
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

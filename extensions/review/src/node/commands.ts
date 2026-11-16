import { ChildProcess, execFile, spawn, SpawnOptions } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';

/**
 * Running the commands of agent harnesses and ACP agents (`claude`, `pi`, `npx`, `gemini`, …) on every platform.
 * On macOS and Linux they are executables or scripts with a shebang, and `spawn`/`execFile` run them. On Windows
 * they are mostly `.cmd` shims written by npm, which only `cmd.exe` can run: Node refuses to spawn one without a
 * shell. So on Windows the command is found on PATH (with PATHEXT, as cmd.exe does), and a `.cmd` or `.bat` is run
 * through `cmd.exe /d /s /c` with the arguments escaped the way cross-spawn does. The macOS and Linux paths are
 * unchanged. The Windows logic takes its platform and environment as arguments so it is testable anywhere.
 */

export interface Lookup {
    platform?: NodeJS.Platform;
    env?: NodeJS.ProcessEnv;
    /** Whether a path exists; the real file system by default. */
    exists?: (file: string) => boolean;
}

/**
 * The file `command` names on Windows, as cmd.exe would find it: with PATHEXT (`.COM;.EXE;.BAT;.CMD` by default)
 * appended unless it already has one of those extensions, in each PATH entry. Unlike cmd.exe, never in the current
 * directory: an agent runs with the repository under review as its directory, and a file there must not be what
 * `npx` resolves to. A command with a directory part is looked up there only. The extensionless shim npm writes
 * next to each `.cmd` (for bash) is never picked, since it cannot run.
 */
export function findOnWindowsPath(command: string, { env = process.env, exists = fs.existsSync }: Lookup = {}): string | undefined {
    const exts = (env.PATHEXT || '.COM;.EXE;.BAT;.CMD').split(';').filter(Boolean);
    const hasExt = exts.some(ext => command.toLowerCase().endsWith(ext.toLowerCase()));
    const names = hasExt ? [command] : exts.map(ext => command + ext);
    const dirs = /[\\/]/.test(command) ? [''] : (env.PATH ?? env.Path ?? '').split(';').filter(Boolean);
    for (const dir of dirs) {
        for (const name of names) {
            const file = dir ? path.win32.join(dir, name) : name;
            if (exists(file)) {
                return file;
            }
        }
    }
    return undefined;
}

/** Whether `command` is available: `which` on macOS and Linux, a PATHEXT lookup on Windows. */
export function commandExists(command: string, lookup: Lookup = {}): Promise<boolean> {
    if ((lookup.platform ?? process.platform) === 'win32') {
        return Promise.resolve(!!findOnWindowsPath(command, lookup));
    }
    return new Promise(resolve => execFile('/usr/bin/env', ['which', command], err => resolve(!err)));
}

// cmd.exe quoting, as cross-spawn does it (lib/util/escape.js): meta characters are escaped with ^, arguments are
// double-quoted with their quotes and trailing backslashes doubled, and escaped twice for a .cmd/.bat, which
// cmd.exe parses a second time.
const META = /([()\][%!^"`<>&|;, *?])/g;

function escapeCommand(arg: string): string {
    return arg.replace(META, '^$1');
}

function escapeArgument(arg: string): string {
    let out = `${arg}`;
    out = out.replace(/(?=(\\+?)?)\1"/g, '$1$1\\"');
    out = out.replace(/(?=(\\+?)?)\1$/, '$1$1');
    out = `"${out}"`;
    out = out.replace(META, '^$1');
    out = out.replace(META, '^$1');
    return out;
}

function isBatch(file: string): boolean {
    return /\.(cmd|bat)$/i.test(file);
}

/** The `cmd.exe /d /s /c` command line that runs the batch file `file` with `args`. */
export function windowsCommandLine(file: string, args: string[]): string {
    return `"${[escapeCommand(file), ...args.map(escapeArgument)].join(' ')}"`;
}

/** What to spawn for `command` with `args` on Windows: the resolved file, or cmd.exe for a .cmd/.bat. */
export function windowsSpawn(command: string, args: string[], lookup: Lookup = {}): { file: string; args: string[]; batch: boolean } {
    const file = findOnWindowsPath(command, lookup) ?? command;
    if (!isBatch(file)) {
        return { file, args, batch: false };
    }
    const env = lookup.env ?? process.env;
    return { file: env.ComSpec || env.comspec || 'cmd.exe', args: ['/d', '/s', '/c', windowsCommandLine(file, args)], batch: true };
}

/** `spawn(command, args, options)`, through cmd.exe when `command` is a .cmd or .bat on Windows. */
export function spawnCommand(command: string, args: string[], options: SpawnOptions): ChildProcess {
    if (process.platform !== 'win32') {
        return spawn(command, args, options);
    }
    const w = windowsSpawn(command, args, { env: (options.env as NodeJS.ProcessEnv | undefined) ?? process.env });
    return spawn(w.file, w.args, w.batch ? { ...options, windowsVerbatimArguments: true } : options);
}

/** Runs `command` to completion and resolves to its stdout; rejects with its stderr (or the error) when it fails. */
export function runCommand(command: string, args: string[], timeout = 30000): Promise<string> {
    if (process.platform !== 'win32') {
        return new Promise((resolve, reject) => execFile(command, args, { timeout }, (err, stdout, stderr) =>
            err ? reject(new Error((stderr || err.message).trim())) : resolve(stdout.trim())));
    }
    return new Promise((resolve, reject) => {
        const child = spawnCommand(command, args, { stdio: ['ignore', 'pipe', 'pipe'], timeout });
        let stdout = '';
        let stderr = '';
        child.stdout!.on('data', chunk => { stdout += chunk; });
        child.stderr!.on('data', chunk => { stderr += chunk; });
        child.on('error', err => reject(new Error(err.message)));
        child.on('close', code => code === 0 ? resolve(stdout.trim()) : reject(new Error((stderr || `${command} exited with ${code}`).trim())));
    });
}

/**
 * Ends a child and what it started. On Windows a `.cmd` runs under cmd.exe, so killing the child alone would leave
 * the program it started running: the whole process tree is ended (`taskkill /t`).
 */
export function killTree(child: ChildProcess): void {
    if (process.platform === 'win32' && child.pid) {
        spawn('taskkill', ['/pid', String(child.pid), '/t', '/f'], { stdio: 'ignore' }).on('error', () => child.kill());
    } else {
        child.kill();
    }
}

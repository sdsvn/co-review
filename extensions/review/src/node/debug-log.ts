import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

/**
 * Co-Review's debug logs, for when something hangs or fails: `$CO_REVIEW_HOME/logs/<name>.log` (default
 * `~/.co-review/logs`), one file per process kind (`main`: the desktop app's windows; `backend`; `bridge`: each
 * `co-review mcp`), every line stamped with the time and process id. `co-review logs` prints them merged by time,
 * with whether Co-Review answers. A file over 5 MB is moved to `<name>.1.log` when a process starts writing it.
 */
export function logsDir(): string {
    return path.join(process.env.CO_REVIEW_HOME || path.join(os.homedir(), '.co-review'), 'logs');
}

const MAX_BYTES = 5 * 1024 * 1024;

export class DebugLog {

    protected readonly stream: fs.WriteStream | undefined;

    constructor(readonly name: string) {
        const file = path.join(logsDir(), `${name}.log`);
        try {
            fs.mkdirSync(logsDir(), { recursive: true });
            if ((fs.statSync(file, { throwIfNoEntry: false })?.size ?? 0) > MAX_BYTES) {
                fs.renameSync(file, path.join(logsDir(), `${name}.1.log`));
            }
            this.stream = fs.createWriteStream(file, { flags: 'a' });
            this.stream.on('error', () => undefined);
        } catch {
            /* no logs, rather than no app */
        }
    }

    /** One entry; a multi-line text keeps the stamp on its first line and indents the rest. */
    write(level: 'info' | 'warn' | 'error', text: string): void {
        const lines = text.replace(/\s+$/, '').split('\n');
        this.stream?.write(`${new Date().toISOString()} ${process.pid} ${level.toUpperCase().padEnd(5)} ${lines.join('\n    ')}\n`);
    }

    info(text: string): void {
        this.write('info', text);
    }

    warn(text: string): void {
        this.write('warn', text);
    }

    error(text: string): void {
        this.write('error', text);
    }
}

/** Also writes everything this process prints (stdout, stderr) into the log, as it goes to its terminal (if any). */
export function captureOutput(log: DebugLog): void {
    for (const [stream, level] of [[process.stdout, 'info'], [process.stderr, 'error']] as const) {
        const write = stream.write.bind(stream) as (chunk: unknown, ...rest: unknown[]) => boolean;
        stream.write = ((chunk: unknown, ...rest: unknown[]) => {
            try {
                log.write(level, typeof chunk === 'string' ? chunk : Buffer.from(chunk as Uint8Array).toString('utf8'));
            } catch {
                /* the log never breaks the output */
            }
            try {
                return write(chunk, ...rest);
            } catch {
                // Nobody reads the output (a detached app): the log has it.
                return true;
            }
        }) as typeof stream.write;
    }
}

/**
 * Logs when this process's event loop was blocked for longer than `thresholdMs`: a window that stops responding, or a
 * backend that stops answering, shows up here with when and for how long.
 */
export function watchEventLoop(log: DebugLog, thresholdMs = 1000): void {
    const interval = 500;
    let last = Date.now();
    setInterval(() => {
        const now = Date.now();
        const blocked = now - last - interval;
        if (blocked > thresholdMs) {
            log.warn(`event loop blocked for ${blocked} ms`);
        }
        last = now;
    }, interval).unref();
}

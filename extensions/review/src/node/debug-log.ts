import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

/**
 * Co-Review's debug logs, for when something hangs or fails: `$CO_REVIEW_HOME/logs/<name>.log` (default
 * `~/.co-review/logs`), one file per process kind (`main`: the desktop app's windows; `backend`; `cli`: the
 * `co-review` command and each `co-review mcp`), every line stamped with the time and process id. `co-review logs`
 * prints the end of them merged by time, with whether Co-Review answers.
 *
 * A log can never grow without bound, whatever floods it (a loop logging an error millions of times): a file is moved
 * to `<name>.1.log` once it passes 5 MB, an entry repeated right after itself is counted instead of written again,
 * and an entry is cut at 4 KB.
 */
export function logsDir(): string {
    return path.join(process.env.CO_REVIEW_HOME || path.join(os.homedir(), '.co-review'), 'logs');
}

const MAX_BYTES = 5 * 1024 * 1024;
const MAX_ENTRY = 4 * 1024;

export class DebugLog {

    /** The open log file; written synchronously, so moving it aside is exact (and nothing is lost at exit). */
    protected fd: number | undefined;
    protected readonly file: string;
    protected written = 0;
    /** The last entry, without its time stamp, and how many times it came again since. */
    protected last: { level: string; text: string; repeats: number; since: number } | undefined;

    constructor(readonly name: string) {
        this.file = path.join(logsDir(), `${name}.log`);
        try {
            fs.mkdirSync(logsDir(), { recursive: true });
            this.written = fs.statSync(this.file, { throwIfNoEntry: false })?.size ?? 0;
            this.open();
        } catch {
            /* no logs, rather than no app */
        }
    }

    protected open(): void {
        if (this.written > MAX_BYTES) {
            fs.renameSync(this.file, path.join(logsDir(), `${this.name}.1.log`));
            this.written = 0;
        }
        this.fd = fs.openSync(this.file, 'a');
    }

    /** One entry; a multi-line text keeps the stamp on its first line and indents the rest. */
    write(level: 'info' | 'warn' | 'error', text: string): void {
        if (this.fd === undefined) {
            return;
        }
        let body = text.replace(/\s+$/, '');
        if (body.length > MAX_ENTRY) {
            body = `${body.slice(0, MAX_ENTRY)} … (${body.length - MAX_ENTRY} more characters)`;
        }
        // The same entry again (a loop): counted, and written once it stops or every 10 seconds.
        const key = body.replace(/\d{4}-\d\d-\d\dT[\d:.]+Z/g, '');
        if (this.last && this.last.level === level && this.last.text === key) {
            this.last.repeats++;
            if (Date.now() - this.last.since < 10_000) {
                return;
            }
        }
        this.flushRepeats();
        this.last = { level, text: key, repeats: 0, since: Date.now() };
        this.append(level, body);
    }

    protected flushRepeats(): void {
        if (this.last?.repeats) {
            this.append(this.last.level as 'info', `(the entry above came ${this.last.repeats} more times)`);
            this.last.repeats = 0;
            this.last.since = Date.now();
        }
    }

    protected append(level: 'info' | 'warn' | 'error', body: string): void {
        const line = `${new Date().toISOString()} ${process.pid} ${level.toUpperCase().padEnd(5)} ${body.split('\n').join('\n    ')}\n`;
        if (this.fd === undefined) {
            return;
        }
        try {
            fs.writeSync(this.fd, line);
            this.written += Buffer.byteLength(line);
            if (this.written > MAX_BYTES) {
                // Moved aside while running too, not only at the next start.
                fs.closeSync(this.fd);
                this.fd = undefined;
                this.open();
            }
        } catch {
            this.fd = undefined;
        }
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

/**
 * Lets through at most `perSecond` entries a second from one source (a window's console), and says how many were
 * dropped: a window caught in a loop can't flood the log or keep the main process busy.
 */
export function rateLimit(log: DebugLog, perSecond = 20): (level: 'info' | 'warn' | 'error', text: string) => void {
    let windowStart = 0;
    let count = 0;
    let dropped = 0;
    return (level, text) => {
        const now = Date.now();
        if (now - windowStart >= 1000) {
            if (dropped) {
                log.warn(`(${dropped} more entries dropped in a second: too many)`);
            }
            windowStart = now;
            count = 0;
            dropped = 0;
        }
        if (++count > perSecond) {
            dropped++;
            return;
        }
        log.write(level, text);
    };
}

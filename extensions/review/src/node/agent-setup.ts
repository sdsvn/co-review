import { FileUri } from '@theia/core/lib/common/file-uri';
import { injectable } from '@theia/core/shared/inversify';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { AgentSetupInfo, HarnessSetup, McpLaunch, SkillTarget } from '../common/review-protocol';
import { runCommand } from './commands';

const SERVER = 'co-review';

/** First existing path, relative to the backend bundle (`lib/backend`): packaged app, then checkout. */
function bundled(...candidates: string[]): string | undefined {
    return candidates.map(c => path.resolve(__dirname, c)).find(p => fs.existsSync(p));
}

/** Runs a harness's command (`claude`, `pi`, `omp`; on Windows .cmd shims, see commands.ts) and returns its output. */
function exec(command: string, args: string[]): Promise<string> {
    return runCommand(command, args);
}

/**
 * Connecting agents to Co-Review from inside the app: installs the bundled skills into an agent's
 * skills directory, and adds Co-Review's MCP server to an agent harness's config (or hands out the
 * snippet where the config format isn't safe to edit).
 */
@injectable()
export class AgentSetup {

    protected get skillsDir(): string | undefined {
        return bundled('../../plugin/skills', '../../../../plugin/skills');
    }

    /** Root with `.claude-plugin/marketplace.json` (the packaged app, or the checkout). */
    protected get marketplaceDir(): string | undefined {
        return ['../..', '../../../..'].map(c => path.resolve(__dirname, c)).find(d => fs.existsSync(path.join(d, '.claude-plugin', 'marketplace.json')));
    }

    protected get piPackageDir(): string | undefined {
        return bundled('../../integrations/pi', '../../../../integrations/pi');
    }

    protected get ompPackageDir(): string | undefined {
        return bundled('../../integrations/omp', '../../../../integrations/omp');
    }

    /** Where `installCli` puts the commands on Windows: `.cmd` shims in ~/.co-review/bin (no /usr/local/bin there). */
    protected get windowsBinDir(): string {
        return path.join(os.homedir(), '.co-review', 'bin');
    }

    protected get installedCli(): string | undefined {
        const candidates = process.platform === 'win32'
            ? [path.join(this.windowsBinDir, `${SERVER}.cmd`)]
            : [path.join('/usr/local/bin', SERVER), path.join(os.homedir(), '.local', 'bin', SERVER)];
        return candidates.find(p => fs.existsSync(p));
    }

    /**
     * Installs the `co-review` commands. On macOS and Linux, bin/install-cli.sh writes shell shims to /usr/local/bin
     * when writable, else ~/.local/bin. Windows has no `sh`, so the shims are written here, as `.cmd` files.
     */
    async installCli(): Promise<string> {
        if (process.platform === 'win32') {
            return this.installCliWindows();
        }
        const script = bundled('../../bin/install-cli.sh', '../../../../bin/install-cli.sh');
        if (!script) {
            throw new Error('install-cli.sh is not bundled with this build.');
        }
        return exec('/bin/sh', [script]);
    }

    /**
     * Writes `co-review.cmd` and `co-review-server.cmd` to ~/.co-review/bin. Installed from the app, they run
     * bin/co-review.mjs on the app's own Electron as Node (ELECTRON_RUN_AS_NODE=1, no Node.js needed); from a
     * checkout, on `node` from PATH. Like install-cli.sh, this says when the directory is not on PATH and leaves
     * adding it to the user (a change to their environment they should see).
     */
    protected installCliWindows(): string {
        const bin = bundled('../../bin', '../../../../bin');
        if (!bin || !fs.existsSync(path.join(bin, 'co-review.mjs'))) {
            throw new Error('co-review.mjs is not bundled with this build.');
        }
        const dir = this.windowsBinDir;
        fs.mkdirSync(dir, { recursive: true });
        // Installed from the app: the app's Electron as Node; from a checkout (browser app): node from PATH.
        const electron = !!process.versions.electron;
        const exe = electron ? `"${process.execPath}"` : 'node';
        const lines: string[] = [];
        for (const [cmd, script] of [[SERVER, 'co-review.mjs'], [`${SERVER}-server`, 'review-server.mjs']]) {
            const file = path.join(dir, `${cmd}.cmd`);
            const body = ['@echo off', 'setlocal', ...electron ? ['set "ELECTRON_RUN_AS_NODE=1"'] : [], `${exe} "${path.join(bin, script)}" %*`];
            fs.writeFileSync(file, body.join('\r\n') + '\r\n');
            lines.push(`installed ${file}`);
        }
        // What the shims run, for programs that cannot run a .cmd without a shell (the Pi and Oh My Pi packages,
        // integrations/shared/co-review.ts): the exe and script to spawn directly, with the environment they need.
        const launch = electron
            ? { command: process.execPath, args: [path.join(bin, 'co-review.mjs')], env: { ELECTRON_RUN_AS_NODE: '1' } }
            : { command: 'node', args: [path.join(bin, 'co-review.mjs')], env: {} };
        fs.writeFileSync(path.join(dir, 'launch.json'), JSON.stringify(launch, undefined, 2) + '\n');
        const normalize = (p: string) => p.trim().replace(/[\\/]+$/, '').toLowerCase();
        if (!(process.env.PATH ?? '').split(';').some(p => normalize(p) === normalize(dir))) {
            lines.push(`note: ${dir} is not on your PATH; add it (Start > "Edit environment variables for your account" > Path), `
                + 'then open a new terminal.');
        }
        return lines.join('\n');
    }

    /**
     * How a harness should launch `co-review mcp`: the installed CLI, else this app's own runtime. On Windows always
     * the runtime: the installed command is a `.cmd`, which harnesses that spawn without a shell can't run.
     */
    launch(): McpLaunch {
        const cli = process.platform === 'win32' ? undefined : this.installedCli;
        if (cli) {
            return { command: cli, args: ['mcp'] };
        }
        const script = bundled('../../bin/co-review.mjs', '../../../../bin/co-review.mjs') ?? 'co-review.mjs';
        return process.versions.electron
            ? { command: process.execPath, args: [script, 'mcp'], env: { ELECTRON_RUN_AS_NODE: '1' } }
            : { command: process.execPath, args: [script, 'mcp'] };
    }

    info(workspaceRoot?: string): AgentSetupInfo {
        const launch = this.launch();
        return { launch, skills: this.skillTargets(workspaceRoot), harnesses: this.harnesses(launch), skillsAvailable: !!this.skillsDir, cliInstalled: !!this.installedCli };
    }

    protected skillTargets(workspaceRoot?: string): SkillTarget[] {
        const home = os.homedir();
        const targets: SkillTarget[] = [
            { id: 'claude', label: 'Claude Code (user)', dir: path.join(home, '.claude', 'skills') },
            { id: 'agents', label: 'Shared agents directory (Pi, skills CLI)', dir: path.join(home, '.agents', 'skills') }
        ];
        if (workspaceRoot) {
            targets.push({ id: 'project', label: 'This repository (Claude Code)', dir: path.join(FileUri.fsPath(workspaceRoot), '.claude', 'skills') });
        }
        return targets;
    }

    async installSkills(targetId: string, workspaceRoot?: string): Promise<string[]> {
        const source = this.skillsDir;
        const target = this.skillTargets(workspaceRoot).find(t => t.id === targetId);
        if (!source || !target) {
            throw new Error(source ? `Unknown target ${targetId}` : 'The skills are not bundled with this build.');
        }
        const installed: string[] = [];
        for (const name of fs.readdirSync(source).filter(n => fs.existsSync(path.join(source, n, 'SKILL.md')))) {
            const dest = path.join(target.dir, name);
            fs.rmSync(dest, { recursive: true, force: true });
            fs.cpSync(path.join(source, name), dest, { recursive: true });
            installed.push(dest);
        }
        return installed;
    }

    protected harnesses(l: McpLaunch): HarnessSetup[] {
        const home = os.homedir();
        const win = process.platform === 'win32';
        const env = l.env ?? {};
        const hasEnv = Object.keys(env).length > 0;
        const stdio = { command: l.command, args: l.args, ...hasEnv ? { env } : {} };
        const json = (value: unknown) => JSON.stringify(value, undefined, 2);
        const mac = process.platform === 'darwin';
        // Where apps keep per-user config: Application Support, %APPDATA% (Roaming), or XDG's ~/.config.
        const appSupport = mac ? path.join(home, 'Library', 'Application Support')
            : win ? process.env.APPDATA || path.join(home, 'AppData', 'Roaming')
            : path.join(home, '.config');
        const claudeArgs = ['mcp', 'add', '-s', 'user', ...Object.entries(env).flatMap(([k, v]) => ['-e', `${k}=${v}`]), SERVER, '--', l.command, ...l.args];
        // Quoting for the snippet the reviewer runs themselves: a POSIX shell, or cmd.exe / PowerShell on Windows.
        const quote = (s: string) => win
            ? (/^[\w@%+=:,.\\/-]+$/.test(s) ? s : `"${s.replace(/"/g, '\\"')}"`)
            : (/^[\w@%+=:,./-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`);
        const cli = (label: string, id: string, commands: string[][], detail?: string): HarnessSetup =>
            ({ id, label, kind: 'cli', argv: commands, snippet: commands.map(c => c.map(quote).join(' ')).join('\n'), detail });
        const market = this.marketplaceDir;
        const pi = this.piPackageDir;
        const omp = this.ompPackageDir;
        return [
            // The plugin declares its MCP server as the bare `co-review` command, and a plugin manifest cannot say
            // anything else per platform. On Windows that command is the .cmd shim installCli writes, which (as far
            // as the Claude Code docs say) a spawn without a shell cannot run. So on Windows the plugin is not
            // offered, and the MCP server is added by its launch command instead. Revisit when verified there.
            ...market && !win ? [cli('Claude Code: plugin (skills, commands and MCP server)', 'claude-code-plugin',
                [['claude', 'plugin', 'marketplace', 'add', market], ['claude', 'plugin', 'install', 'co-review@co-review']])] : [],
            cli(win ? 'Claude Code: MCP server (install the skills separately)' : 'Claude Code: MCP server only', 'claude-code', [['claude', ...claudeArgs]]),
            ...pi ? [cli('Pi: package (extension, /co-review, co-reviewer subagent, skills)', 'pi', [['pi', 'install', pi]])] : [],
            ...omp ? [cli('Oh My Pi: package (extension, /co-review, /co-review-design, skills)', 'omp', [['omp', 'install', omp]])] : [],
            { id: 'codex', label: 'Codex', kind: 'toml', file: path.join(home, '.codex', 'config.toml'),
                snippet: [`[mcp_servers.${SERVER}]`, `command = ${JSON.stringify(l.command)}`, `args = ${JSON.stringify(l.args)}`,
                    ...hasEnv ? [`env = { ${Object.entries(env).map(([k, v]) => `${k} = ${JSON.stringify(v)}`).join(', ')} }`] : [],
                    '# await_reviewer and ask_reviewer block for minutes', 'tool_timeout_sec = 600'].join('\n') },
            { id: 'cursor', label: 'Cursor', kind: 'json', file: path.join(home, '.cursor', 'mcp.json'), key: ['mcpServers'], snippet: json({ mcpServers: { [SERVER]: stdio } }) },
            { id: 'claude-desktop', label: 'Claude Desktop', kind: 'json', file: path.join(appSupport, 'Claude', 'claude_desktop_config.json'), key: ['mcpServers'],
                snippet: json({ mcpServers: { [SERVER]: stdio } }) },
            { id: 'vscode', label: 'VS Code (Copilot)', kind: 'json', file: path.join(appSupport, 'Code', 'User', 'mcp.json'), key: ['servers'],
                snippet: json({ servers: { [SERVER]: { type: 'stdio', ...stdio } } }) },
            { id: 'windsurf', label: 'Windsurf', kind: 'json', file: path.join(home, '.codeium', 'windsurf', 'mcp_config.json'), key: ['mcpServers'],
                snippet: json({ mcpServers: { [SERVER]: stdio } }) },
            { id: 'gemini', label: 'Gemini CLI', kind: 'json', file: path.join(home, '.gemini', 'settings.json'), key: ['mcpServers'],
                snippet: json({ mcpServers: { [SERVER]: { ...stdio, timeout: 600000 } } }) },
            { id: 'opencode', label: 'OpenCode', kind: 'json', file: path.join(home, '.config', 'opencode', 'opencode.json'), key: ['mcp'],
                snippet: json({ mcp: { [SERVER]: { type: 'local', command: [l.command, ...l.args], enabled: true, ...hasEnv ? { environment: env } : {} } } }) },
            // The Zed and Goose Windows paths are from memory of their docs, not confirmed: both are copy-only hints.
            { id: 'zed', label: 'Zed', kind: 'manual', file: win ? path.join(appSupport, 'Zed', 'settings.json') : path.join(home, '.config', 'zed', 'settings.json'),
                snippet: json({ context_servers: { [SERVER]: { source: 'custom', ...stdio } } }) },
            { id: 'goose', label: 'Goose', kind: 'manual', file: win ? path.join(appSupport, 'Block', 'goose', 'config', 'config.yaml') : path.join(home, '.config', 'goose', 'config.yaml'),
                snippet: ['extensions:', `  ${SERVER}:`, `    name: ${SERVER}`, '    type: stdio', `    cmd: ${JSON.stringify(l.command)}`, `    args: ${JSON.stringify(l.args)}`,
                    ...hasEnv ? [`    envs: ${JSON.stringify(env)}`] : [], '    enabled: true', '    timeout: 600'].join('\n') }
        ];
    }

    /** Adds Co-Review to the harness's config; returns a one-line result. JSON is merged, never overwritten. */
    async apply(harnessId: string): Promise<string> {
        const h = this.harnesses(this.launch()).find(x => x.id === harnessId);
        if (!h) {
            throw new Error(`Unknown harness ${harnessId}`);
        }
        if (h.kind === 'cli') {
            for (const [command, ...args] of h.argv!) {
                await exec(command, args);
            }
            return `Set up ${h.label}.`;
        }
        if (!h.file || h.kind === 'manual') {
            throw new Error(`${h.label}'s config can't be edited safely; paste the snippet into ${h.file}.`);
        }
        fs.mkdirSync(path.dirname(h.file), { recursive: true });
        const existing = fs.existsSync(h.file) ? fs.readFileSync(h.file, 'utf8') : '';
        if (h.kind === 'toml') {
            if (existing.includes(`[mcp_servers.${SERVER}]`)) {
                return `${h.file} already has [mcp_servers.${SERVER}]; left unchanged.`;
            }
            fs.writeFileSync(h.file, `${existing}${existing && !existing.endsWith('\n') ? '\n' : ''}\n${h.snippet}\n`);
            return `Added to ${h.file}.`;
        }
        let config: Record<string, any>;
        try {
            config = existing.trim() ? JSON.parse(existing) : {};
        } catch {
            throw new Error(`${h.file} isn't plain JSON (comments?); paste the snippet instead.`);
        }
        const [key] = h.key!;
        const entry = JSON.parse(h.snippet)[key][SERVER];
        config[key] = { ...config[key], [SERVER]: entry };
        if (existing) {
            fs.writeFileSync(`${h.file}.co-review.bak`, existing);
        }
        fs.writeFileSync(h.file, JSON.stringify(config, undefined, 2) + '\n');
        return `Added to ${h.file}${existing ? ` (previous version saved as ${path.basename(h.file)}.co-review.bak)` : ''}.`;
    }
}

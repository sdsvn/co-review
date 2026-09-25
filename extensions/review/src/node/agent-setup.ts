import { FileUri } from '@theia/core/lib/common/file-uri';
import { injectable } from '@theia/core/shared/inversify';
import { execFile } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { AgentSetupInfo, HarnessSetup, McpLaunch, SkillTarget } from '../common/review-protocol';

const SERVER = 'co-review';

/** First existing path, relative to the backend bundle (`lib/backend`): packaged app, then checkout. */
function bundled(...candidates: string[]): string | undefined {
    return candidates.map(c => path.resolve(__dirname, c)).find(p => fs.existsSync(p));
}

function exec(command: string, args: string[]): Promise<string> {
    return new Promise((resolve, reject) => execFile(command, args, { timeout: 30000 }, (err, stdout, stderr) =>
        err ? reject(new Error((stderr || err.message).trim())) : resolve(stdout.trim())));
}

/**
 * Connecting agents to Co-Review from inside the app: installs the bundled skills into an agent's
 * skills directory, and adds Co-Review's MCP server to an agent harness's config (or hands out the
 * snippet where the config format isn't safe to edit).
 */
@injectable()
export class AgentSetup {

    protected get skillsDir(): string | undefined {
        return bundled('../../skills', '../../../../plugin/skills');
    }

    /** How a harness should launch `co-review mcp`: the installed CLI, else this app's own runtime. */
    launch(): McpLaunch {
        const cli = [path.join('/usr/local/bin', SERVER), path.join(os.homedir(), '.local', 'bin', SERVER)].find(p => fs.existsSync(p));
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
        return { launch, skills: this.skillTargets(workspaceRoot), harnesses: this.harnesses(launch), skillsAvailable: !!this.skillsDir };
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
        const env = l.env ?? {};
        const hasEnv = Object.keys(env).length > 0;
        const stdio = { command: l.command, args: l.args, ...hasEnv ? { env } : {} };
        const json = (value: unknown) => JSON.stringify(value, undefined, 2);
        const mac = process.platform === 'darwin';
        const appSupport = mac ? path.join(home, 'Library', 'Application Support') : path.join(home, '.config');
        const claudeArgs = ['mcp', 'add', '-s', 'user', ...Object.entries(env).flatMap(([k, v]) => ['-e', `${k}=${v}`]), SERVER, '--', l.command, ...l.args];
        const quote = (s: string) => /^[\w@%+=:,./-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`;
        return [
            { id: 'claude-code', label: 'Claude Code', kind: 'cli', argv: ['claude', ...claudeArgs], snippet: ['claude', ...claudeArgs].map(quote).join(' ') },
            { id: 'codex', label: 'Codex', kind: 'toml', file: path.join(home, '.codex', 'config.toml'),
                snippet: [`[mcp_servers.${SERVER}]`, `command = ${JSON.stringify(l.command)}`, `args = ${JSON.stringify(l.args)}`,
                    ...hasEnv ? [`env = { ${Object.entries(env).map(([k, v]) => `${k} = ${JSON.stringify(v)}`).join(', ')} }`] : [],
                    '# await_comment / await_review block for minutes', 'tool_timeout_sec = 600'].join('\n') },
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
            { id: 'zed', label: 'Zed', kind: 'manual', file: path.join(home, '.config', 'zed', 'settings.json'),
                snippet: json({ context_servers: { [SERVER]: { source: 'custom', ...stdio } } }) },
            { id: 'goose', label: 'Goose', kind: 'manual', file: path.join(home, '.config', 'goose', 'config.yaml'),
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
            await exec(h.argv![0], h.argv!.slice(1));
            return 'Added to Claude Code (user scope).';
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

// Entry point of the Co-Review desktop app: points Theia at the bundled VS Code built-in
// extensions (Git, language support), then starts Theia's generated Electron main.
const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

// Opened from Finder/Dock, the app gets launchd's minimal environment (PATH=/usr/bin:/bin:…), so
// language servers (gopls, …) and ACP agents are not found. Take the login shell's environment,
// as terminal-launched editors have it. Skipped when started from a shell.
if (process.platform !== 'win32' && !process.env.TERM_PROGRAM && !process.env.CO_REVIEW_NO_SHELL_ENV) {
    try {
        const shell = process.env.SHELL || '/bin/zsh';
        const mark = '__CO_REVIEW_ENV__';
        const out = execFileSync(shell, ['-ilc', `printf '${mark}'; command env -0`], { encoding: 'utf8', timeout: 5000, stdio: ['ignore', 'pipe', 'ignore'] });
        for (const entry of out.slice(out.indexOf(mark) + mark.length).split('\0')) {
            const i = entry.indexOf('=');
            const key = entry.slice(0, i);
            if (i > 0 && !['_', 'SHLVL', 'PWD', 'OLDPWD'].includes(key) && (key === 'PATH' || process.env[key] === undefined)) {
                process.env[key] = entry.slice(i + 1);
            }
        }
    } catch {
        /* keep the launch environment */
    }
}

if (!process.env.THEIA_DEFAULT_PLUGINS) {
    // Packaged app: <app>/plugins. Development: the repository's plugins/ folder.
    const candidates = [path.resolve(__dirname, '..', 'plugins'), path.resolve(__dirname, '..', '..', '..', 'plugins')];
    const dir = candidates.find(d => fs.existsSync(d));
    if (dir) {
        process.env.THEIA_DEFAULT_PLUGINS = `local-dir:${dir}`;
    }
}

require('../lib/backend/electron-main');

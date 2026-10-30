// electron-builder afterPack hook: drops the node_modules the app never loads.
//
// The app's code is bundled by esbuild into lib/ (frontend, backend, plugin host), and its native modules and
// ripgrep are copied into lib/backend/native, yet electron-builder still ships every production dependency next to
// it: more than the app itself. Only two packages are loaded from node_modules at run time, so only they and what
// they depend on are kept:
// - @modelcontextprotocol/sdk: imported by the `co-review` CLI (bin/) and the Pi / Oh My Pi integrations;
// - @vscode/tree-sitter-wasm: its grammars are loaded by path (SyntaxServiceImpl.wasmDir).
const fs = require('fs');
const path = require('path');

const KEEP = ['@modelcontextprotocol/sdk', '@vscode/tree-sitter-wasm'];

/** The installed directory of `name` as Node would find it from `from` (a package directory). */
function locate(name, from, root) {
    for (let dir = from; ; dir = path.dirname(dir)) {
        const candidate = path.join(dir, 'node_modules', name);
        if (fs.existsSync(path.join(candidate, 'package.json'))) {
            return candidate;
        }
        if (dir === root || dir === path.dirname(dir)) {
            return undefined;
        }
    }
}

/** Every package directory `roots` need, following dependencies and optional dependencies. */
function closure(appDir) {
    const kept = new Set();
    const queue = KEEP.map(name => {
        const dir = locate(name, appDir, appDir);
        if (!dir) {
            throw new Error(`prune-node-modules: ${name} is not installed in ${appDir}`);
        }
        return dir;
    });
    while (queue.length) {
        const dir = queue.pop();
        if (kept.has(dir)) {
            continue;
        }
        kept.add(dir);
        const pkg = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8'));
        for (const dep of Object.keys({ ...pkg.dependencies, ...pkg.optionalDependencies })) {
            const found = locate(dep, dir, appDir);
            if (found) {
                queue.push(found);
            } else if (!(pkg.optionalDependencies && dep in pkg.optionalDependencies)) {
                throw new Error(`prune-node-modules: ${dep} (needed by ${pkg.name}) is not installed`);
            }
        }
    }
    return kept;
}

function size(dir) {
    let total = 0;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        total += entry.isDirectory() ? size(full) : entry.isFile() ? fs.statSync(full).size : 0;
    }
    return total;
}

exports.default = async function (context) {
    const { appOutDir, packager, electronPlatformName } = context;
    const appDir = electronPlatformName === 'darwin' || electronPlatformName === 'mas'
        ? path.join(appOutDir, `${packager.appInfo.productFilename}.app`, 'Contents', 'Resources', 'app')
        : path.join(appOutDir, 'resources', 'app');
    const modules = path.join(appDir, 'node_modules');
    if (!fs.existsSync(modules)) {
        return;
    }
    const before = size(modules);
    const kept = closure(appDir);
    // Top-level packages (and @scope/name) not in the closure go; nested node_modules inside kept packages stay.
    for (const entry of fs.readdirSync(modules)) {
        const full = path.join(modules, entry);
        if (entry.startsWith('@')) {
            for (const scoped of fs.readdirSync(full)) {
                if (!kept.has(path.join(full, scoped))) {
                    fs.rmSync(path.join(full, scoped), { recursive: true, force: true });
                }
            }
            if (!fs.readdirSync(full).length) {
                fs.rmdirSync(full);
            }
        } else if (!kept.has(full)) {
            fs.rmSync(full, { recursive: true, force: true });
        }
    }
    const mb = n => `${Math.round(n / 1048576)} MB`;
    console.log(`  • pruned node_modules  ${mb(before)} → ${mb(size(modules))} (kept ${kept.size} packages)`);
};

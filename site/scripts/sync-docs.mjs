// Generates the site's guide and reference pages from the repository's docs (docs/*.md, llms.txt), so the
// Markdown in the repo stays the single source. Runs before `astro dev` / `astro build`. The site's own
// /llms.txt, /llms-full.txt and /llms-small.txt are generated from all pages by starlight-llms-txt.
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const site = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repo = resolve(site, '..');
const base = '/co-review';
const github = 'https://github.com/sdsvn/co-review';
const out = join(site, 'src', 'content', 'docs', 'docs');

/** Repository file → site page (slug under /docs/), with its sidebar order. */
const pages = {
	'docs/agent-setup.md': { slug: 'guides/connect-your-agent', order: 1, title: 'Connect your agent' },
	'docs/agents.md': { slug: 'guides/agents', order: 2, title: 'How agents review with you' },
	'docs/design-docs.md': { slug: 'guides/design-docs', order: 3, title: 'Design documents' },
	'docs/running-and-packaging.md': { slug: 'guides/running-and-packaging', order: 4, title: 'Running and packaging' },
	'llms.txt': { slug: 'reference/agent-contract', order: 1, title: 'Agent contract (llms.txt)' },
	'docs/architecture.md': { slug: 'reference/architecture', order: 2, title: 'Architecture' }
};
const readmeAnchors = { install: 'start/install', 'made-for-claude-code': 'start/claude-code' };

function link(target, from) {
	if (/^(https?:|mailto:|#)/.test(target)) {
		return target;
	}
	const [file, hash] = target.split('#');
	const path = resolve(repo, dirname(from), file).slice(repo.length + 1);
	const anchor = hash ? `#${hash}` : '';
	if (pages[path]) {
		return `${base}/docs/${pages[path].slug}/${anchor}`;
	}
	if (path === 'README.md') {
		return hash && readmeAnchors[hash] ? `${base}/docs/${readmeAnchors[hash]}/` : `${base}/`;
	}
	return `${github}/${/\.[a-z]+$/i.test(path) ? 'blob' : 'tree'}/main/${path}${anchor}`;
}

/** GitHub `> [!NOTE]` blocks → Starlight asides. */
function asides(markdown) {
	const kinds = { NOTE: 'note', TIP: 'tip', IMPORTANT: 'caution', WARNING: 'caution', CAUTION: 'danger' };
	return markdown.replace(/^> \[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\n((?:>.*\n?)*)/gm,
		(_m, kind, body) => `:::${kinds[kind]}\n${body.replace(/^> ?/gm, '').trimEnd()}\n:::\n`);
}

rmSync(join(out, 'guides'), { recursive: true, force: true });
rmSync(join(out, 'reference'), { recursive: true, force: true });
for (const [source, page] of Object.entries(pages)) {
	let markdown = readFileSync(join(repo, source), 'utf8');
	markdown = markdown.replace(/^# .+\n+/, '');
	markdown = asides(markdown);
	// Links outside code: [text](target)
	markdown = markdown.split(/(```[\s\S]*?```)/).map((part, i) =>
		i % 2 ? part : part.replace(/\[([^\]]*)\]\(([^)\s]+)\)/g, (_m, text, target) => {
			// A link written as a file name (`../llms.txt`, agents.md) reads as the page title on the site.
			const page = pages[resolve(repo, dirname(source), target.split('#')[0]).slice(repo.length + 1)];
			const label = page && /^`?[\w./-]+\.(md|txt)`?$/.test(text) ? page.title : text;
			return `[${label}](${link(target, source)})`;
		})).join('');
	const front = ['---', `title: ${JSON.stringify(page.title)}`, `editUrl: ${github}/edit/main/${source}`,
		'sidebar:', `  order: ${page.order}`, '---', '', ''].join('\n');
	const file = join(out, `${page.slug}.md`);
	mkdirSync(dirname(file), { recursive: true });
	writeFileSync(file, front + markdown);
}
// The installer, served at /co-review/install.sh for `curl -fsSL … | bash`.
copyFileSync(join(repo, 'install.sh'), join(site, 'public', 'install.sh'));
console.log(`synced ${Object.keys(pages).length} pages from docs/ and llms.txt`);

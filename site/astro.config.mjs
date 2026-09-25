// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLlmsTxt from 'starlight-llms-txt';

// Deployed by .github/workflows/site.yml to GitHub Pages.
export default defineConfig({
	site: 'https://sdsvn.github.io',
	base: '/co-review',
	integrations: [
		starlight({
			title: 'Co-Review',
			description: 'Review code and designs with Claude Code as your co-reviewer.',
			logo: { src: './src/assets/logo.svg' },
			favicon: '/favicon.svg',
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/sdsvn/co-review' }],
			// Light only, with the landing page's header links (see src/components).
			components: {
				ThemeProvider: './src/components/ThemeProvider.astro',
				ThemeSelect: './src/components/ThemeSelect.astro',
				SocialIcons: './src/components/SocialIcons.astro'
			},
			// Code blocks as on the landing page: dark ink, rounded, Geist Mono.
			expressiveCode: {
				themes: ['github-dark-default'],
				// One theme for the light-only site: don't tie it to Starlight's dark-mode switch or UI colors.
				useStarlightDarkModeSwitch: false,
				useStarlightUiThemeColors: false,
				styleOverrides: {
					borderRadius: '10px',
					borderColor: '#1e1b4b',
					codeBackground: '#1e1b4b',
					codeFontFamily: "'Geist Mono Variable', ui-monospace, monospace",
					codeFontSize: '13.5px',
					uiFontFamily: "'Geist Variable', system-ui, sans-serif",
					frames: {
						editorTabBarBackground: '#15123d',
						editorActiveTabBackground: '#1e1b4b',
						terminalTitlebarBackground: '#15123d',
						terminalBackground: '#1e1b4b',
						frameBoxShadowCssValue: 'none'
					}
				}
			},
			customCss: [
				'@fontsource-variable/geist',
				'@fontsource-variable/geist-mono',
				'./src/styles/docs.css'
			],
			// llms.txt, llms-full.txt and llms-small.txt for agents, generated from these docs.
			plugins: [
				starlightLlmsTxt({
					projectName: 'Co-Review',
					description: 'Co-Review is a review app with IDE features: an agent such as Claude Code opens its work as a review, the human comments on code, designs and diagrams, and the agent answers in the threads through MCP tools and waits for the verdict.',
					details: [
						'- Agents connect through the MCP server `co-review mcp` (tools: open_review, await_comment, reply, add_findings, ask_reviewer, await_review, get_review). The Agent contract page is the reference.',
						'- Claude Code: install the plugin with `/plugin marketplace add sdsvn/co-review` and `/plugin install co-review@co-review`.',
						'- Design documents declare `co-review: design` frontmatter; the Design documents page is the format.'
					].join('\n'),
					customSets: [
						{ label: 'Agent integration', description: 'what an agent needs to run a review: setup, the tool contract and the design-document format', paths: ['docs/start/claude-code', 'docs/guides/connect-your-agent', 'docs/guides/agents', 'docs/guides/design-docs', 'docs/reference/agent-contract'] }
					],
					promote: ['docs/start/overview', 'docs/reference/agent-contract'],
					demote: ['docs/reference/architecture', 'docs/guides/running-and-packaging'],
					exclude: ['docs/reference/architecture', 'docs/guides/running-and-packaging']
				})
			],
			sidebar: [
				{ label: 'Get started', items: [{ autogenerate: { directory: 'docs/start' } }] },
				{ label: 'Guides', items: [{ autogenerate: { directory: 'docs/guides' } }] },
				{ label: 'Reference', items: [{ autogenerate: { directory: 'docs/reference' } }] }
			]
		})
	]
});

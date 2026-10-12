#!/usr/bin/env node
/**
 * Generates conformance fixtures for the review_design crate.
 *
 * Runs the compiled TypeScript design-format.js over a corpus of inputs and writes
 * each case as a JSON file under ../fixtures/. The Rust test replays them.
 *
 * Usage: node gen.mjs
 */

import { readFileSync, writeFileSync, mkdirSync, readdirSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = join(__dirname, '..', '..', '..', '..');

// Import the compiled TS
const df = await import(join(root, 'extensions/review/lib/common/design-format.js'));

// Also grab stepId from the renderer
const dr = await import(join(root, 'extensions/review/lib/browser/document/document-render.js')).catch(() => null);

const fixturesDir = join(__dirname, '..', 'fixtures');
mkdirSync(fixturesDir, { recursive: true });

// Utility: convert a Step tree to plain JSON (drop circular/class info)
function stepsToJson(steps) {
    return steps.map(s => ({
        text: s.text,
        indent: s.indent,
        depth: s.depth,
        children: stepsToJson(s.children),
    }));
}

// Extract fenced markdown examples from design-docs.md
function extractExamples(docPath) {
    const content = readFileSync(docPath, 'utf8');
    const examples = [];
    // Match ````markdown ... ```` or ```markdown ... ```
    const re = /^(`{3,4})markdown\s*\n([\s\S]*?)^\1\s*$/gm;
    let m;
    while ((m = re.exec(content)) !== null) {
        const md = m[2];
        // Only include those that look like design documents (have frontmatter or ## Design)
        if (md.includes('co-review: design') || md.includes('## Design')) {
            examples.push(md);
        }
    }
    return examples;
}

// Build the corpus
const cases = [];
let caseId = 0;

function addCase(name, markdown, fileName) {
    const id = `${String(caseId++).padStart(3, '0')}_${name}`;
    const { data, body } = df.splitFrontmatter(markdown);
    const sectionsResult = df.sectionsOf(body);
    const format = df.documentFormat(markdown, fileName);

    // Build step trees for tree sections
    const stepTrees = {};
    for (const section of sectionsResult.sections) {
        const idx = df.designSectionIndex(section.key);
        const def = idx >= 0 ? df.DESIGN_SECTIONS[idx] : null;
        if (def?.tree || section.key === 'design' || section.key === 'open questions') {
            stepTrees[section.key] = stepsToJson(df.steps(section.body));
        }
    }

    // Step IDs (if stepId is available)
    const allSteps = [];
    const walk = list => list.forEach(s => { allSteps.push(s); walk(s.children); });
    if (stepTrees.design) walk(stepTrees.design);

    const stepIds = dr ? allSteps.map(s => dr.stepId(s.text)) : allSteps.map(s =>
        (s.text || '').replace(/[`*]/g, '').replace(/\s+/g, ' ').trim().slice(0, 80)
    );

    const fixture = {
        name,
        input: { markdown, fileName },
        expected: {
            frontmatter: { data, body },
            sections: sectionsResult,
            format,
            stepTrees,
            stepIds,
        },
    };

    writeFileSync(join(fixturesDir, `${id}.json`), JSON.stringify(fixture, null, 2) + '\n');
    return id;
}

// --- 1. Examples from docs/design-docs.md ---
const docExamples = extractExamples(join(root, 'docs/design-docs.md'));
docExamples.forEach((md, i) => addCase(`doc_example_${i}`, md, 'index.markdown'));

// --- 2. Hand-written edge cases ---

// Minimal valid design
addCase('minimal_valid', `---
co-review: design
---
# Title
## Design
- Step one
`, 'index.markdown');

// Unknown co-review value
addCase('unknown_format', `---
co-review: review
---
# Title
## Design
- Step one
`, 'index.markdown');

// Missing title
addCase('missing_title', `---
co-review: design
---
## Design
- Step one
`, 'index.markdown');

// Title in frontmatter
addCase('title_in_frontmatter', `---
co-review: design
title: My Title
---
## Design
- Step one
`, 'index.markdown');

// Missing Design section
addCase('missing_design', `---
co-review: design
---
# Title
## Context
Some context.
`, 'index.markdown');

// Empty Design (no steps)
addCase('empty_design', `---
co-review: design
---
# Title
## Design
Just some text but no steps.
`, 'index.markdown');

// Design with wrong markers only (*, +, numbered)
addCase('wrong_markers_only', `---
co-review: design
---
# Title
## Design
* Wrong item one
+ Wrong item two
1. Numbered item
`, 'index.markdown');

// Design mixing - with wrong markers
addCase('mixed_markers', `---
co-review: design
---
# Title
## Design
- Good step
* Bad step
`, 'index.markdown');

// Extra sections
addCase('extra_sections', `---
co-review: design
---
# Title
## Context
Some context.
## Design
- Step one
## Implementation
Details here.
## Testing
Test plan.
`, 'index.markdown');

// Sections out of order
addCase('out_of_order', `---
co-review: design
---
# Title
## Design
- Step one
## Context
Late context.
`, 'index.markdown');

// Behavior spelling (alias)
addCase('behavior_alias', `---
co-review: design
---
# Title
## Design
- Step one
## Behavior
Some behavior.
`, 'index.markdown');

// Behaviour spelling
addCase('behaviour_spelling', `---
co-review: design
---
# Title
## Design
- Step one
## Behaviour
Some behaviour.
`, 'index.markdown');

// Line references
addCase('line_references', `---
co-review: design
---
# Title
## Design
- Step one
## Context
See handler.go:42 and also router.ts:100 for context. Also #L55 is relevant.
`, 'index.markdown');

// Line references inside code blocks should not be detected
addCase('line_refs_in_code', `---
co-review: design
---
# Title
## Design
- Step one
## Context
\`\`\`mermaid
%% id: flow
graph LR
  A[file.go:42] --> B
\`\`\`
`, 'index.markdown');

// Non-mermaid code blocks
addCase('non_mermaid_code', `---
co-review: design
---
# Title
## Design
- Step one
## Context
\`\`\`go
func main() {}
\`\`\`
`, 'index.markdown');

// Mermaid code blocks are fine
addCase('mermaid_only', `---
co-review: design
---
# Title
## Design
- Step one
## Behaviour
\`\`\`mermaid
sequenceDiagram
  A->>B: Request
\`\`\`
`, 'index.markdown');

// Long steps (>220 chars)
addCase('long_steps', `---
co-review: design
---
# Title
## Design
- This is a really long step that goes on and on and on and on and on and on and on and on and on and on and on and on and on and on and on and on and on and on about something really detailed that should be broken up
- Another overly long step that discusses many implementation details at once without using child steps to break them into individual decisions which is a problem because it makes the step harder to comment on individually
- Short step
`, 'index.markdown');

// Repeated code names (3+ steps)
addCase('repeated_code_names', `---
co-review: design
---
# Title
## Design
- Use \`RateLimiter\` to limit requests
- Configure \`RateLimiter\` for each endpoint
- Test \`RateLimiter\` behavior
- Also use \`JobRunner\` here
- Configure \`JobRunner\` there
- Wrap \`JobRunner\` in try-catch
`, 'index.markdown');

// Unicode step text (emoji hitting 80-char boundary)
addCase('unicode_step_id', `---
co-review: design
---
# Title
## Design
- Step with emoji \u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}end
`, 'index.markdown');

// CJK text in steps
addCase('cjk_step_text', `---
co-review: design
---
# Title
## Design
- \u{8FD9}\u{662F}\u{4E00}\u{4E2A}\u{4E2D}\u{6587}\u{6B65}\u{9AA4}\u{FF0C}\u{6D4B}\u{8BD5}Unicode\u{5904}\u{7406}
`, 'index.markdown');

// Continuation lines
addCase('continuation_lines', `---
co-review: design
---
# Title
## Design
- This is a step that
  continues on the next line
  and even continues here
  - Child step
`, 'index.markdown');

// Tabs in indentation
addCase('tabs_indentation', `---
co-review: design
---
# Title
## Design
- Top level
\t- Tab indented child
\t\t- Double tab grandchild
`, 'index.markdown');

// Pseudocode file (detected, not declared)
addCase('pseudocode_detected', `# Title
## Design
- Step one
  - Sub step
`, 'something.pseudocode.md');

// Undeclared with ## Design section (detected)
addCase('undeclared_with_design', `# Title
## Design
- Step one
`, 'readme.md');

// Plain file, no detection
addCase('plain_file', `# Title
## Introduction
Just text.
`, 'readme.md');

// Markers in steps
addCase('step_markers', `---
co-review: design
---
# Title
## Design
- Create new \`payment_outbox\` table \u{2014} \`\u{270e}\` new table for charge requests
- Check rate limit \u{2014} \`\u{26a0}\` provider forgets keys after 24h
- Decide on retry policy \u{2014} \`?\` open question about backoff
`, 'index.markdown');

// Reason after em dash
addCase('em_dash_reason', `---
co-review: design
---
# Title
## Design
- Save the order as "payment pending" \u{2014} new \`payment_outbox\` table
- Retry with backoff \u{2014} provider forgets keys after 24 hours
`, 'index.markdown');

// Empty document
addCase('empty_document', ``, 'index.markdown');

// Frontmatter only
addCase('frontmatter_only', `---
co-review: design
---
`, 'index.markdown');

// Multiple warnings at once
addCase('many_warnings', `---
co-review: design
---
## Implementation
Some text.
## Design
* Wrong marker
## Context
Late context.
`, 'index.markdown');

// Open Questions section as step tree
addCase('open_questions', `---
co-review: design
---
# Title
## Design
- Step one
## Open Questions
- Should we use feature flags?
- What about backward compatibility?
`, 'index.markdown');

// Diagrams section
addCase('diagrams_section', `---
co-review: design
---
# Title
## Design
- Step one
## Diagrams
\`\`\`mermaid
%% id: flow
graph LR
  A --> B
\`\`\`
`, 'index.markdown');

// Headings inside fenced code blocks should not be detected as sections
addCase('heading_in_fence', `---
co-review: design
---
# Title
## Design
- Step one
## Context
\`\`\`
## This is not a section
\`\`\`
`, 'index.markdown');

// Frontmatter with quotes
addCase('frontmatter_quotes', `---
co-review: design
title: "Quoted Title"
---
## Design
- Step one
`, 'index.markdown');

// Frontmatter with single quotes
addCase('frontmatter_single_quotes', `---
co-review: design
title: 'Single Quoted'
---
## Design
- Step one
`, 'index.markdown');

// Frontmatter with inline comment
addCase('frontmatter_comment', `---
co-review: design # this is a comment
title: Test
---
# Heading
## Design
- Step one
`, 'index.markdown');

// Backtick and bold markers in step text
addCase('markers_in_text', `---
co-review: design
---
# Title
## Design
- Use \`code\` and **bold** and *italic* in steps
  - Child with \`another\` code name
`, 'index.markdown');

// CRLF line endings
addCase('crlf_endings', "---\r\nco-review: design\r\n---\r\n# Title\r\n## Design\r\n- Step one\r\n", 'index.markdown');

// URL in prose should not trigger line ref warning
addCase('url_in_prose', `---
co-review: design
---
# Title
## Design
- Step one
## Context
See https://example.com/path/file.go:42 for docs.
`, 'index.markdown');

// Empty language code fence (should trigger non-mermaid warning)
addCase('empty_lang_fence', `---
co-review: design
---
# Title
## Design
- Step one
## Context
\`\`\`
some code
\`\`\`
`, 'index.markdown');

// Tilde fences
addCase('tilde_fences', `---
co-review: design
---
# Title
## Design
- Step one
## Context
~~~python
print("hello")
~~~
`, 'index.markdown');

// Repeated code names not reaching threshold
addCase('code_names_below_threshold', `---
co-review: design
---
# Title
## Design
- Use \`Foo\` here
- Use \`Foo\` there
- Use \`Bar\` once
`, 'index.markdown');

// Deeply nested steps
addCase('deep_nesting', `---
co-review: design
---
# Title
## Design
- Level 1
  - Level 2
    - Level 3
      - Level 4
        - Level 5
`, 'index.markdown');

// All sections present and in order
addCase('all_sections', `---
co-review: design
---
# Title
## Context
Context text.
## Design
- Step one
## Alternatives
Alt text.
## Behaviour
Behaviour text.
## Diagrams
Diagram text.
## Open Questions
- Question one
`, 'index.markdown');

// Heading with trailing hashes
addCase('heading_trailing_hashes', `---
co-review: design
---
# Title
## Design ##
- Step one
## Context ###
Context.
`, 'index.markdown');

// Mismatched fence types: ``` opened, ~~~ in the middle, ``` closes
addCase('mismatched_fences', `---
co-review: design
---
# Title
## Design
- Step one
## Context
\`\`\`
some code
~~~
still in fence?
\`\`\`
After fence.
`, 'index.markdown');

// Mismatched fences reversed: ~~~ opened, ``` in the middle, ~~~ closes
addCase('mismatched_fences_reversed', `---
co-review: design
---
# Title
## Design
- Step one
## Context
~~~
some code
\`\`\`
still in fence?
~~~
After fence.
`, 'index.markdown');

console.log(`Generated ${caseId} fixtures in ${fixturesDir}`);

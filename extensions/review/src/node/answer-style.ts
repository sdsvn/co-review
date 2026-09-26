/**
 * How agents answer a reviewer, shared by every way an agent joins a review (ACP prompt, MCP instructions,
 * tool results). A reviewer asking in a thread wants what a colleague would say, quickly — not a report.
 */
export const ANSWER_STYLE = [
    'How to answer: the reviewer is a person reading a chat thread. Write the way a knowledgeable colleague would reply.',
    '- Answer the question in the first sentence, in plain language.',
    '- Keep it short: a few sentences, or a short list when there are steps. No headings, tables or long code blocks.',
    '- Explain what the code does and why in words. Don\'t walk through file paths and line numbers; if a pointer helps, '
    + 'end with one or two links like path/to/file.ext:42.',
    '- Be quick: read only what you need to answer. If you are not sure, say so briefly instead of exploring everything.'
].join('\n');

/** One line of ANSWER_STYLE, for tool descriptions and results. */
export const ANSWER_STYLE_SHORT = 'Answer first, in plain language, in a few sentences; no line-number walkthroughs '
    + '(at most one or two path:line links at the end). Read only what you need, so the answer comes fast.';

/**
 * Co-Review for Oh My Pi (omp). The extension itself is shared with Pi (../shared/co-review.ts); this
 * file only says how omp differs:
 *
 * - omp rebinds extensions into every task subagent, so only the main session listens and honors
 *   `--co-review`; a subagent opens a review only when it calls `co_review_start` itself.
 * - A reviewer question is attributed to the agent side, not recorded as something the user typed.
 * - Tool schemas use omp's TypeBox-compatible builder (`pi.typebox`).
 */
import type { ExtensionAPI, ExtensionContext } from "@oh-my-pi/pi-coding-agent";
import { registerCoReview } from "../shared/co-review.ts";

export default function coReview(pi: ExtensionAPI) {
	registerCoReview<ExtensionContext>(pi, {
		name: "Oh My Pi",
		Type: pi.typebox.Type,
		deliver: (ctx, prompt) => pi.sendUserMessage(prompt, { attribution: "agent", ...(ctx.isIdle() ? {} : { deliverAs: "followUp" }) }),
		isMain: ctx => ctx.agent?.kind !== "sub",
		subagentGuideline: "To keep co-reviewing while you work on something else, spawn a task subagent that loops co_review_wait and co_review_reply until the reviewer ends the review; tell it to answer briefly and quickly."
	});
}

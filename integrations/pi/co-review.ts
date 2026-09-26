/**
 * Co-Review for Pi. The extension itself is shared with Oh My Pi (../shared/co-review.ts); this file
 * only says how Pi differs: TypeBox from Pi's `typebox`, and a question waits until Pi is free.
 */
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { registerCoReview } from "../shared/co-review.ts";

export default function coReview(pi: ExtensionAPI) {
	registerCoReview<ExtensionContext>(pi, {
		name: "Pi",
		Type,
		deliver: (ctx, prompt) => pi.sendUserMessage(prompt, ctx.isIdle() ? undefined : { deliverAs: "followUp" }),
		isMain: () => true,
		subagentGuideline: "Without a UI (e.g. as a subagent), loop co_review_wait and co_review_reply until the reviewer ends the review."
	});
}

import { Command } from '@theia/core/lib/common/command';

export namespace ReviewCommands {
    const category = 'Review';
    export const TOGGLE_PANEL: Command = { id: 'co-review.togglePanel', category, label: 'Toggle Review Panel' };
    export const CREATE_REVIEW: Command = { id: 'co-review.createReview', category, label: 'New Review…' };
    export const RENAME_REVIEW: Command = { id: 'co-review.renameReview', category, label: 'Rename Review…' };
    export const DELETE_REVIEW: Command = { id: 'co-review.deleteReview', category, label: 'Delete Review…' };
    export const COMMENT_SELECTION: Command = { id: 'co-review.commentSelection', category, label: 'Add Review Comment' };
    export const ASK_SELECTION: Command = { id: 'co-review.askSelection', category, label: 'Ask Agent About Selection' };
    export const CONFIGURE_AGENT: Command = { id: 'co-review.configureAgent', category, label: 'Connect ACP Agent…' };
    export const AGENT_SETTINGS: Command = { id: 'co-review.agentSettings', category, label: 'Choose Agent Model…' };
    export const TOGGLE_VIEWED: Command = { id: 'co-review.toggleViewed', category, label: 'Mark File as Viewed / Not Viewed' };
    /** The same for the file or folder selected in the explorer. */
    export const TOGGLE_VIEWED_PATH: Command = { id: 'co-review.toggleViewedPath', category, label: 'Mark as Viewed / Not Viewed' };
    export const OPEN_OVERVIEW: Command = { id: 'co-review.openOverview', category, label: 'Open Repository Overview' };
    export const REVIEW_ACTIONS: Command = { id: 'co-review.reviewActions', category, label: 'Review Actions…' };
    export const COLLAPSE_ALL: Command = { id: 'co-review.collapseAll', category, label: 'Collapse All Comments' };
    export const EXPAND_ALL: Command = { id: 'co-review.expandAll', category, label: 'Expand All Comments' };
    export const NEXT_COMMENT: Command = { id: 'co-review.nextComment', category, label: 'Go to Next Comment' };
    export const PREVIOUS_COMMENT: Command = { id: 'co-review.previousComment', category, label: 'Go to Previous Comment' };
    export const GO_TO_COMMENT: Command = { id: 'co-review.goToComment', category, label: 'Go to Comment…' };
    export const COMMENT_SYMBOL: Command = { id: 'co-review.commentSymbol', category, label: 'Add Review Comment on Symbol' };
    export const COMMENT_PATH: Command = { id: 'co-review.commentPath', category, label: 'Add Review Comment' };
    export const COMMENT_REPOSITORY: Command = { id: 'co-review.commentRepository', category, label: 'Add Repository Comment' };
    export const REVIEW_PATHS: Command = { id: 'co-review.reviewPaths', category, label: 'Start Review of Selection' };
    export const INSTALL_SKILLS: Command = { id: 'co-review.installSkills', category, label: 'Install Agent Skills…' };
    export const INSTALL_CLI: Command = { id: 'co-review.installCli', category, label: 'Install the co-review Command' };
    export const SETUP_HARNESS: Command = { id: 'co-review.setupHarness', category, label: 'Add Co-Review to an Agent Harness (MCP)…' };
}

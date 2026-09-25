import { ContributionFilterRegistry, FilterContribution } from '@theia/core/lib/common/contribution-filter';
import { injectable } from '@theia/core/shared/inversify';
import { DebugConsoleContribution } from '@theia/debug/lib/browser/console/debug-console-contribution';
import { DebugFrontendApplicationContribution } from '@theia/debug/lib/browser/debug-frontend-application-contribution';
import { TaskFrontendContribution } from '@theia/task/lib/browser/task-frontend-contribution';
import { TestOutputViewContribution } from '@theia/test/lib/browser/view/test-output-view-contribution';
import { TestResultViewContribution } from '@theia/test/lib/browser/view/test-result-view-contribution';
import { TestRunViewContribution } from '@theia/test/lib/browser/view/test-run-view-contribution';
import { TestViewContribution } from '@theia/test/lib/browser/view/test-view-contribution';

/**
 * Co-Review is a review tool with IDE features, not an IDE: drop the parts of the workbench that
 * are about running and building code (debugging, test runners, tasks). They arrive transitively
 * with VS Code extension support.
 */
const IDE_ONLY = [
    DebugFrontendApplicationContribution, DebugConsoleContribution,
    TestViewContribution, TestRunViewContribution, TestResultViewContribution, TestOutputViewContribution,
    TaskFrontendContribution
];

@injectable()
export class ReviewShellFilter implements FilterContribution {
    registerContributionFilters(registry: ContributionFilterRegistry): void {
        registry.addFilters('*', [contribution => !IDE_ONLY.some(type => contribution instanceof type)]);
    }
}

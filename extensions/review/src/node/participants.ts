import { FileUri } from '@theia/core/lib/common/file-uri';
import { execFile } from 'child_process';
import * as os from 'os';
import { Participant } from '../common/review-model';

function git(cwd: string, args: string[]): Promise<string | undefined> {
    return new Promise(resolve => execFile('git', args, { cwd }, (err, stdout) => resolve(err ? undefined : stdout.trim())));
}

/** The human reviewer, from the repository's git config (falls back to the OS user). */
export async function currentUser(workspaceRoot: string): Promise<Participant> {
    const cwd = FileUri.fsPath(workspaceRoot);
    const email = await git(cwd, ['config', 'user.email']);
    const name = await git(cwd, ['config', 'user.name']) || os.userInfo().username;
    return { id: `human:${email || name}`, kind: 'human', name };
}

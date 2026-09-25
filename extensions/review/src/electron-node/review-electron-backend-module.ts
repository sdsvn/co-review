import { ElectronTokenValidator } from '@theia/core/lib/electron-node/token/electron-token-validator';
import { ContainerModule, injectable } from '@theia/core/shared/inversify';
import * as http from 'http';

/**
 * The desktop backend only admits its own windows (a per-launch token cookie). Agents (`/mcp`) and
 * the phone view (`/m`, `/api/m`) cannot carry that token; they have their own local-host guards,
 * as in the browser app. Everything else still requires the token.
 */
@injectable()
export class ReviewTokenValidator extends ElectronTokenValidator {
    override allowRequest(request: http.IncomingMessage): boolean {
        return /^\/(mcp|m|api\/m)(\/|\?|$)/.test(request.url ?? '') || super.allowRequest(request);
    }
}

export default new ContainerModule((bind, _unbind, _isBound, rebind) => {
    bind(ReviewTokenValidator).toSelf().inSingletonScope();
    rebind(ElectronTokenValidator).toService(ReviewTokenValidator);
});

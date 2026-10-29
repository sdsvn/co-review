# Third-party notices

Co-Review's own code is under the [MIT License](LICENSE). The desktop app and the browser app are built from
unmodified open-source components under their own licenses. Each npm package ships its license text in its folder
under `node_modules`, which is included in the app as it is.

| Component | License | Source |
|---|---|---|
| [Eclipse Theia](https://theia-ide.org) (`@theia/*` packages) | EPL-2.0 OR GPL-2.0 WITH Classpath-exception-2.0 | https://github.com/eclipse-theia/theia |
| [Monaco Editor](https://github.com/microsoft/monaco-editor) | MIT | https://github.com/microsoft/vscode |
| [Electron](https://www.electronjs.org) (with Chromium and Node.js) | MIT; Chromium's licenses are in `LICENSES.chromium.html` in the app | https://github.com/electron/electron |
| Built-in VS Code extensions (`eclipse-theia.builtin-extension-pack`, from Open VSX) | MIT | https://github.com/eclipse-theia/vscode-builtin-extensions |
| [Go extension](https://github.com/golang/vscode-go) (`golang.Go`, from Open VSX) | MIT | https://github.com/golang/vscode-go |
| [jschardet](https://github.com/aadsm/jschardet) | LGPL-2.1-or-later | https://github.com/aadsm/jschardet |
| [DOMPurify](https://github.com/cure53/DOMPurify) | MPL-2.0 OR Apache-2.0 | https://github.com/cure53/DOMPurify |

Co-Review uses these components unmodified, as npm and Open VSX packages; the source of each version is available
from the links above. The other npm dependencies are under permissive licenses (mostly MIT, ISC, BSD and
Apache-2.0); `npm ls --all` lists them and each package's `license` field names its license.

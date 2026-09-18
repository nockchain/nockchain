import * as vscode from 'vscode';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from 'vscode-languageclient/node';

import { parseShowReferencesArguments } from './codeLens';
import {
  CODE_LENS_SECTION,
  HonkSettings,
  SERVER_SETTINGS,
  readCodeLensSettings,
  resolveServerCommand,
  serverArguments,
} from './config';

let client: LanguageClient | undefined;
let output: vscode.LogOutputChannel | undefined;
let watcher: vscode.FileSystemWatcher | undefined;

function readSettings(): HonkSettings {
  const config = vscode.workspace.getConfiguration('honk');
  return {
    serverPath: config.get('server.path', ''),
    preludePath: config.get('preludePath', ''),
    dependenciesPath: config.get('dependenciesPath', ''),
    entryPath: config.get('entryPath', ''),
    subjectTypeJamPath: config.get('subjectTypeJamPath', ''),
    dbug: config.get('dbug', true),
    vet: config.get('vet', true),
    checkDelayMilliseconds: config.get('checkDelayMilliseconds', 150),
    maxChecks: config.get('maxChecks', 256),
    workerStackBytes: config.get('workerStackBytes', 4_294_967_296),
  };
}

async function startClient(context: vscode.ExtensionContext): Promise<void> {
  if (client) {
    return;
  }
  const workspaceFolder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const settings = readSettings();
  const command = resolveServerCommand(settings.serverPath, context.extensionPath, workspaceFolder);
  const args = serverArguments(settings, workspaceFolder);
  output ??= vscode.window.createOutputChannel('Honk Language Server', { log: true });

  const executable = { command, args, transport: TransportKind.stdio };
  const serverOptions: ServerOptions = { run: executable, debug: executable };
  if (!watcher) {
    watcher = vscode.workspace.createFileSystemWatcher('**/*.{hoon,jam}');
    context.subscriptions.push(watcher);
  }
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ language: 'hoon', scheme: 'file' }],
    synchronize: { fileEvents: watcher },
    outputChannel: output,
    initializationOptions: {
      checkDelayMs: settings.checkDelayMilliseconds,
      codeLens: readCodeLensSettings(vscode.workspace.getConfiguration('honk')),
    },
  };
  client = new LanguageClient(
    'honk',
    'Honk Hoon Language Server',
    serverOptions,
    clientOptions,
  );
  try {
    await client.start();
  } catch (error) {
    client = undefined;
    const message = error instanceof Error ? error.message : String(error);
    void vscode.window.showErrorMessage(`Failed to start honk-lsp: ${message}`);
  }
}

function toPosition(position: { line: number; character: number }): vscode.Position {
  return new vscode.Position(position.line, position.character);
}

/**
 * Open the references peek for a resolved reference-count lens.
 *
 * The server emits `honk.showReferences` with LSP-typed JSON arguments, which
 * `editor.action.showReferences` does not accept directly; convert them into
 * editor types first.
 */
async function showReferences(...args: unknown[]): Promise<void> {
  const parsed = parseShowReferencesArguments(args);
  if (!parsed) {
    output?.warn(`honk.showReferences received malformed arguments: ${JSON.stringify(args)}`);
    return;
  }
  const locations = parsed.locations.map(
    (location) =>
      new vscode.Location(
        vscode.Uri.parse(location.uri),
        new vscode.Range(toPosition(location.range.start), toPosition(location.range.end)),
      ),
  );
  await vscode.commands.executeCommand(
    'editor.action.showReferences',
    vscode.Uri.parse(parsed.uri),
    toPosition(parsed.position),
    locations,
  );
}

async function restartClient(context: vscode.ExtensionContext): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
  await startClient(context);
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  output = vscode.window.createOutputChannel('Honk Language Server', { log: true });
  context.subscriptions.push(output);
  context.subscriptions.push(
    vscode.commands.registerCommand('honk.restartServer', () => restartClient(context)),
    vscode.commands.registerCommand('honk.showOutput', () => output?.show()),
    vscode.commands.registerCommand('honk.showReferences', showReferences),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (SERVER_SETTINGS.some((setting) => event.affectsConfiguration(setting))) {
        void restartClient(context);
      } else if (event.affectsConfiguration(CODE_LENS_SECTION) && client) {
        // Lens settings retune live: the server refreshes the lenses itself.
        void client.sendNotification('workspace/didChangeConfiguration', {
          settings: {
            honk: { codeLens: readCodeLensSettings(vscode.workspace.getConfiguration('honk')) },
          },
        });
      }
    }),
  );
  await startClient(context);
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
  watcher = undefined;
}

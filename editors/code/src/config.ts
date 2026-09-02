import * as fs from 'node:fs';
import * as path from 'node:path';

export interface HonkSettings {
  serverPath: string;
  preludePath: string;
  dependenciesPath: string;
  entryPath: string;
  subjectTypeJamPath: string;
  dbug: boolean;
  vet: boolean;
  checkDelayMilliseconds: number;
  maxChecks: number;
  workerStackBytes: number;
}

export function expandWorkspacePath(value: string, workspaceFolder?: string): string {
  const expanded = workspaceFolder
    ? value.replaceAll('${workspaceFolder}', workspaceFolder)
    : value;
  if (!expanded || path.isAbsolute(expanded) || !workspaceFolder) {
    return expanded;
  }
  return path.join(workspaceFolder, expanded);
}

export function resolveServerCommand(
  configured: string,
  extensionRoot: string,
  workspaceFolder?: string,
): string {
  const explicit = expandWorkspacePath(configured.trim(), workspaceFolder);
  if (explicit) {
    return explicit;
  }
  const executable = process.platform === 'win32' ? 'honk-lsp.exe' : 'honk-lsp';
  const candidates = [
    path.join(extensionRoot, 'server', process.platform, process.arch, executable),
    workspaceFolder ? path.join(workspaceFolder, 'target', 'release', executable) : '',
    workspaceFolder ? path.join(workspaceFolder, 'target', 'debug', executable) : '',
  ];
  return candidates.find((candidate) => candidate && fs.existsSync(candidate)) ?? executable;
}

function addPathArgument(
  args: string[],
  flag: string,
  value: string,
  workspaceFolder?: string,
): void {
  const expanded = expandWorkspacePath(value.trim(), workspaceFolder);
  if (expanded) {
    args.push(flag, expanded);
  }
}

export function serverArguments(settings: HonkSettings, workspaceFolder?: string): string[] {
  const args: string[] = [];
  addPathArgument(args, '--prelude', settings.preludePath, workspaceFolder);
  addPathArgument(args, '--deps-dir', settings.dependenciesPath, workspaceFolder);
  addPathArgument(args, '--entry', settings.entryPath, workspaceFolder);
  addPathArgument(args, '--sut-jam', settings.subjectTypeJamPath, workspaceFolder);
  if (!settings.dbug) {
    args.push('--no-dbug');
  }
  if (!settings.vet) {
    args.push('--no-vet');
  }
  args.push('--check-delay-ms', String(Math.max(0, Math.trunc(settings.checkDelayMilliseconds))));
  args.push('--max-compiles', String(Math.max(0, Math.trunc(settings.maxChecks))));
  args.push('--worker-stack-bytes', String(Math.max(1_048_576, Math.trunc(settings.workerStackBytes))));
  return args;
}

/**
 * Arguments of the `honk.showReferences` command that resolved reference
 * lenses carry, as the language server sends them: LSP-typed, JSON-shaped.
 */
export interface LspPosition {
  line: number;
  character: number;
}

export interface LspRange {
  start: LspPosition;
  end: LspPosition;
}

export interface LspLocation {
  uri: string;
  range: LspRange;
}

export interface ShowReferencesArguments {
  uri: string;
  position: LspPosition;
  locations: LspLocation[];
}

function isNonNegativeInteger(value: unknown): value is number {
  return typeof value === 'number' && Number.isInteger(value) && value >= 0;
}

function isPosition(value: unknown): value is LspPosition {
  return (
    typeof value === 'object' &&
    value !== null &&
    isNonNegativeInteger((value as LspPosition).line) &&
    isNonNegativeInteger((value as LspPosition).character)
  );
}

function isLocation(value: unknown): value is LspLocation {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const location = value as LspLocation;
  return (
    typeof location.uri === 'string' &&
    location.uri.length > 0 &&
    typeof location.range === 'object' &&
    location.range !== null &&
    isPosition(location.range.start) &&
    isPosition(location.range.end)
  );
}

/**
 * Validate the raw command arguments before they are converted into editor
 * types. A malformed lens must fail here with a clear message rather than
 * inside VS Code's references peek.
 */
export function parseShowReferencesArguments(args: unknown[]): ShowReferencesArguments | undefined {
  if (args.length !== 3) {
    return undefined;
  }
  const [uri, position, locations] = args;
  if (typeof uri !== 'string' || uri.length === 0 || !isPosition(position)) {
    return undefined;
  }
  if (!Array.isArray(locations) || !locations.every(isLocation)) {
    return undefined;
  }
  return { uri, position, locations };
}

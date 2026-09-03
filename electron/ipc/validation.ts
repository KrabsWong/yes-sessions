import type { AppType } from '../../src/types';
import { APP_ORDER } from '../../src/config/apps';
import { errors } from '../utils/errors';

type ArgValidator = (value: unknown, index: number) => void;
type ResultValidator = (value: unknown) => void;

const APP_TYPES = new Set<AppType>(APP_ORDER);

function describeArg(index: number, name?: string): string {
  return name || `arg${index}`;
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isBoolean(value: unknown): value is boolean {
  return typeof value === 'boolean';
}

function isNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function assertResult(condition: boolean, resultName: string): void {
  if (!condition) {
    throw errors.validationFailed(`Invalid IPC result: ${resultName}`);
  }
}

export function validateArgs(...validators: ArgValidator[]) {
  return (args: unknown[]): void => {
    validators.forEach((validator, index) => {
      validator(args[index], index);
    });
  };
}

export function stringArg(name?: string): ArgValidator {
  return (value, index) => {
    if (typeof value !== 'string' || value.length === 0) {
      throw errors.invalidInput(describeArg(index, name), 'expected a non-empty string');
    }
  };
}

export function optionalStringArg(name?: string): ArgValidator {
  return (value, index) => {
    if (value !== undefined && typeof value !== 'string') {
      throw errors.invalidInput(describeArg(index, name), 'expected a string when provided');
    }
  };
}

export function recordArg(name?: string): ArgValidator {
  return (value, index) => {
    if (!isPlainRecord(value)) {
      throw errors.invalidInput(describeArg(index, name), 'expected an object');
    }
  };
}

export function appTypeArg(name?: string): ArgValidator {
  return (value, index) => {
    if (typeof value !== 'string' || !APP_TYPES.has(value as AppType)) {
      throw errors.invalidInput(describeArg(index, name), 'expected a supported app type');
    }
  };
}

function isTreeNode(value: unknown): boolean {
  if (!isPlainRecord(value)) return false;
  if (!isString(value.name) || !isString(value.path)) return false;
  if (value.type !== 'file' && value.type !== 'directory') return false;
  if (value.children !== undefined) {
    if (!Array.isArray(value.children)) return false;
    return value.children.every(isTreeNode);
  }
  return true;
}

function isGitFileChange(value: unknown): boolean {
  if (!isPlainRecord(value)) return false;
  const validStatuses = new Set([
    'added',
    'modified',
    'deleted',
    'renamed',
    'untracked',
    'unknown',
  ]);
  return (
    isString(value.path) &&
    isString(value.status) &&
    validStatuses.has(value.status) &&
    isNumber(value.additions) &&
    isNumber(value.deletions)
  );
}

function isSessionMessage(value: unknown): boolean {
  if (!isPlainRecord(value)) return false;
  const validMessageTypes = new Set(['user', 'assistant', 'tool_use', 'tool_result', 'system']);

  const hasValidBaseShape =
    isString(value.type) &&
    validMessageTypes.has(value.type) &&
    isString(value.timestamp) &&
    (value.content === undefined || isString(value.content)) &&
    (value.reasoning_content === undefined || isString(value.reasoning_content)) &&
    (value.redacted_content === undefined || isString(value.redacted_content)) &&
    (value.sub_agent_session_id === undefined || isString(value.sub_agent_session_id)) &&
    (value.tool_name === undefined || isString(value.tool_name)) &&
    (value.tool_input === undefined || isPlainRecord(value.tool_input)) &&
    (value.tool_output === undefined || isPlainRecord(value.tool_output)) &&
    (value.callId === undefined || isString(value.callId)) &&
    (value.metadata === undefined || isPlainRecord(value.metadata)) &&
    (value.model === undefined || isString(value.model));

  if (!hasValidBaseShape) return false;

  switch (value.type) {
    case 'user':
    case 'system':
      return isString(value.content) || isString(value.redacted_content);
    case 'assistant':
      return (
        isString(value.content) ||
        isString(value.reasoning_content) ||
        isString(value.redacted_content)
      );
    case 'tool_use':
      return isString(value.tool_name) && isPlainRecord(value.tool_input);
    case 'tool_result':
      return isString(value.tool_name) && isPlainRecord(value.tool_output);
    default:
      return false;
  }
}

function isSession(value: unknown): boolean {
  if (!isPlainRecord(value)) return false;
  return (
    isString(value.id) &&
    isString(value.appType) &&
    isString(value.fileName) &&
    isString(value.filePath) &&
    isNumber(value.createdAt) &&
    isNumber(value.updatedAt) &&
    isNumber(value.messageCount) &&
    (value.firstMessage === undefined || isString(value.firstMessage)) &&
    (value.lastMessage === undefined || isString(value.lastMessage)) &&
    (value.directory === undefined || isString(value.directory)) &&
    (value.uuid === undefined || isString(value.uuid)) &&
    (value.kind === undefined || value.kind === 'main' || value.kind === 'subagent') &&
    (value.parentSessionId === undefined || isString(value.parentSessionId)) &&
    (value.agentType === undefined || isString(value.agentType))
  );
}

export function treeNodesResult(): ResultValidator {
  return (value) => {
    assertResult(Array.isArray(value) && value.every(isTreeNode), 'TreeNode[]');
  };
}

export function gitStatusResult(): ResultValidator {
  return (value) => {
    assertResult(
      isPlainRecord(value) &&
        isBoolean(value.isGitRepo) &&
        isString(value.branch) &&
        isNumber(value.ahead) &&
        isNumber(value.behind) &&
        Array.isArray(value.staged) &&
        value.staged.every(isGitFileChange) &&
        Array.isArray(value.unstaged) &&
        value.unstaged.every(isGitFileChange) &&
        Array.isArray(value.untracked) &&
        value.untracked.every(isGitFileChange),
      'GitStatusResult'
    );
  };
}

export function gitFileDiffResult(): ResultValidator {
  return (value) => {
    assertResult(
      isPlainRecord(value) &&
        isString(value.original) &&
        isString(value.modified) &&
        isBoolean(value.hasChanges),
      'GitFileDiffResult'
    );
  };
}

export function sessionStatsResult(): ResultValidator {
  return (value) => {
    assertResult(
      isPlainRecord(value) &&
        isNumber(value.totalSessions) &&
        isNumber(value.totalMessages) &&
        (value.firstSessionDate === undefined || isNumber(value.firstSessionDate)) &&
        (value.lastSessionDate === undefined || isNumber(value.lastSessionDate)),
      'SessionStatsSummary'
    );
  };
}

export function sessionsResult(): ResultValidator {
  return (value) => {
    assertResult(Array.isArray(value) && value.every(isSession), 'Session[]');
  };
}

export function sessionDetailResult(): ResultValidator {
  return (value) => {
    assertResult(
      value === null ||
        (isSession(value) &&
          isPlainRecord(value) &&
          Array.isArray(value.messages) &&
          value.messages.every(isSessionMessage)),
      'SessionDetail | null'
    );
  };
}

export function appSupportResult(): ResultValidator {
  return (value) => {
    const validSupportStatuses = new Set(['full', 'partial', 'coming_soon', 'not_supported']);
    assertResult(
      isPlainRecord(value) &&
        isBoolean(value.supported) &&
        isString(value.status) &&
        validSupportStatuses.has(value.status) &&
        isBoolean(value.isAvailable) &&
        (value.notAvailableReason === undefined || isString(value.notAvailableReason)),
      'AppSupportSummary'
    );
  };
}

export function terminalInfoResult(): ResultValidator {
  return (value) => {
    const validPreferred = new Set(['auto', 'ghostty', 'kitty', 'terminal']);
    assertResult(
      isPlainRecord(value) &&
        isString(value.preferred) &&
        validPreferred.has(value.preferred) &&
        isBoolean(value.ghosttyInstalled) &&
        isBoolean(value.kittyInstalled),
      'TerminalInfo'
    );
  };
}

/**
 * ConversationView Utilities
 *
 * 工具函数集中管理
 */

import * as Diff from 'diff';
import type { SessionMessage } from '@/types/session';
import type { MessageTurn, MessageTurnWithCount } from './types';
import { type ToolType } from './types';

/**
 * Parse Claude Code XML output format
 */
export function parseClaudeCodeXML(content: string): Array<{
  type: 'file' | 'directory' | 'text';
  path?: string;
  content?: string;
  entries?: string[];
}> | null {
  if (!content.includes('<path>') || !content.includes('<type>')) {
    return null;
  }

  const results: Array<{
    type: 'file' | 'directory' | 'text';
    path?: string;
    content?: string;
    entries?: string[];
  }> = [];

  const extractTag = (str: string, tagName: string): string | null => {
    const openTag = `<${tagName}>`;
    const closeTag = `</${tagName}>`;
    const startIdx = str.indexOf(openTag);
    if (startIdx === -1) return null;
    const contentStart = startIdx + openTag.length;
    const endIdx = str.indexOf(closeTag, contentStart);
    if (endIdx === -1) return null;
    return str.substring(contentStart, endIdx);
  };

  const paths: Array<{ path: string; start: number }> = [];
  const pathRegex = /<path>([\s\S]*?)<\/path>/g;
  let match;

  while ((match = pathRegex.exec(content)) !== null) {
    const path = match[1];
    if (path.startsWith('/')) {
      paths.push({ path, start: match.index });
    }
  }

  const extractContentAfter = (str: string, startPos: number): string | null => {
    const openTag = '<content>';
    const closeTag = '</content>';
    const openIdx = str.indexOf(openTag, startPos);
    if (openIdx === -1) return null;
    const contentStart = openIdx + openTag.length;
    const closeIdx = str.indexOf(closeTag, contentStart);
    if (closeIdx === -1) return null;
    return str.substring(contentStart, closeIdx);
  };

  const extractEntriesAfter = (str: string, startPos: number): string | null => {
    const openTag = '<entries>';
    const closeTag = '</entries>';
    const openIdx = str.indexOf(openTag, startPos);
    if (openIdx === -1) return null;
    const contentStart = openIdx + openTag.length;
    const closeIdx = str.indexOf(closeTag, contentStart);
    if (closeIdx === -1) return null;
    return str.substring(contentStart, closeIdx);
  };

  for (let i = 0; i < paths.length; i++) {
    const { path, start } = paths[i];
    const end = i < paths.length - 1 ? paths[i + 1].start : content.length;
    const segment = content.substring(start, end);

    const type = extractTag(segment, 'type');

    if (type === 'file') {
      const typePos = segment.indexOf('<type>file</type>');
      const fileContent = extractContentAfter(segment, typePos);
      if (fileContent !== null) {
        results.push({ type: 'file', path, content: fileContent });
      }
    } else if (type === 'directory') {
      const typePos = segment.indexOf('<type>directory</type>');
      const entriesContent = extractEntriesAfter(segment, typePos);
      if (entriesContent !== null) {
        const entries = entriesContent
          .trim()
          .split('\n')
          .filter((e) => e.trim());
        results.push({ type: 'directory', path, entries });
      }
    } else if (type) {
      results.push({ type: 'text', path });
    }
  }

  return results.length > 0 ? results : null;
}

/**
 * Group messages into turns
 */
export function groupMessagesIntoTurns(
  messages: SessionMessage[],
  appType?: string
): MessageTurn[] {
  const turns: MessageTurn[] = [];
  let currentTurn: MessageTurn | null = null;
  let pendingToolCalls: SessionMessage[] = [];
  type ToolCallPair = MessageTurn['toolCalls'][number];
  const pendingPairs: ToolCallPair[] = [];
  const pendingByCallId = new Map<string, ToolCallPair>();
  const pendingByToolName = new Map<string, ToolCallPair[]>();

  const rememberPendingPair = (pair: ToolCallPair): void => {
    pendingPairs.push(pair);
    if (pair.toolUse?.callId) {
      pendingByCallId.set(pair.toolUse.callId, pair);
    }
    if (pair.toolUse?.tool_name) {
      const pairs = pendingByToolName.get(pair.toolUse.tool_name) || [];
      pairs.push(pair);
      pendingByToolName.set(pair.toolUse.tool_name, pairs);
    }
  };

  const forgetPendingPair = (pair: ToolCallPair): void => {
    const pendingIndex = pendingPairs.indexOf(pair);
    if (pendingIndex >= 0) {
      pendingPairs.splice(pendingIndex, 1);
    }

    if (pair.toolUse?.callId && pendingByCallId.get(pair.toolUse.callId) === pair) {
      pendingByCallId.delete(pair.toolUse.callId);
    }

    if (pair.toolUse?.tool_name) {
      const pairs = pendingByToolName.get(pair.toolUse.tool_name);
      if (pairs) {
        const nameIndex = pairs.indexOf(pair);
        if (nameIndex >= 0) {
          pairs.splice(nameIndex, 1);
        }
        if (pairs.length === 0) {
          pendingByToolName.delete(pair.toolUse.tool_name);
        }
      }
    }
  };

  for (let i = 0; i < messages.length; i++) {
    const message = messages[i];

    switch (message.type) {
      case 'user':
        if (currentTurn) {
          turns.push(currentTurn);
        }
        currentTurn = {
          userMessage: message,
          userMessageOriginalIndex: i,
          toolCalls: [],
          assistantMessage: null,
          systemMessages: [],
        };
        pendingToolCalls = [];
        break;

      case 'system':
        if (currentTurn) {
          currentTurn.systemMessages.push(message);
        } else {
          currentTurn = {
            userMessage: null,
            toolCalls: [],
            assistantMessage: null,
            systemMessages: [message],
          };
        }
        break;

      case 'tool_use':
        if (currentTurn) {
          pendingToolCalls.push(message);
          const toolCall = {
            toolUse: message,
            toolResult: null,
          };
          currentTurn.toolCalls.push(toolCall);
          rememberPendingPair(toolCall);
        } else {
          const toolCall = { toolUse: message, toolResult: null };
          turns.push({
            userMessage: null,
            toolCalls: [toolCall],
            assistantMessage: null,
            systemMessages: [],
          });
          rememberPendingPair(toolCall);
        }
        break;

      case 'tool_result': {
        let matched = false;

        // First try to match by callId if available (most accurate)
        if (currentTurn && message.callId) {
          const pendingIndex = currentTurn.toolCalls.findIndex(
            (tc) => tc.toolUse?.callId === message.callId && !tc.toolResult
          );
          if (pendingIndex >= 0) {
            currentTurn.toolCalls[pendingIndex].toolResult = message;
            forgetPendingPair(currentTurn.toolCalls[pendingIndex]);
            // Also remove from pendingToolCalls if present
            const pendingToolIndex = pendingToolCalls.findIndex(
              (tc) => tc.callId === message.callId
            );
            if (pendingToolIndex >= 0) {
              pendingToolCalls.splice(pendingToolIndex, 1);
            }
            matched = true;
          }
        }

        // Fall back to tool_name matching if callId matching failed
        if (!matched && currentTurn && pendingToolCalls.length > 0) {
          const pendingIndex = pendingToolCalls.findIndex(
            (tc) => tc.tool_name === message.tool_name
          );
          if (pendingIndex >= 0) {
            const pendingMessage = pendingToolCalls[pendingIndex];
            const pendingPair = currentTurn.toolCalls.find(
              (pair) => pair.toolUse === pendingMessage && !pair.toolResult
            );
            if (pendingPair) {
              pendingPair.toolResult = message;
              forgetPendingPair(pendingPair);
              matched = true;
            }
            pendingToolCalls.splice(pendingIndex, 1);
          } else {
            const firstPending = currentTurn.toolCalls.find((tc) => !tc.toolResult);
            if (firstPending) {
              firstPending.toolResult = message;
              forgetPendingPair(firstPending);
              matched = true;
            }
          }
        }

        // Try indexed pending calls if not matched in current turn.
        if (!matched) {
          const pendingById = message.callId ? pendingByCallId.get(message.callId) : undefined;
          if (pendingById && !pendingById.toolResult) {
            pendingById.toolResult = message;
            forgetPendingPair(pendingById);
            matched = true;
          }
        }

        if (!matched && message.tool_name) {
          const pendingByName = pendingByToolName
            .get(message.tool_name)
            ?.find((pair) => pair.toolUse && !pair.toolResult);
          if (pendingByName) {
            pendingByName.toolResult = message;
            forgetPendingPair(pendingByName);
            matched = true;
          }
        }

        if (!matched) {
          const firstPending = pendingPairs.find((pair) => pair.toolUse && !pair.toolResult);
          if (firstPending) {
            firstPending.toolResult = message;
            forgetPendingPair(firstPending);
            matched = true;
          }
        }

        if (!matched) {
          if (currentTurn) {
            currentTurn.toolCalls.push({ toolUse: null, toolResult: message });
          } else {
            turns.push({
              userMessage: null,
              toolCalls: [{ toolUse: null, toolResult: message }],
              assistantMessage: null,
              systemMessages: [],
            });
          }
        }

        break;
      }

      case 'assistant':
        if (currentTurn) {
          const nextMessage = messages[i + 1];
          const isMultiMessageTurn =
            appType === 'claude' &&
            nextMessage &&
            (nextMessage.type === 'assistant' || nextMessage.type === 'tool_use');

          if (currentTurn.assistantMessage) {
            if (message.reasoning_content) {
              currentTurn.assistantMessage.reasoning_content = [
                currentTurn.assistantMessage.reasoning_content,
                message.reasoning_content,
              ]
                .filter(Boolean)
                .join('\n\n');
            }
            if (message.content) {
              currentTurn.assistantMessage.content = [
                currentTurn.assistantMessage.content,
                message.content,
              ]
                .filter(Boolean)
                .join('\n\n');
            }
            if (message.redacted_content) {
              currentTurn.assistantMessage.redacted_content = [
                currentTurn.assistantMessage.redacted_content,
                message.redacted_content,
              ]
                .filter(Boolean)
                .join('\n\n');
            }
            currentTurn.assistantMessageCount = (currentTurn.assistantMessageCount || 1) + 1;
          } else {
            currentTurn.assistantMessage = { ...message };
            currentTurn.assistantMessageCount = 1;
          }

          if (!isMultiMessageTurn) {
            turns.push(currentTurn);
            currentTurn = null;
          }
        } else {
          turns.push({
            userMessage: null,
            toolCalls: [],
            assistantMessage: message,
            assistantMessageCount: 1,
            systemMessages: [],
          });
        }
        break;
    }
  }

  if (currentTurn) {
    turns.push(currentTurn);
  }

  return turns;
}

/**
 * Group messages into turns with count
 */
export function groupMessagesIntoTurnsWithCount(
  messages: SessionMessage[],
  appType?: string
): MessageTurnWithCount[] {
  const turns = groupMessagesIntoTurns(messages, appType);
  return turns.map((turn) => {
    let messageCount = 0;
    if (turn.userMessage) messageCount++;
    for (const tc of turn.toolCalls) {
      if (tc.toolUse) messageCount++;
      if (tc.toolResult) messageCount++;
    }
    if (turn.assistantMessage) messageCount += turn.assistantMessageCount || 1;
    messageCount += turn.systemMessages.length;
    return { ...turn, messageCount };
  });
}

/**
 * Verify message count consistency
 */
export function verifyMessageCount(
  turns: MessageTurnWithCount[],
  originalMessages: SessionMessage[],
  appType?: string
): void {
  const totalCount = turns.reduce((sum, t) => sum + t.messageCount, 0);
  if (totalCount !== originalMessages.length) {
    console.warn(
      `[ConversationView] Message count verification failed: turns total=${totalCount}, original=${originalMessages.length}, appType=${appType}`
    );
    const typeCounts: Record<string, number> = {};
    for (const msg of originalMessages) {
      typeCounts[msg.type] = (typeCounts[msg.type] || 0) + 1;
    }
    console.warn('[ConversationView] Original messages breakdown:', typeCounts);
  }
}

/**
 * Get tool type from name
 */
export function getToolType(toolName: string): ToolType {
  const name = toolName.toLowerCase();

  if (name.includes('skill') || name.includes('mcp')) {
    return 'mcp';
  }

  if (name.includes('agent') || name.includes('spawn') || name.includes('delegate')) {
    return 'subagent';
  }

  if (name.includes('planmode') || name === 'enterplanmode' || name === 'exitplanmode') {
    return 'plan';
  }

  if (
    ['read', 'write', 'glob', 'grep', 'edit', 'ls', 'mkdir', 'apply_patch'].includes(name) ||
    name.includes('file')
  ) {
    return 'filesystem';
  }

  if (['search', 'fetch', 'curl', 'web'].some((item) => name.includes(item))) {
    return 'search';
  }

  if (
    ['bash', 'python', 'node', 'npm', 'exec_command', 'write_stdin', 'shell'].includes(name) ||
    name.includes('command')
  ) {
    return 'code';
  }

  return 'generic';
}

/**
 * Get tool display name
 */
export function getToolDisplayName(toolName: string): string {
  if (toolName.startsWith('mcp:')) {
    return `MCP ${toolName.slice(4)}`;
  }

  const displayNames: Record<string, string> = {
    read: 'Read File',
    write: 'Write File',
    edit: 'Edit File',
    glob: 'Find Files',
    grep: 'Search Content',
    ls: 'List Directory',
    mkdir: 'Create Directory',
    bash: 'Execute Command',
    exec_command: 'Execute Command',
    write_stdin: 'Write Stdin',
    apply_patch: 'Apply Patch',
    skill: 'MCP Skill',
    EnterPlanMode: 'Enter Plan Mode',
    ExitPlanMode: 'Exit Plan Mode',
  };

  return displayNames[toolName] || toolName.charAt(0).toUpperCase() + toolName.slice(1);
}

/**
 * Get tool summary for header display
 */
export function getToolSummary(toolName: string, input?: Record<string, unknown>): string | null {
  if (!input) return null;

  const name = toolName.toLowerCase();

  if (['read', 'write', 'edit'].includes(name)) {
    const filePath = input.file_path || input.path;
    if (typeof filePath === 'string') {
      const parts = filePath.split('/');
      return parts[parts.length - 1] || filePath;
    }
  }

  if (name === 'glob') {
    const pattern = input.pattern || input.glob;
    if (typeof pattern === 'string') {
      return pattern;
    }
  }

  if (name === 'grep') {
    const pattern = input.pattern || input.regex;
    const path = input.path || input.file_path;
    if (typeof pattern === 'string') {
      const pathSuffix = typeof path === 'string' ? ` in ${path.split('/').pop()}` : '';
      return `"${pattern}"${pathSuffix}`;
    }
  }

  if (name === 'bash' || name === 'exec_command') {
    const command = input.command || input.cmd;
    if (typeof command === 'string') {
      if (command.length > 50) {
        return command.substring(0, 50) + '...';
      }
      return command;
    }
  }

  if (name === 'ls') {
    const dir = input.dir || input.directory || input.path;
    if (typeof dir === 'string') {
      const parts = dir.split('/');
      return parts[parts.length - 1] || dir;
    }
  }

  return null;
}

/**
 * Get language from file path
 */
export function getLanguageFromPath(path: string): string {
  const ext = path.split('.').pop()?.toLowerCase();
  const langMap: Record<string, string> = {
    js: 'javascript',
    ts: 'typescript',
    jsx: 'jsx',
    tsx: 'tsx',
    py: 'python',
    go: 'go',
    rs: 'rust',
    java: 'java',
    kt: 'kotlin',
    swift: 'swift',
    c: 'c',
    cpp: 'cpp',
    h: 'c',
    hpp: 'cpp',
    json: 'json',
    yaml: 'yaml',
    yml: 'yaml',
    md: 'markdown',
    sh: 'bash',
    bash: 'bash',
    zsh: 'bash',
    html: 'html',
    css: 'css',
    scss: 'scss',
    less: 'less',
    sql: 'sql',
    xml: 'xml',
    dockerfile: 'dockerfile',
    env: 'bash',
  };
  return langMap[ext || ''] || 'text';
}

/**
 * Format timestamp with date and time
 */
export function formatTimestamp(timestamp: string): string {
  const date = new Date(timestamp);
  const dateStr = date.toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  });
  const timeStr = date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  return `${dateStr} ${timeStr}`;
}

/**
 * Format value for display
 */
export function formatValue(value: unknown, truncate: boolean = false): string {
  if (value === null) return 'null';
  if (value === undefined) return 'undefined';
  if (typeof value === 'string') {
    if (truncate && value.length > 100) {
      return value.substring(0, 100) + '...';
    }
    return value;
  }
  if (typeof value === 'object') {
    return JSON.stringify(value);
  }
  return String(value);
}

// Diff computation types
export type DiffLine = {
  type: 'unchanged' | 'removed' | 'added';
  oldLine?: string;
  newLine?: string;
  oldIndex?: number;
  newIndex?: number;
};

/**
 * Compute diff for edit display
 */
export function computeDiff(oldLines: string[], newLines: string[]): DiffLine[] {
  const oldContent = oldLines.join('\n');
  const newContent = newLines.join('\n');

  const changes = Diff.diffLines(oldContent, newContent, {
    ignoreWhitespace: true,
    newlineIsToken: false,
  });

  const result: DiffLine[] = [];
  let oldIdx = 0;
  let newIdx = 0;

  for (const change of changes) {
    const lines = change.value.replace(/\n$/, '').split('\n');
    if (change.value.endsWith('\n') && lines[lines.length - 1] === '') {
      lines.pop();
    }

    if (change.added) {
      for (const line of lines) {
        result.push({ type: 'added', newLine: line, newIndex: newIdx });
        newIdx++;
      }
    } else if (change.removed) {
      for (const line of lines) {
        result.push({ type: 'removed', oldLine: line, oldIndex: oldIdx });
        oldIdx++;
      }
    } else {
      for (const line of lines) {
        result.push({
          type: 'unchanged',
          oldLine: line,
          newLine: line,
          oldIndex: oldIdx,
          newIndex: newIdx,
        });
        oldIdx++;
        newIdx++;
      }
    }
  }

  return result;
}

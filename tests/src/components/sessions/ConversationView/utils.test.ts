import { describe, expect, it, vi } from 'vitest';
import type { SessionMessage } from '@/types/session';
import {
  computeDiff,
  formatTimestamp,
  formatValue,
  getLanguageFromPath,
  getToolDisplayName,
  getToolSummary,
  getToolType,
  groupMessagesIntoTurns,
  groupMessagesIntoTurnsWithCount,
  parseClaudeCodeXML,
  verifyMessageCount,
} from '@/components/sessions/ConversationView/utils';

const timestamp = '2024-01-02T03:04:00+08:00';

function message(overrides: Partial<SessionMessage>): SessionMessage {
  return {
    type: 'user',
    timestamp,
    ...overrides,
  };
}

describe('ConversationView utils', () => {
  it('parses Claude Code XML file and directory blocks', () => {
    const content = [
      '<path>/tmp/example.ts</path>',
      '<type>file</type>',
      '<content>const value = 1;</content>',
      '<path>/tmp/src</path>',
      '<type>directory</type>',
      '<entries>',
      'index.ts',
      'utils.ts',
      '</entries>',
    ].join('\n');

    expect(parseClaudeCodeXML(content)).toEqual([
      {
        type: 'file',
        path: '/tmp/example.ts',
        content: 'const value = 1;',
      },
      {
        type: 'directory',
        path: '/tmp/src',
        entries: ['index.ts', 'utils.ts'],
      },
    ]);
  });

  it('groups user, tool, assistant, and system messages into a turn', () => {
    const messages: SessionMessage[] = [
      message({ type: 'system', content: 'system note' }),
      message({ type: 'user', content: 'read package' }),
      message({ type: 'tool_use', tool_name: 'read', callId: 'call-1' }),
      message({ type: 'tool_result', tool_name: 'read', callId: 'call-1' }),
      message({ type: 'assistant', content: 'done' }),
    ];

    const turns = groupMessagesIntoTurns(messages);

    expect(turns).toHaveLength(2);
    expect(turns[0].systemMessages).toHaveLength(1);
    expect(turns[1].userMessage?.content).toBe('read package');
    expect(turns[1].toolCalls).toHaveLength(1);
    expect(turns[1].toolCalls[0].toolUse?.callId).toBe('call-1');
    expect(turns[1].toolCalls[0].toolResult?.callId).toBe('call-1');
    expect(turns[1].assistantMessage?.content).toBe('done');
  });

  it('matches delayed tool results against earlier turns by callId', () => {
    const messages: SessionMessage[] = [
      message({ type: 'user', content: 'run command' }),
      message({ type: 'tool_use', tool_name: 'bash', callId: 'call-2' }),
      message({ type: 'assistant', content: 'waiting' }),
      message({ type: 'tool_result', tool_name: 'bash', callId: 'call-2' }),
    ];

    const turns = groupMessagesIntoTurns(messages);

    expect(turns).toHaveLength(1);
    expect(turns[0].toolCalls[0].toolResult?.callId).toBe('call-2');
  });

  it('matches multiple tool results without call IDs to distinct pending calls', () => {
    const messages: SessionMessage[] = [
      message({ type: 'user', content: 'run tools' }),
      message({ type: 'tool_use', tool_name: 'read', tool_input: { path: 'a' } }),
      message({ type: 'tool_use', tool_name: 'read', tool_input: { path: 'b' } }),
      message({ type: 'tool_result', tool_name: 'read', tool_output: { output: 'a' } }),
      message({ type: 'tool_result', tool_name: 'read', tool_output: { output: 'b' } }),
    ];

    const turns = groupMessagesIntoTurns(messages);

    expect(turns[0].toolCalls[0].toolResult?.tool_output?.output).toBe('a');
    expect(turns[0].toolCalls[1].toolResult?.tool_output?.output).toBe('b');
  });

  it('preserves consecutive Claude assistant content when grouping', () => {
    const messages: SessionMessage[] = [
      message({ type: 'user', content: 'work' }),
      message({ type: 'assistant', content: 'Before tool' }),
      message({ type: 'tool_use', tool_name: 'read', tool_input: {} }),
      message({ type: 'assistant', content: 'After tool' }),
    ];

    const turns = groupMessagesIntoTurnsWithCount(messages, 'claude');

    expect(turns).toHaveLength(1);
    expect(turns[0].assistantMessage?.content).toBe('Before tool\n\nAfter tool');
    expect(turns[0].messageCount).toBe(4);
  });

  it('counts grouped messages consistently for normal turns', () => {
    const messages: SessionMessage[] = [
      message({ type: 'user', content: 'hello' }),
      message({ type: 'assistant', content: 'hi' }),
    ];

    expect(groupMessagesIntoTurnsWithCount(messages)).toEqual([
      expect.objectContaining({ messageCount: 2 }),
    ]);
  });

  it('warns when grouped message counts do not match source messages', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

    verifyMessageCount(
      [
        {
          userMessage: null,
          toolCalls: [],
          assistantMessage: null,
          systemMessages: [],
          messageCount: 0,
        },
      ],
      [message({ type: 'user', content: 'hello' })],
      'codebuddy'
    );

    expect(warn).toHaveBeenCalledWith(expect.stringContaining('Message count verification failed'));
    warn.mockRestore();
  });

  it('classifies and summarizes common tools', () => {
    expect(getToolType('Read')).toBe('filesystem');
    expect(getToolType('Bash')).toBe('code');
    expect(getToolType('mcp:node_repl.js')).toBe('mcp');
    expect(getToolDisplayName('mcp:node_repl.js')).toBe('MCP node_repl.js');
    expect(getToolType('SpawnAgent')).toBe('subagent');
    expect(getToolType('EnterPlanMode')).toBe('plan');
    expect(getToolSummary('read', { file_path: '/tmp/package.json' })).toBe('package.json');
    expect(getToolSummary('grep', { pattern: 'TODO', path: '/tmp/src' })).toBe('"TODO" in src');
    expect(getToolSummary('bash', { command: 'npm run build' })).toBe('npm run build');
  });

  it('maps common file extensions to syntax languages', () => {
    expect(getLanguageFromPath('src/App.tsx')).toBe('tsx');
    expect(getLanguageFromPath('scripts/install.sh')).toBe('bash');
    expect(getLanguageFromPath('.env')).toBe('bash');
    expect(getLanguageFromPath('README.unknown')).toBe('text');
  });

  it('formats timestamps and values for display', () => {
    expect(formatTimestamp(timestamp)).toMatch(/^2024\/01\/02 03:04$/);
    expect(formatValue(null)).toBe('null');
    expect(formatValue(undefined)).toBe('undefined');
    expect(formatValue({ ok: true })).toBe('{"ok":true}');
    expect(formatValue('a'.repeat(101), true)).toBe(`${'a'.repeat(100)}...`);
  });

  it('computes added, removed, and unchanged diff lines', () => {
    expect(computeDiff(['one', 'two'], ['one', 'three'])).toEqual([
      {
        type: 'unchanged',
        oldLine: 'one',
        newLine: 'one',
        oldIndex: 0,
        newIndex: 0,
      },
      {
        type: 'removed',
        oldLine: 'two',
        oldIndex: 1,
      },
      {
        type: 'added',
        newLine: 'three',
        newIndex: 1,
      },
    ]);
  });
});

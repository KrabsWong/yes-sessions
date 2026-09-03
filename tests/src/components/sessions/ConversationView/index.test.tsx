import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionDetail, SessionMessage } from '@/types/session';
import { ConversationView } from '@/components/sessions/ConversationView';

const { getDetailMock } = vi.hoisted(() => ({ getDetailMock: vi.fn() }));

vi.mock('@/lib/api/sessions', () => ({
  sessionsApi: { getDetail: getDetailMock },
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, defaultValue?: string) => {
      const translations: Record<string, string> = {
        'sessions.you': 'You',
        'sessions.caveat': 'Caveat',
        'sessions.pasted': 'Pasted',
        'sessions.command': 'Command',
        'sessions.commandOutput': 'Command Output',
        'sessions.system': 'System',
        'sessions.model': 'Model',
        'sessions.input': 'Input',
        'sessions.output': 'Output',
        'sessions.thinking': 'Thinking',
        'sessions.expand': 'Expand',
        'sessions.collapse': 'Collapse',
        'sessions.viewSubAgentSession': 'View Sub-Agent Session',
        'sessions.expandSubAgentSession': 'Expand sub-agent conversation',
        'sessions.collapseSubAgentSession': 'Collapse sub-agent conversation',
        'sessions.loadingConversation': 'Loading conversation...',
      };
      return translations[key] || defaultValue || key;
    },
  }),
}));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const timestamp = '2024-01-02T03:04:00+08:00';

function message(overrides: Partial<SessionMessage>): SessionMessage {
  return {
    type: 'user',
    timestamp,
    ...overrides,
  };
}

function render(element: React.ReactElement): { container: HTMLDivElement; root: Root } {
  const scrollContainer = document.createElement('div');
  scrollContainer.id = 'conversation-scroll-container';
  document.body.appendChild(scrollContainer);

  const container = document.createElement('div');
  scrollContainer.appendChild(container);
  const root = createRoot(container);

  act(() => {
    root.render(element);
  });

  return { container, root };
}

function click(element: Element): void {
  act(() => {
    element.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  });
}

function unmount(root: Root): void {
  act(() => {
    root.unmount();
  });
}

describe('ConversationView', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    getDetailMock.mockReset();
  });

  it('renders system, user, and assistant messages', () => {
    const messages: SessionMessage[] = [
      message({ type: 'system', content: 'System reminder' }),
      message({ type: 'user', content: 'Summarize this repo' }),
      message({ type: 'assistant', content: 'Here is the summary.' }),
    ];

    const { container, root } = render(
      <ConversationView messages={messages} appType="codebuddy" />
    );

    expect(container.textContent).toContain('System reminder');
    expect(container.textContent).toContain('You');
    expect(container.textContent).toContain('Summarize this repo');
    expect(container.textContent).toContain('Here is the summary.');
    unmount(root);
  });

  it('renders tool calls and expands input and output details', () => {
    const messages: SessionMessage[] = [
      message({ type: 'user', content: 'Read package metadata' }),
      message({
        type: 'tool_use',
        tool_name: 'read',
        callId: 'read-1',
        tool_input: { file_path: '/repo/package.json' },
      }),
      message({
        type: 'tool_result',
        tool_name: 'read',
        callId: 'read-1',
        tool_output: { output: '{ "name": "yes-sessions" }' },
      }),
      message({ type: 'assistant', content: 'The package is yes-sessions.' }),
    ];

    const { container, root } = render(
      <ConversationView messages={messages} appType="codebuddy" />
    );

    expect(container.textContent).toContain('Read File');
    expect(container.textContent).toContain('package.json');
    expect(container.textContent).not.toContain('file_path:');
    expect(container.textContent).not.toContain('"name": "yes-sessions"');

    const toolButton = Array.from(container.querySelectorAll('button')).find((button) =>
      button.textContent?.includes('Read File')
    );
    expect(toolButton).toBeTruthy();
    click(toolButton!);

    expect(container.textContent).toContain('file_path:');
    expect(container.textContent).toContain('/repo/package.json');
    expect(container.textContent).toContain('"name": "yes-sessions"');
    expect(container.textContent).toContain('The package is yes-sessions.');
    unmount(root);
  });

  it('keeps embedded local image data URLs in assistant markdown', () => {
    const imageDataUrl =
      'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciPjwvc3ZnPg==';
    const messages: SessionMessage[] = [
      message({ type: 'user', content: 'Show image' }),
      message({ type: 'assistant', content: `![Preview](${imageDataUrl})` }),
    ];

    const { container, root } = render(<ConversationView messages={messages} appType="codex" />);

    const img = container.querySelector('img[alt="Preview"]') as HTMLImageElement | null;
    expect(img).toBeTruthy();
    expect(img?.getAttribute('src')).toBe(imageDataUrl);
    unmount(root);
  });

  it('renders redacted-only messages instead of dropping them', () => {
    const messages: SessionMessage[] = [
      message({ type: 'user', content: undefined, redacted_content: '[user redacted]' }),
      message({ type: 'assistant', content: undefined, redacted_content: '[assistant redacted]' }),
    ];

    const { container, root } = render(
      <ConversationView messages={messages} appType="codebuddy" />
    );

    expect(container.textContent).toContain('[user redacted]');
    expect(container.textContent).toContain('[assistant redacted]');
    unmount(root);
  });

  it('opens a linked CodeBuddy sub-agent session', () => {
    const onViewSubAgentSession = vi.fn();
    const messages: SessionMessage[] = [
      message({ type: 'user', content: 'delegate' }),
      message({
        type: 'tool_use',
        tool_name: 'Agent',
        callId: 'agent-call',
        tool_input: { description: 'Inspect repository' },
      }),
      message({
        type: 'tool_result',
        tool_name: 'Agent',
        callId: 'agent-call',
        tool_output: { output: 'done' },
        metadata: {
          childSessionId: 'agent-child',
          childSessionAppType: 'codebuddy',
          subtype: 'completed',
        },
      }),
    ];

    const { container, root } = render(
      <ConversationView
        messages={messages}
        appType="codebuddy"
        onViewSubAgentSession={onViewSubAgentSession}
      />
    );
    const button = Array.from(container.querySelectorAll('button')).find((item) =>
      item.textContent?.includes('View Sub-Agent Session')
    );

    expect(button).toBeTruthy();
    click(button!);
    expect(onViewSubAgentSession).toHaveBeenCalledWith('agent-child', 'codebuddy');
    unmount(root);
  });

  it('expands a linked sub-agent conversation inline', async () => {
    getDetailMock.mockResolvedValue({
      id: 'agent-child',
      appType: 'codebuddy',
      fileName: 'agent-child.jsonl',
      filePath: '/repo/parent/subagents/agent-child.jsonl',
      createdAt: 1,
      updatedAt: 2,
      messageCount: 2,
      kind: 'subagent',
      parentSessionId: 'parent',
      agentType: 'Explore',
      messages: [
        message({ type: 'user', content: 'Investigate rendering' }),
        message({ type: 'assistant', content: 'Nested investigation result' }),
      ],
    });
    const messages: SessionMessage[] = [
      message({ type: 'user', content: 'delegate' }),
      message({
        type: 'tool_use',
        tool_name: 'Agent',
        callId: 'agent-call',
        tool_input: { description: 'Inspect repository', subagent_type: 'Explore' },
      }),
      message({
        type: 'tool_result',
        tool_name: 'Agent',
        callId: 'agent-call',
        tool_output: { output: 'done' },
        metadata: { childSessionId: 'agent-child', childSessionAppType: 'codebuddy' },
      }),
    ];
    const { container, root } = render(
      <ConversationView messages={messages} appType="codebuddy" />
    );
    const expandButton = Array.from(container.querySelectorAll('button')).find((item) =>
      item.textContent?.includes('Expand sub-agent conversation')
    );

    expect(expandButton).toBeTruthy();
    await act(async () => {
      expandButton!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      await Promise.resolve();
    });
    await vi.waitFor(() => {
      expect(container.textContent).toContain('Nested investigation result');
    });
    expect(getDetailMock).toHaveBeenCalledWith('agent-child', 'codebuddy');
    unmount(root);
  });

  it('ignores a stale inline sub-agent response after the card is reused', async () => {
    let resolveFirst: (detail: SessionDetail | null) => void = () => undefined;
    let resolveSecond: (detail: SessionDetail | null) => void = () => undefined;
    const firstRequest = new Promise<SessionDetail | null>((resolve) => {
      resolveFirst = resolve;
    });
    const secondRequest = new Promise<SessionDetail | null>((resolve) => {
      resolveSecond = resolve;
    });
    getDetailMock.mockImplementation((sessionId: string) =>
      sessionId === 'agent-first' ? firstRequest : secondRequest
    );
    const agentMessages = (childSessionId: string): SessionMessage[] => [
      message({ type: 'user', content: 'delegate' }),
      message({
        type: 'tool_use',
        tool_name: 'Agent',
        callId: 'agent-call',
        tool_input: { description: childSessionId },
      }),
      message({
        type: 'tool_result',
        tool_name: 'Agent',
        callId: 'agent-call',
        tool_output: { output: 'done' },
        metadata: { childSessionId, childSessionAppType: 'codebuddy' },
      }),
    ];
    const { container, root } = render(
      <ConversationView messages={agentMessages('agent-first')} appType="codebuddy" />
    );
    const findExpandButton = () =>
      Array.from(container.querySelectorAll('button')).find((item) =>
        item.textContent?.includes('Expand sub-agent conversation')
      );

    await act(async () => {
      findExpandButton()!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      await Promise.resolve();
    });
    act(() => {
      root.render(
        <ConversationView messages={agentMessages('agent-second')} appType="codebuddy" />
      );
    });
    await act(async () => {
      findExpandButton()!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      await Promise.resolve();
    });

    await act(async () => {
      resolveSecond({
        id: 'agent-second',
        appType: 'codebuddy',
        fileName: 'agent-second.jsonl',
        filePath: '/repo/agent-second.jsonl',
        createdAt: 1,
        updatedAt: 2,
        messageCount: 1,
        messages: [message({ type: 'assistant', content: 'second response' })],
      });
      await secondRequest;
    });
    await vi.waitFor(() => expect(container.textContent).toContain('second response'));

    await act(async () => {
      resolveFirst({
        id: 'agent-first',
        appType: 'codebuddy',
        fileName: 'agent-first.jsonl',
        filePath: '/repo/agent-first.jsonl',
        createdAt: 1,
        updatedAt: 2,
        messageCount: 1,
        messages: [message({ type: 'assistant', content: 'stale first response' })],
      });
      await firstRequest;
    });
    expect(container.textContent).toContain('second response');
    expect(container.textContent).not.toContain('stale first response');
    unmount(root);
  });
});

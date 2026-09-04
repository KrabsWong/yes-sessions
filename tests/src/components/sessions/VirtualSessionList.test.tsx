import React, { act, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session } from '@/types';
import '@/lib/i18n';
import {
  VirtualSessionList,
  formatSessionDateGroupKey,
  getSessionDirectoryGroupKey,
  type ViewMode,
} from '@/components/sessions/VirtualSessionList';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: index * 40,
      })),
    getTotalSize: () => count * 40,
    measure: vi.fn(),
  }),
}));

vi.mock('@/components/MarqueeText', () => ({
  MarqueeText: ({ text, className }: { text: string; className?: string }) => (
    <span className={className}>{text}</span>
  ),
}));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const baseSessions: Session[] = [
  {
    id: 'newer',
    appType: 'codebuddy',
    fileName: 'newer.jsonl',
    filePath: '/repo/app/newer.jsonl',
    createdAt: Date.UTC(2024, 0, 2, 8, 0),
    updatedAt: Date.UTC(2024, 0, 2, 10, 0),
    messageCount: 4,
    firstMessage: 'Newest request',
    directory: '/repo/app',
  },
  {
    id: 'older',
    appType: 'codebuddy',
    fileName: 'older.jsonl',
    filePath: '/repo/lib/older.jsonl',
    createdAt: Date.UTC(2024, 0, 1, 8, 0),
    updatedAt: Date.UTC(2024, 0, 1, 9, 0),
    messageCount: 2,
    firstMessage: 'Older request',
    directory: '/repo/lib',
  },
];

interface HarnessProps {
  viewMode?: ViewMode;
  onSelect?: (session: Session) => void;
  sessions?: Session[];
}

function Harness({
  viewMode = 'date',
  onSelect = () => undefined,
  sessions = baseSessions,
}: HarnessProps) {
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set());

  const groupKeys = new Set(
    sessions
      .filter((session) => session.kind !== 'subagent')
      .map((session) =>
        viewMode === 'date'
          ? formatSessionDateGroupKey(session.updatedAt)
          : getSessionDirectoryGroupKey(session, '— No Directory —')
      )
  );

  return (
    <VirtualSessionList
      sessions={sessions}
      selectedSession={sessions[0]}
      onSelect={onSelect}
      collapsedGroups={collapsedGroups}
      toggleGroup={(groupKey) =>
        setCollapsedGroups((previous) => {
          const next = new Set(previous);
          if (next.has(groupKey)) {
            next.delete(groupKey);
          } else {
            next.add(groupKey);
          }
          return next;
        })
      }
      expandAll={() => setCollapsedGroups(new Set())}
      collapseAll={() => setCollapsedGroups(groupKeys)}
      allExpanded={collapsedGroups.size === 0}
      allCollapsed={collapsedGroups.size === groupKeys.size}
      viewMode={viewMode}
    />
  );
}

function render(element: React.ReactElement): { container: HTMLDivElement; root: Root } {
  const container = document.createElement('div');
  container.style.height = '600px';
  document.body.appendChild(container);
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

describe('VirtualSessionList', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('renders date-grouped sessions and selects a clicked session', () => {
    const onSelect = vi.fn();
    const { container, root } = render(<Harness onSelect={onSelect} />);

    expect(container.textContent).toContain('01/02/2024');
    expect(container.textContent).toContain('Newest request');
    expect(container.textContent).toContain('Older request');

    const olderButton = Array.from(container.querySelectorAll('button')).find((button) =>
      button.textContent?.includes('Older request')
    );
    expect(olderButton).toBeTruthy();
    click(olderButton!);

    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ id: 'older' }));
    unmount(root);
  });

  it('orders sessions by recency regardless of the input order', () => {
    const sameUpdatedAt = Date.UTC(2024, 0, 2, 10, 0);
    const sessions = [
      baseSessions[1],
      {
        ...baseSessions[0],
        id: 'newest-created',
        firstMessage: 'Newest created request',
        createdAt: Date.UTC(2024, 0, 2, 9, 0),
        updatedAt: sameUpdatedAt,
      },
      { ...baseSessions[0], updatedAt: sameUpdatedAt },
    ];
    const { container, root } = render(<Harness sessions={sessions} />);

    const renderedIds = Array.from(container.querySelectorAll('[data-session-id]')).map(
      (element) => element.getAttribute('data-session-id')
    );
    expect(renderedIds).toEqual(['newest-created', 'newer', 'older']);
    unmount(root);
  });

  it('collapses and expands all groups from header controls', () => {
    const { container, root } = render(<Harness />);

    const collapseAll = container.querySelector('button[title="Collapse All"]');
    expect(collapseAll).toBeTruthy();
    click(collapseAll!);
    expect(container.textContent).not.toContain('Newest request');
    expect(container.textContent).not.toContain('Older request');

    const expandAll = container.querySelector('button[title="Expand All"]');
    expect(expandAll).toBeTruthy();
    click(expandAll!);
    expect(container.textContent).toContain('Newest request');
    expect(container.textContent).toContain('Older request');
    unmount(root);
  });

  it('renders directory-grouped sessions with directory labels', () => {
    const { container, root } = render(<Harness viewMode="directory" />);

    expect(container.textContent).toContain('app');
    expect(container.textContent).toContain('lib');
    expect(container.textContent).toContain('Newest request');
    expect(container.textContent).toContain('Older request');
    unmount(root);
  });

  it('nests sub-agent sessions under their parent and exposes their type', () => {
    const onSelect = vi.fn();
    const child: Session = {
      id: 'agent-child',
      appType: 'codebuddy',
      fileName: 'agent-child.jsonl',
      filePath: '/repo/app/newer/subagents/agent-child.jsonl',
      createdAt: Date.UTC(2024, 0, 2, 8, 30),
      updatedAt: Date.UTC(2024, 0, 2, 9, 30),
      messageCount: 3,
      firstMessage: 'Inspect the renderer',
      directory: '/repo/app',
      kind: 'subagent',
      parentSessionId: 'newer',
      agentType: 'Explore',
    };
    const { container, root } = render(
      <Harness
        sessions={[
          ...baseSessions,
          child,
          {
            ...child,
            id: 'agent-grandchild',
            fileName: 'agent-grandchild.jsonl',
            firstMessage: 'Plan nested work',
            parentSessionId: 'agent-child',
            agentType: 'Plan',
          },
        ]}
        onSelect={onSelect}
      />
    );

    expect(container.textContent).not.toContain('Inspect the renderer');
    const parentCard = container.querySelector('[data-session-id="newer"]');
    const toggle = parentCard?.querySelector('[data-testid="toggle-subagents"]');
    expect(toggle).toBeTruthy();
    expect(toggle?.textContent).toContain('1');

    click(toggle!);
    expect(container.textContent).toContain('Inspect the renderer');
    expect(container.textContent).toContain('Explore');
    expect(container.textContent).not.toContain('Plan nested work');

    const childToggle = container
      .querySelector('[data-session-id="agent-child"]')
      ?.querySelector('[data-testid="toggle-subagents"]');
    expect(childToggle).toBeTruthy();
    click(childToggle!);
    expect(container.textContent).toContain('Plan nested work');
    expect(container.textContent).toContain('Plan');

    const childCard = container.querySelector('[data-session-id="agent-child"]');
    expect(childCard?.getAttribute('data-session-kind')).toBe('subagent');
    const childButton = childCard?.querySelector('button');
    expect(childButton).toBeTruthy();
    click(childButton!);
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ id: 'agent-child' }));
    unmount(root);
  });

  it('orders expanded sub-agent sessions by recency', () => {
    const child = (id: string, updatedAt: number): Session => ({
      id,
      appType: 'codebuddy',
      fileName: `${id}.jsonl`,
      filePath: `/repo/app/newer/subagents/${id}.jsonl`,
      createdAt: updatedAt - 1,
      updatedAt,
      messageCount: 1,
      firstMessage: id,
      directory: '/repo/app',
      kind: 'subagent',
      parentSessionId: 'newer',
    });
    const { container, root } = render(
      <Harness
        sessions={[
          ...baseSessions,
          child('older-child', Date.UTC(2024, 0, 2, 10, 1)),
          child('newer-child', Date.UTC(2024, 0, 2, 10, 2)),
        ]}
      />
    );

    const toggle = container
      .querySelector('[data-session-id="newer"]')
      ?.querySelector('[data-testid="toggle-subagents"]');
    expect(toggle).toBeTruthy();
    click(toggle!);

    const childIds = Array.from(container.querySelectorAll('[data-session-kind="subagent"]')).map(
      (element) => element.getAttribute('data-session-id')
    );
    expect(childIds).toEqual(['newer-child', 'older-child']);
    unmount(root);
  });
});

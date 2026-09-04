/**
 * Virtual Session List Component
 *
 * Uses virtual scrolling to efficiently render large lists of sessions
 * Only renders visible items + small buffer for smooth scrolling
 */

import {
  useRef,
  useMemo,
  useContext,
  createContext,
  useCallback,
  useEffect,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';
import { useVirtualizer } from '@tanstack/react-virtual';
import { Bot, ChevronDown, ChevronRight, ChevronsDown, ChevronsUp, Folder } from 'lucide-react';
import { MarqueeText } from '@/components/MarqueeText';
import type { Session } from '@/types';

export type ViewMode = 'date' | 'directory';

export function compareSessionsByRecency(a: Session, b: Session): number {
  return b.updatedAt - a.updatedAt || b.createdAt - a.createdAt || a.id.localeCompare(b.id);
}

// Context for collapse state (shared with parent)
const CollapseContext = createContext<{
  collapsedGroups: Set<string>;
  toggleGroup: (groupKey: string) => void;
  expandAll: () => void;
  collapseAll: () => void;
  allExpanded: boolean;
  allCollapsed: boolean;
}>({
  collapsedGroups: new Set(),
  toggleGroup: () => {},
  expandAll: () => {},
  collapseAll: () => {},
  allExpanded: true,
  allCollapsed: false,
});

interface VirtualSessionListProps {
  sessions: Session[];
  selectedSession: Session | null;
  onSelect: (session: Session) => void;
  collapsedGroups: Set<string>;
  toggleGroup: (groupKey: string) => void;
  expandAll: () => void;
  collapseAll: () => void;
  allExpanded: boolean;
  allCollapsed: boolean;
  viewMode: ViewMode;
}

export function formatSessionDateGroupKey(timestamp: number): string {
  const date = new Date(timestamp);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

export function getSessionDirectoryGroupKey(session: Session, noDirectoryLabel: string): string {
  let dir = session.directory || '';
  if (!dir && session.filePath) {
    const lastSlashIndex = session.filePath.lastIndexOf('/');
    dir = lastSlashIndex > 0 ? session.filePath.substring(0, lastSlashIndex) : '/';
  }
  return dir || noDirectoryLabel;
}

type SessionListItem =
  | { type: 'header'; groupKey: string; isFirst: boolean }
  | {
      type: 'session';
      session: Session;
      groupKey: string;
      childCount: number;
      isSubAgent: boolean;
      depth: number;
    };

function appendSessionItems(
  items: SessionListItem[],
  session: Session,
  groupKey: string,
  childrenByParent: Map<string, Session[]>,
  expandedParentIds: Set<string>,
  depth = 0,
  ancestors = new Set<string>()
): void {
  if (ancestors.has(session.id)) return;
  const nextAncestors = new Set(ancestors);
  nextAncestors.add(session.id);
  const children = childrenByParent.get(session.id) || [];
  items.push({
    type: 'session',
    session,
    groupKey,
    childCount: children.length,
    isSubAgent: depth > 0,
    depth,
  });
  if (!expandedParentIds.has(session.id)) return;
  for (const child of children) {
    appendSessionItems(
      items,
      child,
      groupKey,
      childrenByParent,
      expandedParentIds,
      depth + 1,
      nextAncestors
    );
  }
}

/**
 * Group sessions by date and prepare virtual list items
 */
function useDateGroupedSessions(
  sessions: Session[],
  collapsedGroups: Set<string>,
  childrenByParent: Map<string, Session[]>,
  expandedParentIds: Set<string>
) {
  return useMemo(() => {
    // Group sessions by date
    const groups = new Map<string, Session[]>();

    for (const session of sessions) {
      const dateKey = formatSessionDateGroupKey(session.updatedAt);

      if (!groups.has(dateKey)) {
        groups.set(dateKey, []);
      }
      groups.get(dateKey)!.push(session);
    }

    // Sort sessions within each group by recency, newest first
    for (const [, groupSessions] of groups) {
      groupSessions.sort(compareSessionsByRecency);
    }

    // Sort date keys descending
    const sortedDates = Array.from(groups.keys()).sort((a, b) => b.localeCompare(a));

    // Build virtual list items
    const items: SessionListItem[] = [];

    sortedDates.forEach((dateKey, index) => {
      // Add header
      items.push({
        type: 'header',
        groupKey: dateKey,
        isFirst: index === 0,
      });

      // Add sessions if not collapsed
      if (!collapsedGroups.has(dateKey)) {
        const dateSessions = groups.get(dateKey)!;
        dateSessions.forEach((session) =>
          appendSessionItems(items, session, dateKey, childrenByParent, expandedParentIds)
        );
      }
    });

    return { items, groups, sortedGroupKeys: sortedDates };
  }, [sessions, collapsedGroups, childrenByParent, expandedParentIds]);
}

/**
 * Group sessions by directory and prepare virtual list items
 * Sessions within each directory are sorted by updatedAt descending
 */
function useDirectoryGroupedSessions(
  sessions: Session[],
  collapsedGroups: Set<string>,
  noDirectoryLabel: string,
  childrenByParent: Map<string, Session[]>,
  expandedParentIds: Set<string>
) {
  return useMemo(() => {
    // Group sessions by directory
    const groups = new Map<string, Session[]>();

    for (const session of sessions) {
      const dirKey = getSessionDirectoryGroupKey(session, noDirectoryLabel);

      if (!groups.has(dirKey)) {
        groups.set(dirKey, []);
      }
      groups.get(dirKey)!.push(session);
    }

    // Sort sessions within each group by recency, newest first
    for (const [, groupSessions] of groups) {
      groupSessions.sort(compareSessionsByRecency);
    }

    // Sort directories by their most recent session's updatedAt descending
    const sortedDirs = Array.from(groups.keys()).sort((a, b) => {
      const sessionsA = groups.get(a)!;
      const sessionsB = groups.get(b)!;
      const latestA = sessionsA[0]?.updatedAt || 0;
      const latestB = sessionsB[0]?.updatedAt || 0;
      return latestB - latestA;
    });

    // Build virtual list items
    const items: SessionListItem[] = [];

    sortedDirs.forEach((dirKey, index) => {
      // Add header
      items.push({
        type: 'header',
        groupKey: dirKey,
        isFirst: index === 0,
      });

      // Add sessions if not collapsed
      if (!collapsedGroups.has(dirKey)) {
        const dirSessions = groups.get(dirKey)!;
        dirSessions.forEach((session) =>
          appendSessionItems(items, session, dirKey, childrenByParent, expandedParentIds)
        );
      }
    });

    return { items, groups, sortedGroupKeys: sortedDirs };
  }, [sessions, collapsedGroups, noDirectoryLabel, childrenByParent, expandedParentIds]);
}

/**
 * Format date group label (Today, Yesterday, or date)
 */
function formatDateGroupLabel(
  dateKey: string,
  todayLabel: string,
  yesterdayLabel: string,
  language: string
): string {
  const today = formatSessionDateGroupKey(Date.now());
  const yesterday = formatSessionDateGroupKey(Date.now() - 86400000);

  if (dateKey === today) {
    return todayLabel;
  }
  if (dateKey === yesterday) {
    return yesterdayLabel;
  }

  const [year, month, day] = dateKey.split('-').map(Number);
  return new Date(year, month - 1, day).toLocaleDateString(language, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  });
}

/**
 * Get the last directory name from path
 */
function getLastDirectoryName(dirKey: string): string {
  const parts = dirKey.split('/').filter((p) => p.length > 0);
  return parts.length > 0 ? parts[parts.length - 1] : dirKey;
}

/**
 * Format parent path for display on the right side
 * - More than 2 levels: "../aa/bb..."
 * - Exactly 2 levels: "/aa/bb"
 * - Only 1 level: "aa/..."
 * - Otherwise: empty
 */
function formatParentPath(dirKey: string): string {
  const parts = dirKey.split('/').filter((p) => p.length > 0);

  // Remove the last part (current directory name)
  const parentParts = parts.slice(0, -1);

  if (parentParts.length === 0) {
    // No parent or only root
    return '';
  } else if (parentParts.length === 1) {
    // Only 1 parent level
    return `${parentParts[0]}/...`;
  } else if (parentParts.length === 2) {
    // Exactly 2 parent levels, show from root
    return '/' + parentParts.join('/');
  } else {
    // More than 2 parent levels, show last 2 with ../ prefix
    return '../' + parentParts.slice(-2).join('/') + '...';
  }
}

/**
 * Expand/Collapse Controls Component
 */
interface ExpandCollapseControlsProps {
  allExpanded: boolean;
  allCollapsed: boolean;
}

function ExpandCollapseControls({ allExpanded, allCollapsed }: ExpandCollapseControlsProps) {
  const { t } = useTranslation();
  const { expandAll, collapseAll } = useContext(CollapseContext);

  return (
    <div className="flex items-center gap-0.5">
      <button
        onClick={expandAll}
        disabled={allExpanded}
        className="p-1 rounded text-muted-foreground hover:text-foreground hover:bg-accent disabled:opacity-30 disabled:cursor-not-allowed disabled:hover:bg-transparent transition-colors"
        title={t('sessions.expandAll')}
      >
        <ChevronsDown className="h-3.5 w-3.5" />
      </button>
      <button
        onClick={collapseAll}
        disabled={allCollapsed}
        className="p-1 rounded text-muted-foreground hover:text-foreground hover:bg-accent disabled:opacity-30 disabled:cursor-not-allowed disabled:hover:bg-transparent transition-colors"
        title={t('sessions.collapseAll')}
      >
        <ChevronsUp className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}

/**
 * Date Header Component
 */
interface DateHeaderProps {
  dateKey: string;
  isFirst: boolean;
  isCollapsed: boolean;
  onToggle: () => void;
  allExpanded: boolean;
  allCollapsed: boolean;
}

function DateHeader({
  dateKey,
  isFirst,
  isCollapsed,
  onToggle,
  allExpanded,
  allCollapsed,
}: DateHeaderProps) {
  const { t, i18n } = useTranslation();

  return (
    <div className="w-full sticky top-0 bg-card z-10 py-2 px-2 flex items-center justify-between hover:bg-accent/50 rounded-md transition-colors app-chrome">
      <button onClick={onToggle} className="flex items-center gap-2 flex-1 text-left">
        {isCollapsed ? (
          <ChevronRight className="h-4 w-4 text-foreground" />
        ) : (
          <ChevronDown className="h-4 w-4 text-foreground" />
        )}
        <h4 className="text-sm font-semibold text-foreground">
          {formatDateGroupLabel(
            dateKey,
            t('sessions.today', 'Today'),
            t('sessions.yesterday', 'Yesterday'),
            i18n.language
          )}
        </h4>
      </button>
      {isFirst && <ExpandCollapseControls allExpanded={allExpanded} allCollapsed={allCollapsed} />}
    </div>
  );
}

/**
 * Directory Header Component
 */
interface DirectoryHeaderProps {
  dirKey: string;
  isFirst: boolean;
  isCollapsed: boolean;
  onToggle: () => void;
  allExpanded: boolean;
  allCollapsed: boolean;
}

function DirectoryHeader({
  dirKey,
  isFirst,
  isCollapsed,
  onToggle,
  allExpanded,
  allCollapsed,
}: DirectoryHeaderProps) {
  const lastDirName = getLastDirectoryName(dirKey);
  const parentPath = formatParentPath(dirKey);

  return (
    <div className="w-full sticky top-0 bg-card z-10 py-2 px-2 flex items-center justify-between hover:bg-accent/50 rounded-md transition-colors app-chrome">
      <button onClick={onToggle} className="flex items-center gap-2 flex-1 text-left min-w-0">
        {isCollapsed ? (
          <ChevronRight className="h-4 w-4 text-foreground shrink-0" />
        ) : (
          <ChevronDown className="h-4 w-4 text-foreground shrink-0" />
        )}
        <Folder className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
        {/* Left side: last directory name */}
        <h4 className="text-sm font-semibold text-foreground shrink-0" title={dirKey}>
          {lastDirName}
        </h4>
        {/* Right side: parent path */}
        {parentPath && (
          <span className="text-xs text-muted-foreground/60 truncate" title={dirKey}>
            {parentPath}
          </span>
        )}
      </button>
      {isFirst && <ExpandCollapseControls allExpanded={allExpanded} allCollapsed={allCollapsed} />}
    </div>
  );
}

/**
 * Session Card Component
 */
interface SessionCardProps {
  session: Session;
  isSelected: boolean;
  onClick: () => void;
  viewMode: ViewMode;
  childCount: number;
  isSubAgent: boolean;
  isChildrenExpanded: boolean;
  onToggleChildren: () => void;
  depth: number;
}

function SessionCard({
  session,
  isSelected,
  onClick,
  viewMode,
  childCount,
  isSubAgent,
  isChildrenExpanded,
  onToggleChildren,
  depth,
}: SessionCardProps) {
  const { t, i18n } = useTranslation();
  // Format date + time for directory view (MM/DD HH:MM)
  const formatDateTime = (timestamp: number) => {
    const date = new Date(timestamp);
    const dateStr = date.toLocaleDateString(i18n.language, {
      month: '2-digit',
      day: '2-digit',
    });
    const timeStr = date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    return `${dateStr} ${timeStr}`;
  };

  return (
    <div
      data-testid="session-card"
      data-session-id={session.id}
      data-session-kind={isSubAgent ? 'subagent' : 'main'}
      style={
        isSubAgent
          ? {
              marginLeft: `${Math.min(depth, 4) * 20}px`,
              width: `calc(100% - ${Math.min(depth, 4) * 20}px)`,
            }
          : undefined
      }
      className={`w-full flex items-center rounded transition-all duration-150 relative group min-w-0 app-chrome ${
        isSelected
          ? 'bg-primary-light text-primary shadow-sm'
          : 'hover:bg-accent/30 text-foreground'
      } ${isSubAgent ? 'border-l border-purple-300/60 dark:border-purple-700/60' : ''}`}
    >
      {/* Left indicator bar for selected state - full height */}
      {isSelected && (
        <div className="absolute left-0 top-0 bottom-0 w-0.5 bg-primary rounded-r-full" />
      )}

      <button
        onClick={onClick}
        className="flex flex-1 items-center gap-2 min-w-0 py-1.5 px-2 text-left"
      >
        {isSubAgent && <Bot className="h-3.5 w-3.5 shrink-0 text-purple-500" />}
        {/* Title: First Message Preview with Marquee */}
        <MarqueeText
          text={session.firstMessage || session.fileName || t('sessions.untitledSession')}
          className={`text-xs flex-1 min-w-0 ${isSelected ? 'text-primary' : 'text-muted-foreground'}`}
        />

        {isSubAgent && session.agentType && (
          <span className="shrink-0 rounded bg-purple-100 px-1.5 py-0.5 text-[10px] text-purple-700 dark:bg-purple-900/40 dark:text-purple-300">
            {session.agentType}
          </span>
        )}

        {/* Time - show only in directory mode */}
        {viewMode === 'directory' && (
          <div className="flex items-center shrink-0">
            <span
              className={`px-1.5 py-0.5 rounded text-[10px] font-medium ${
                isSelected ? 'bg-primary-muted text-primary' : 'bg-muted text-muted-foreground'
              }`}
            >
              {formatDateTime(session.updatedAt)}
            </span>
          </div>
        )}
      </button>

      {childCount > 0 && (
        <button
          type="button"
          data-testid="toggle-subagents"
          onClick={onToggleChildren}
          className="mr-1 flex shrink-0 items-center gap-0.5 rounded px-1.5 py-1 text-[10px] text-purple-600 hover:bg-purple-100 dark:text-purple-300 dark:hover:bg-purple-900/40"
          title={t('sessions.subAgentCount', { count: childCount })}
        >
          {isChildrenExpanded ? (
            <ChevronDown className="h-3.5 w-3.5" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5" />
          )}
          <Bot className="h-3 w-3" />
          <span>{childCount}</span>
        </button>
      )}
    </div>
  );
}

/**
 * Virtual Session List Component
 *
 * Renders a virtualized list of sessions grouped by date or directory
 * Only renders visible items for optimal performance with large lists
 */
export function VirtualSessionList({
  sessions,
  selectedSession,
  onSelect,
  collapsedGroups,
  toggleGroup,
  expandAll,
  collapseAll,
  allExpanded,
  allCollapsed,
  viewMode,
}: VirtualSessionListProps) {
  const { t } = useTranslation();
  const parentRef = useRef<HTMLDivElement>(null);
  const noDirectoryLabel = t('sessions.noDirectoryGroup', '— No Directory —');
  const [expandedParentIds, setExpandedParentIds] = useState<Set<string>>(new Set());
  const { mainSessions, childrenByParent } = useMemo(() => {
    const main: Session[] = [];
    const children = new Map<string, Session[]>();
    for (const session of sessions) {
      if (session.kind !== 'subagent') {
        main.push(session);
        continue;
      }
      if (!session.parentSessionId) continue;
      const siblings = children.get(session.parentSessionId) || [];
      siblings.push(session);
      children.set(session.parentSessionId, siblings);
    }
    for (const siblings of children.values()) {
      siblings.sort(compareSessionsByRecency);
    }
    return { mainSessions: main, childrenByParent: children };
  }, [sessions]);

  useEffect(() => {
    if (selectedSession?.kind !== 'subagent' || !selectedSession.parentSessionId) return;
    setExpandedParentIds((previous) => {
      if (previous.has(selectedSession.parentSessionId!)) return previous;
      const next = new Set(previous);
      next.add(selectedSession.parentSessionId!);
      return next;
    });
  }, [selectedSession]);

  // Use appropriate grouping based on view mode
  const dateGrouped = useDateGroupedSessions(
    mainSessions,
    collapsedGroups,
    childrenByParent,
    expandedParentIds
  );
  const dirGrouped = useDirectoryGroupedSessions(
    mainSessions,
    collapsedGroups,
    noDirectoryLabel,
    childrenByParent,
    expandedParentIds
  );

  const { items } = viewMode === 'date' ? dateGrouped : dirGrouped;

  // Configure virtualizer
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => parentRef.current,
    estimateSize: useCallback(
      (index: number) => {
        const item = items[index];
        return item.type === 'header' ? 40 : item.isSubAgent ? 40 : 36;
      },
      [items]
    ),
    overscan: 5, // Render 5 extra items above/below visible area for smooth scrolling
  });

  // Measure when items change or on mount
  useEffect(() => {
    virtualizer.measure();
  }, [items, virtualizer]);

  // Measure on window resize
  useEffect(() => {
    const handleResize = () => {
      virtualizer.measure();
    };
    window.addEventListener('resize', handleResize);
    return () => window.removeEventListener('resize', handleResize);
  }, [virtualizer]);

  const virtualItems = virtualizer.getVirtualItems();

  return (
    <CollapseContext.Provider
      value={{ collapsedGroups, toggleGroup, expandAll, collapseAll, allExpanded, allCollapsed }}
    >
      <div className="h-full flex flex-col">
        {/* Virtual List */}
        <div ref={parentRef} className="flex-1 overflow-auto" style={{ contain: 'strict' }}>
          <div
            style={{
              height: `${virtualizer.getTotalSize()}px`,
              width: '100%',
              position: 'relative',
            }}
          >
            {virtualItems.map((virtualItem) => {
              const item = items[virtualItem.index];

              return (
                <div
                  key={virtualItem.key}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    transform: `translateY(${virtualItem.start}px)`,
                  }}
                  data-index={virtualItem.index}
                >
                  {item.type === 'header' ? (
                    viewMode === 'date' ? (
                      <DateHeader
                        dateKey={item.groupKey}
                        isFirst={item.isFirst}
                        isCollapsed={collapsedGroups.has(item.groupKey)}
                        onToggle={() => toggleGroup(item.groupKey)}
                        allExpanded={allExpanded}
                        allCollapsed={allCollapsed}
                      />
                    ) : (
                      <DirectoryHeader
                        dirKey={item.groupKey}
                        isFirst={item.isFirst}
                        isCollapsed={collapsedGroups.has(item.groupKey)}
                        onToggle={() => toggleGroup(item.groupKey)}
                        allExpanded={allExpanded}
                        allCollapsed={allCollapsed}
                      />
                    )
                  ) : (
                    <SessionCard
                      session={item.session}
                      isSelected={selectedSession?.id === item.session.id}
                      onClick={() => onSelect(item.session)}
                      viewMode={viewMode}
                      childCount={item.childCount}
                      isSubAgent={item.isSubAgent}
                      depth={item.depth}
                      isChildrenExpanded={expandedParentIds.has(item.session.id)}
                      onToggleChildren={() =>
                        setExpandedParentIds((previous) => {
                          const next = new Set(previous);
                          if (next.has(item.session.id)) next.delete(item.session.id);
                          else next.add(item.session.id);
                          return next;
                        })
                      }
                    />
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </div>
    </CollapseContext.Provider>
  );
}

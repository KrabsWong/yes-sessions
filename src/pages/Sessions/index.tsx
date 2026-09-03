/**
 * Sessions Page
 *
 * View and browse local conversation sessions from AI applications
 */

import { useState, useEffect, useRef, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertCircle,
  ArrowLeft,
  Bot,
  Play,
  AlertTriangle,
  ChevronUp,
  ExternalLink,
  Folder,
  RefreshCw,
} from 'lucide-react';
import { useSidebarResize } from '@/hooks/useSidebarResize';
import { useSettingsStore } from '@/stores/settings';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import {
  useSessions,
  useSessionDetail,
  useSessionSupportStatus,
  useResumeSession,
  useTerminalInfo,
} from '@/hooks/useSessions';
import { ConversationView } from '@/components/sessions/ConversationView';
import { UserMessageNavigator } from '@/components/sessions/UserMessageNavigator';
import { filePreviewApi } from '@/lib/api/files';
import {
  VirtualSessionList,
  formatSessionDateGroupKey,
  getSessionDirectoryGroupKey,
  type ViewMode,
} from '@/components/sessions/VirtualSessionList';
import { APP_LABELS, APP_WEBSITES, APP_COLORS } from '@/config/apps';
import { getAppIcon } from '@/components/AppIcons';
import { Select, SelectContent, SelectItem, SelectTrigger } from '@/components/ui/select';
import { APP_ORDER, isAppSupported } from '@/config/apps';
import type { AppType, Session } from '@/types';

interface SessionsPageProps {
  selectedApp: AppType;
  onAppChange: (app: AppType) => void;
}

/**
 * Truncate text to a specific length with ellipsis
 */
function truncateText(text: string, maxLength: number): string {
  if (!text || text.length <= maxLength) return text || '';
  return text.substring(0, maxLength).trim() + '...';
}

function formatCompactCount(value: number, language: string): string {
  return new Intl.NumberFormat(language, {
    notation: 'compact',
    maximumFractionDigits: 1,
  }).format(value);
}

export function SessionsPage({ selectedApp, onAppChange }: SessionsPageProps) {
  const { t, i18n } = useTranslation();
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [pendingSubAgentTarget, setPendingSubAgentTarget] = useState<{
    sessionId: string;
    appType: AppType;
  } | null>(null);

  // Sidebar collapse state
  const { sidebarCollapsed } = useSettingsStore();

  // 侧边栏拖拽调整宽度
  const {
    isResizing,
    startResizing,
    style: sidebarStyle,
  } = useSidebarResize({
    initialWidth: 320,
    minWidth: 160, // 当前宽度的一半
    collapsed: sidebarCollapsed,
  });

  // View mode and collapse state for session list
  const [viewMode, setViewMode] = useState<ViewMode>('date');
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set());

  const scrollContainerRef = useRef<HTMLDivElement>(null);

  // New messages notification
  const [newMessagesCount, setNewMessagesCount] = useState(0);
  const [showNewMessagesTip, setShowNewMessagesTip] = useState(false);

  // Listen to scroll events for hiding new messages tip when at bottom
  useEffect(() => {
    const setupScrollListener = () => {
      const container = scrollContainerRef.current;
      if (!container) {
        // Retry after a short delay if ref is not ready
        setTimeout(setupScrollListener, 100);
        return;
      }

      const handleScroll = () => {
        // Hide new messages tip if user scrolls to bottom
        const { scrollHeight, scrollTop, clientHeight } = container;
        const scrollBottom = Math.ceil(scrollTop + clientHeight);
        const isAtBottom = scrollHeight - scrollBottom <= 50;

        if (showNewMessagesTip && isAtBottom) {
          setShowNewMessagesTip(false);
          setNewMessagesCount(0);
        }
      };

      container.addEventListener('scroll', handleScroll);
      // Check initial scroll position
      handleScroll();

      return () => container.removeEventListener('scroll', handleScroll);
    };

    const cleanup = setupScrollListener();
    return cleanup;
  }, [selectedSession, showNewMessagesTip]); // Re-bind when session or tip visibility changes

  const { data: sessions, isLoading, error } = useSessions(selectedApp);
  const mainSessions = useMemo(
    () => sessions?.filter((session) => session.kind !== 'subagent') || [],
    [sessions]
  );
  const stats = useMemo(() => {
    if (!sessions) return null;
    return {
      totalSessions: mainSessions.length,
      subAgentSessions: sessions.length - mainSessions.length,
      totalMessages: sessions.reduce((sum, session) => sum + session.messageCount, 0),
    };
  }, [sessions, mainSessions]);
  const { data: supportStatus } = useSessionSupportStatus(selectedApp);
  const {
    data: sessionDetail,
    isLoading: isLoadingDetail,
    isError: isSessionDetailError,
  } = useSessionDetail(selectedSession?.id || '', selectedApp, selectedSession?.updatedAt);
  const { data: terminalInfo } = useTerminalInfo();
  const resumeMutation = useResumeSession();

  // Auto-select first session when sessions are loaded
  useEffect(() => {
    if (mainSessions.length > 0 && !selectedSession) {
      setSelectedSession(mainSessions[0]);
    }
  }, [mainSessions, selectedSession]);

  // Reset selected session when app changes
  useEffect(() => {
    setSelectedSession(null);
  }, [selectedApp]);

  useEffect(() => {
    if (!pendingSubAgentTarget || pendingSubAgentTarget.appType !== selectedApp || !sessions) {
      return;
    }
    const matchingSession = sessions.find(
      (session) =>
        session.id === pendingSubAgentTarget.sessionId ||
        session.uuid === pendingSubAgentTarget.sessionId
    );
    setSelectedSession(
      matchingSession || {
        id: pendingSubAgentTarget.sessionId,
        appType: pendingSubAgentTarget.appType,
        fileName: pendingSubAgentTarget.sessionId,
        filePath: '',
        createdAt: Date.now(),
        updatedAt: Date.now(),
        messageCount: 0,
        kind: 'subagent',
      }
    );
    setPendingSubAgentTarget(null);
  }, [pendingSubAgentTarget, selectedApp, sessions]);

  const isSupported = supportStatus?.supported ?? false;
  const parentSession =
    selectedSession?.kind === 'subagent' && selectedSession.parentSessionId
      ? sessions?.find(
          (session) =>
            session.id === selectedSession.parentSessionId ||
            session.uuid === selectedSession.parentSessionId
        )
      : undefined;

  const handleSessionSelect = (session: Session) => {
    setSelectedSession(session);
  };

  const handleViewSubAgentSession = (sessionId: string, appType: string) => {
    if (!APP_ORDER.includes(appType as AppType)) return;
    const targetApp = appType as AppType;
    if (targetApp !== selectedApp) {
      setPendingSubAgentTarget({ sessionId, appType: targetApp });
      onAppChange(targetApp);
      return;
    }
    const matchingSession = sessions?.find(
      (session) => session.id === sessionId || session.uuid === sessionId
    );
    setSelectedSession(
      matchingSession || {
        id: sessionId,
        appType,
        fileName: sessionId,
        filePath: '',
        directory: selectedSession?.directory,
        createdAt: Date.now(),
        updatedAt: Date.now(),
        messageCount: 0,
        kind: 'subagent',
      }
    );
  };

  // Toggle collapse state for a group
  const toggleGroup = (groupKey: string) => {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(groupKey)) {
        next.delete(groupKey);
      } else {
        next.add(groupKey);
      }
      return next;
    });
  };

  // Expand/collapse all groups
  const expandAll = () => setCollapsedGroups(new Set());
  const collapseAll = () => {
    if (mainSessions.length === 0) return;
    const allGroups = new Set<string>();
    mainSessions.forEach((session) => {
      const groupKey =
        viewMode === 'date'
          ? formatSessionDateGroupKey(session.updatedAt)
          : getSessionDirectoryGroupKey(session, t('sessions.noDirectoryGroup'));
      allGroups.add(groupKey);
    });
    setCollapsedGroups(allGroups);
  };

  const allExpanded = collapsedGroups.size === 0;
  const allCollapsed =
    mainSessions.length > 0
      ? collapsedGroups.size ===
        new Set(
          mainSessions.map((s) =>
            viewMode === 'date'
              ? formatSessionDateGroupKey(s.updatedAt)
              : getSessionDirectoryGroupKey(s, t('sessions.noDirectoryGroup'))
          )
        ).size
      : false;

  // Handle resuming session
  const handleResumeSession = async () => {
    if (!selectedSession) return;

    await resumeMutation.mutateAsync({
      sessionId: selectedSession.id,
      appType: selectedApp,
      workingDir: selectedSession.directory,
    });
  };

  // Check if an external terminal supports resume.
  const canResume =
    terminalInfo?.ghosttyInstalled ||
    terminalInfo?.kittyInstalled ||
    terminalInfo?.preferred === 'terminal';

  const handleOpenFilePreview = async () => {
    if (!selectedSession?.directory) return;
    const sessionTitle = selectedSession.firstMessage
      ? selectedSession.firstMessage.slice(0, 50)
      : selectedSession.fileName || t('sessions.untitledSession');
    await filePreviewApi.open(selectedSession.directory, sessionTitle);
  };

  return (
    <div className="flex flex-col h-full overflow-hidden">
      {/* Main Content */}
      <div className={cn('flex gap-0 flex-1 min-h-0 p-4')}>
        {/* Session List - Sidebar - with animation */}
        <div
          className={cn(
            'flex flex-col h-full min-h-0 border-r bg-card/50 overflow-hidden shrink-0 transition-[width,opacity] duration-300 ease-in-out',
            sidebarCollapsed && 'border-r-0'
          )}
          style={sidebarStyle}
        >
          {/* Sidebar Content - with fade animation */}
          <div
            className={cn(
              'flex flex-col min-h-0 flex-1 transition-opacity duration-200 ease-in-out',
              sidebarCollapsed ? 'opacity-0' : 'opacity-100'
            )}
            style={{
              transitionDelay: sidebarCollapsed ? '0ms' : '150ms',
            }}
          >
            {/* App Selector - First item in sidebar */}
            <div className="px-3 py-2 border-b border-border/40 bg-card">
              <Select value={selectedApp} onValueChange={(value) => onAppChange(value as AppType)}>
                <SelectTrigger className="w-full h-9">
                  <div className="flex items-center gap-2">
                    <span className={APP_COLORS[selectedApp]}>{getAppIcon(selectedApp, 16)}</span>
                    <span className="truncate font-medium">{APP_LABELS[selectedApp]}</span>
                  </div>
                </SelectTrigger>
                <SelectContent className="min-w-[18rem]">
                  {APP_ORDER.map((app) => {
                    const supported = isAppSupported(app);
                    return (
                      <SelectItem
                        key={app}
                        value={app}
                        disabled={!supported}
                        className={!supported ? 'opacity-50 cursor-not-allowed' : ''}
                      >
                        <div className="flex items-center gap-2">
                          <span className={APP_COLORS[app]}>{getAppIcon(app, 16)}</span>
                          <span>{APP_LABELS[app]}</span>
                          {!supported && (
                            <span className="text-xs text-muted-foreground ml-2">
                              ({t('sessions.comingSoon')})
                            </span>
                          )}
                        </div>
                      </SelectItem>
                    );
                  })}
                </SelectContent>
              </Select>
            </div>

            {/* List Header */}
            <div className="flex items-center justify-between gap-3 px-3 py-1.5 border-b border-border/40 bg-card">
              {/* Stats */}
              {isSupported && stats && (
                <div
                  data-testid="session-stats"
                  className="flex min-w-0 flex-1 flex-nowrap items-center gap-2 overflow-hidden whitespace-nowrap text-[11px] tabular-nums text-muted-foreground"
                >
                  <span className="shrink-0">
                    {formatCompactCount(stats.totalSessions, i18n.language)}{' '}
                    {t('sessions.sessionsLabel')}
                  </span>
                  {stats.subAgentSessions > 0 && (
                    <>
                      <span className="text-border">·</span>
                      <span className="shrink-0">
                        {formatCompactCount(stats.subAgentSessions, i18n.language)}{' '}
                        {t('sessions.subAgentsLabel')}
                      </span>
                    </>
                  )}
                  <span className="text-border">·</span>
                  <span className="shrink-0">
                    {formatCompactCount(stats.totalMessages, i18n.language)}{' '}
                    {t('sessions.messagesLabel')}
                  </span>
                </div>
              )}

              {/* View Mode Toggle - Icon buttons */}
              <div className="flex shrink-0 items-center bg-primary-muted rounded-md p-0.5">
                <button
                  onClick={() => setViewMode('date')}
                  className={`px-2 py-1 text-[10px] font-medium rounded-sm transition-all ${
                    viewMode === 'date'
                      ? 'bg-card text-primary shadow-sm'
                      : 'text-muted-foreground hover:text-primary'
                  }`}
                  title={t('sessions.viewByDate')}
                >
                  {t('sessions.date')}
                </button>
                <button
                  onClick={() => setViewMode('directory')}
                  className={`px-2 py-1 text-[10px] font-medium rounded-sm transition-all ${
                    viewMode === 'directory'
                      ? 'bg-card text-primary shadow-sm'
                      : 'text-muted-foreground hover:text-primary'
                  }`}
                  title={t('sessions.viewByDirectory')}
                >
                  {t('sessions.directory')}
                </button>
              </div>
            </div>

            {isLoading ? (
              <div className="flex items-center justify-center h-64">
                <div className="text-muted-foreground">{t('sessions.loading')}</div>
              </div>
            ) : error ? (
              <div className="flex items-center justify-center h-64">
                <div className="text-destructive flex items-center gap-2">
                  <AlertCircle className="h-5 w-5" />
                  {t('sessions.error')}
                </div>
              </div>
            ) : !isSupported ? (
              <div className="flex flex-col items-center justify-center h-64 space-y-4 p-4">
                <div className="text-muted-foreground text-center">
                  <p className="text-lg font-medium mb-2">{t('sessions.comingSoon')}</p>
                  <p className="text-sm">{t('sessions.unsupportedApp')}</p>
                </div>
                <Button variant="outline" onClick={() => onAppChange('claude')}>
                  {t('sessions.switchToClaude')}
                </Button>
              </div>
            ) : !supportStatus?.isAvailable ? (
              <div className="flex flex-col items-center justify-center h-64 space-y-4 p-4">
                <div className="text-muted-foreground text-center">
                  <p className="text-lg font-medium mb-2">{t('sessions.notInstalled')}</p>
                  <p className="text-sm">
                    {t('sessions.notInstalledDesc', { app: APP_LABELS[selectedApp] })}
                  </p>
                </div>
                <Button
                  variant="outline"
                  onClick={() => window.open(APP_WEBSITES[selectedApp], '_blank')}
                  className="flex items-center gap-2"
                >
                  <ExternalLink className="h-4 w-4" />
                  {t('sessions.visitWebsite')}
                </Button>
              </div>
            ) : sessions?.length === 0 ? (
              <div className="flex items-center justify-center h-64">
                <div className="text-muted-foreground text-center">
                  <p className="text-lg font-medium mb-2">{t('sessions.noSessions')}</p>
                  <p className="text-sm">{t('sessions.noSessionsDesc')}</p>
                </div>
              </div>
            ) : (
              <div className="flex-1 min-h-0 p-2">
                <VirtualSessionList
                  sessions={sessions || []}
                  selectedSession={selectedSession}
                  onSelect={handleSessionSelect}
                  collapsedGroups={collapsedGroups}
                  toggleGroup={toggleGroup}
                  expandAll={expandAll}
                  collapseAll={collapseAll}
                  allExpanded={allExpanded}
                  allCollapsed={allCollapsed}
                  viewMode={viewMode}
                />
              </div>
            )}
          </div>{' '}
          {/* Sidebar Content */}
        </div>{' '}
        {/* Sidebar Container */}
        {/* Resizable Divider - with animation */}
        <div
          onMouseDown={startResizing}
          className={cn(
            'shrink-0 cursor-col-resize transition-[width,opacity] duration-300 ease-in-out hover:bg-primary/50',
            isResizing && 'bg-primary/50',
            sidebarCollapsed ? 'w-0 opacity-0' : 'w-1 opacity-100'
          )}
          title={t('common.resizeSidebar')}
        />
        {/* Session Detail */}
        <div className="overflow-hidden flex flex-col h-full min-h-0 relative transition-all duration-300 flex-1">
          {selectedSession ? (
            <>
              {/* Header */}
              <div className="flex items-start justify-between border-b p-4 shrink-0 gap-4">
                <div className="flex-1 min-w-0">
                  {/* Title row */}
                  <div className="flex items-center gap-2">
                    {selectedSession.kind === 'subagent' && (
                      <span className="inline-flex shrink-0 items-center gap-1 rounded bg-purple-100 px-2 py-1 text-xs font-medium text-purple-700 dark:bg-purple-900/40 dark:text-purple-300">
                        <Bot className="h-3.5 w-3.5" />
                        {t('sessions.subAgent')}
                        {selectedSession.agentType && ` · ${selectedSession.agentType}`}
                      </span>
                    )}
                    <h3 className="font-semibold truncate">
                      {selectedSession.firstMessage
                        ? truncateText(selectedSession.firstMessage, 100)
                        : selectedSession.fileName || t('sessions.untitledSession')}
                    </h3>
                  </div>

                  {/* Metadata row - Session ID, Work */}
                  <div className="flex flex-col gap-1 mt-2">
                    {selectedSession.kind === 'subagent' && parentSession && (
                      <button
                        type="button"
                        onClick={() => setSelectedSession(parentSession)}
                        className="flex w-fit items-center gap-1.5 text-xs text-purple-600 hover:text-purple-700 dark:text-purple-300 dark:hover:text-purple-200"
                      >
                        <ArrowLeft className="h-3.5 w-3.5" />
                        {t('sessions.backToParentSession')}
                      </button>
                    )}
                    {/* Session ID with Resume Button */}
                    {selectedSession.id && (
                      <div className="flex items-center gap-2">
                        <span className="text-[10px] text-muted-foreground uppercase tracking-wider shrink-0">
                          {t('sessions.sessionId')}
                        </span>
                        <button
                          onClick={() => {
                            navigator.clipboard.writeText(selectedSession.id || '');
                          }}
                          className="text-xs font-mono truncate hover:text-primary transition-colors"
                          title={t('sessions.copySessionId')}
                        >
                          {selectedSession.id}
                        </button>
                        {/* Resume Button - small, right after Session ID */}
                        {selectedSession.kind !== 'subagent' && (
                          <TooltipProvider delayDuration={200}>
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <Button
                                  variant="default"
                                  size="sm"
                                  className="h-6 px-2 py-0 text-[10px] flex items-center gap-1 shrink-0 bg-foreground text-background hover:bg-foreground/90 ml-1"
                                  onClick={handleResumeSession}
                                  disabled={!canResume || resumeMutation.isPending}
                                >
                                  {resumeMutation.isPending ? (
                                    <>
                                      <RefreshCw className="h-3 w-3 animate-spin" />
                                      <span>{t('sessions.resuming')}</span>
                                    </>
                                  ) : (
                                    <>
                                      <Play className="h-3 w-3" />
                                      <span>{t('sessions.resume')}</span>
                                    </>
                                  )}
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent side="bottom">
                                <p>
                                  {!canResume
                                    ? t('sessions.installTerminalTip')
                                    : t('sessions.resumeInTerminal', {
                                        terminal:
                                          terminalInfo?.preferred === 'ghostty'
                                            ? 'Ghostty'
                                            : terminalInfo?.preferred === 'kitty'
                                              ? 'Kitty'
                                              : 'Terminal',
                                      })}
                                </p>
                              </TooltipContent>
                            </Tooltip>
                          </TooltipProvider>
                        )}
                      </div>
                    )}

                    {/* Last Updated */}
                    <div className="flex items-center gap-2">
                      <span className="text-[10px] text-muted-foreground uppercase tracking-wider shrink-0">
                        {t('sessions.updated')}
                      </span>
                      <span className="text-xs text-muted-foreground font-mono">
                        {new Date(selectedSession.updatedAt).toLocaleString(i18n.language, {
                          year: 'numeric',
                          month: '2-digit',
                          day: '2-digit',
                          hour: '2-digit',
                          minute: '2-digit',
                        })}
                      </span>
                    </div>

                    {/* Working Directory */}
                    {selectedSession.directory && (
                      <div className="flex items-center gap-2">
                        <span className="text-[10px] text-muted-foreground uppercase tracking-wider">
                          {t('sessions.work')}
                        </span>
                        <span className="text-xs text-muted-foreground font-mono truncate">
                          {selectedSession.directory}
                        </span>
                      </div>
                    )}
                  </div>
                </div>
                {/* File preview window button */}
                <div className="flex items-center">
                  <TooltipProvider delayDuration={200}>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon"
                          data-testid="open-file-preview-button"
                          className="h-8 w-8 shrink-0"
                          onClick={handleOpenFilePreview}
                        >
                          <Folder className="h-4 w-4" />
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent side="bottom">
                        <p>{t('sessions.showFiles')}</p>
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                </div>
              </div>

              {/* Messages */}
              <div
                ref={scrollContainerRef}
                id="conversation-scroll-container"
                data-testid="conversation-detail"
                className="flex-1 overflow-y-auto p-4 min-h-0 relative"
              >
                {isLoadingDetail ? (
                  <div className="flex items-center justify-center h-full">
                    <div className="text-muted-foreground flex items-center gap-2">
                      <RefreshCw className="h-5 w-5 animate-spin" />
                      {t('sessions.loadingConversation')}
                    </div>
                  </div>
                ) : isSessionDetailError ? (
                  <div className="flex items-center justify-center h-full text-destructive">
                    <div className="flex flex-col items-center gap-2">
                      <AlertCircle className="h-8 w-8 opacity-70" />
                      <p>{t('sessions.error')}</p>
                    </div>
                  </div>
                ) : sessionDetail?.messages && sessionDetail.messages.length > 0 ? (
                  <>
                    <ConversationView
                      messages={sessionDetail.messages}
                      appType={selectedApp}
                      onViewSubAgentSession={handleViewSubAgentSession}
                      onNewMessages={(count, isAtBottom) => {
                        if (!isAtBottom) {
                          // User is not at bottom, show tip
                          setNewMessagesCount((prev) => prev + count);
                          setShowNewMessagesTip(true);
                        }
                        // If at bottom, auto-scroll is handled by ConversationView
                      }}
                    />
                    {/* User Message Navigator - Quick jump to user messages */}
                    <UserMessageNavigator messages={sessionDetail.messages} />
                  </>
                ) : (
                  <div className="flex items-center justify-center h-full text-muted-foreground">
                    <div className="flex flex-col items-center gap-2">
                      <AlertTriangle className="h-8 w-8 opacity-50" />
                      <p>{t('sessions.noMessages')}</p>
                    </div>
                  </div>
                )}
              </div>

              {/* New messages notification - absolute at bottom center */}
              {showNewMessagesTip && (
                <button
                  onClick={() => {
                    // Scroll to bottom to see new messages
                    if (scrollContainerRef.current) {
                      scrollContainerRef.current.scrollTo({
                        top: scrollContainerRef.current.scrollHeight,
                        behavior: 'auto',
                      });
                    }
                    // Hide the tip
                    setShowNewMessagesTip(false);
                    setNewMessagesCount(0);
                  }}
                  className="absolute bottom-4 left-1/2 -translate-x-1/2 z-50 flex items-center gap-2 px-4 py-2 bg-primary text-primary-foreground rounded-full shadow-lg hover:bg-primary-hover transition-all"
                >
                  <span className="flex h-2 w-2 relative">
                    <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-primary-foreground opacity-75"></span>
                    <span className="relative inline-flex rounded-full h-2 w-2 bg-primary-foreground"></span>
                  </span>
                  <span className="text-xs font-medium">
                    {t('sessions.newMessages', { count: newMessagesCount })}
                  </span>
                </button>
              )}
            </>
          ) : (
            /* Empty State */
            <div className="flex items-center justify-center h-full text-muted-foreground">
              <div className="flex flex-col items-center gap-4">
                <div className="w-16 h-16 rounded-full bg-primary-muted flex items-center justify-center">
                  <ChevronUp className="h-8 w-8 opacity-50" />
                </div>
                <div className="text-center">
                  <p className="text-lg font-medium mb-1">{t('sessions.emptyStateTitle')}</p>
                  <p className="text-sm max-w-md">{t('sessions.emptyStateDesc')}</p>
                </div>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

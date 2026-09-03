/**
 * Conversation View Component
 *
 * Renders a conversation session with support for:
 * - User messages
 * - Assistant responses
 * - Tool calls (tool_use)
 * - Tool results (tool_result)
 * - MCP calls and sub-agent calls
 */

import { useMemo, useRef, useEffect, memo, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useVirtualizer } from '@tanstack/react-virtual';
import { cn } from '@/lib/utils';
import { getAppIcon } from '@/components/AppIcons';
import { APP_LABELS } from '@/config/apps';
import type { AppType } from '@/types';
import type { SessionMessage } from '@/types/session';
import type { ConversationViewProps, ConversationTurnProps, MessageTurnWithCount } from './types';

import { groupMessagesIntoTurnsWithCount, verifyMessageCount } from './utils';
import { SystemMessage, UserMessage, AssistantMessage } from './MessageTypes';
import { ToolCallBlock } from './ToolCallBlock';
import { useSettingsStore } from '@/stores/settings';

const VIRTUALIZE_TURN_THRESHOLD = 150;
const VIRTUALIZE_MESSAGE_THRESHOLD = 800;
const COLLAPSE_TOOL_CALLS_THRESHOLD = 25;

// Conversation Turn Component
const ConversationTurn = memo(function ConversationTurn({
  turn,
  appType,
  onViewSubAgentSession,
  userMessageIndex,
}: ConversationTurnProps & { userMessageIndex?: number }) {
  const { t } = useTranslation();
  const { chatLayout } = useSettingsStore();
  const isBubble = chatLayout === 'bubble';
  return (
    <div className="space-y-3">
      {/* System Messages */}
      {turn.systemMessages.length > 0 && (
        <div className="space-y-1">
          {turn.systemMessages.map((sysMsg, index) => (
            <SystemMessage
              key={index}
              content={sysMsg.content || sysMsg.redacted_content || ''}
              timestamp={sysMsg.timestamp}
              metadata={sysMsg.metadata}
              model={sysMsg.model}
            />
          ))}
        </div>
      )}

      {/* User Message */}
      {(turn.userMessage?.content || turn.userMessage?.redacted_content) && (
        <div id={userMessageIndex !== undefined ? `user-message-${userMessageIndex}` : undefined}>
          <UserMessage
            content={turn.userMessage.content || turn.userMessage.redacted_content || ''}
            timestamp={turn.userMessage.timestamp}
            appType={appType}
            model={turn.userMessage.model}
          />
        </div>
      )}

      {/* Agent Response */}
      {turn.toolCalls.length > 0 ? (
        <div className="flex gap-3">
          <div className="flex-shrink-0 w-8 h-8 rounded-full bg-primary-muted flex items-center justify-center">
            {getAppIcon(appType as AppType, 18)}
          </div>
          <div className={cn('min-w-0', isBubble ? 'flex-1 max-w-[calc(100%-120px)]' : 'flex-1')}>
            <div className="flex items-center gap-2 mb-1">
              <span className="font-medium text-sm">
                {APP_LABELS[appType as AppType] || APP_LABELS.claude}
              </span>
              {turn.assistantMessage?.model && (
                <span
                  className="text-xs px-1.5 py-0.5 rounded bg-primary-muted text-primary"
                  title={t('sessions.model')}
                >
                  {turn.assistantMessage.model}
                </span>
              )}
            </div>
            <div className="space-y-2">
              {turn.toolCalls.map((toolCall, index) => (
                <ToolCallBlock
                  key={index}
                  toolUse={toolCall.toolUse}
                  toolResult={toolCall.toolResult}
                  onViewSubAgentSession={onViewSubAgentSession}
                  defaultCollapsed={turn.toolCalls.length > COLLAPSE_TOOL_CALLS_THRESHOLD}
                />
              ))}
            </div>
            {(turn.assistantMessage?.content ||
              turn.assistantMessage?.reasoning_content ||
              turn.assistantMessage?.redacted_content) && (
              <div className="mt-3">
                <AssistantMessage
                  hideAvatar
                  content={
                    turn.assistantMessage.content || turn.assistantMessage.redacted_content || ''
                  }
                  reasoningContent={turn.assistantMessage.reasoning_content}
                  timestamp={turn.assistantMessage.timestamp}
                  appType={appType}
                  model={turn.assistantMessage.model}
                />
              </div>
            )}
          </div>
        </div>
      ) : turn.assistantMessage?.content ||
        turn.assistantMessage?.reasoning_content ||
        turn.assistantMessage?.redacted_content ? (
        <AssistantMessage
          content={turn.assistantMessage.content || turn.assistantMessage.redacted_content || ''}
          reasoningContent={turn.assistantMessage.reasoning_content}
          timestamp={turn.assistantMessage.timestamp}
          appType={appType}
          model={turn.assistantMessage.model}
        />
      ) : null}
    </div>
  );
});

// Main Conversation View Component
export function ConversationView({
  messages,
  className,
  appType = 'claude',
  onViewSubAgentSession,
  onNewMessages,
}: Omit<ConversationViewProps, 'onLoadAll' | 'shouldLoadAll' | 'onLoadAllComplete'>) {
  const turnsWithCount = useMemo(() => {
    const turns = groupMessagesIntoTurnsWithCount(messages, appType);
    verifyMessageCount(turns, messages, appType);
    return turns;
  }, [messages, appType]);

  const prevMessagesLengthRef = useRef(messages.length);
  const prevLastMessageHashRef = useRef<string>('');
  const isAtBottomRef = useRef(true);
  const prevLastTurnHashRef = useRef<string>('');
  const prevTurnCountRef = useRef(turnsWithCount.length);

  const getTurnHash = (turn: MessageTurnWithCount | undefined): string => {
    if (!turn) return '';
    const userContent = turn.userMessage?.content || turn.userMessage?.redacted_content || '';
    const assistantContent = turn.assistantMessage?.content || '';
    const reasoningContent = turn.assistantMessage?.reasoning_content || '';
    const redactedContent = turn.assistantMessage?.redacted_content || '';
    const toolCount = turn.toolCalls.length;
    return `${userContent.length}:${assistantContent.length}:${reasoningContent.length}:${redactedContent.length}:${toolCount}`;
  };

  const getMessageHash = (msg: SessionMessage | undefined): string => {
    if (!msg) return '';
    return `${msg.type}:${msg.content || ''}:${msg.reasoning_content || ''}:${msg.redacted_content || ''}:${JSON.stringify(msg.tool_output || {})}`;
  };

  const getScrollContainer = useCallback(() => {
    return document.getElementById('conversation-scroll-container');
  }, []);

  const autoScrollToBottom = useCallback(
    (behavior: ScrollBehavior = 'auto') => {
      const container = getScrollContainer();
      if (container) {
        container.scrollTo({
          top: container.scrollHeight,
          behavior,
        });
      }
    },
    [getScrollContainer]
  );

  const shouldVirtualize =
    turnsWithCount.length > VIRTUALIZE_TURN_THRESHOLD ||
    messages.length > VIRTUALIZE_MESSAGE_THRESHOLD;
  const userMessageTurnIndex = useMemo(() => {
    const indexByMessageIndex = new Map<number, number>();
    turnsWithCount.forEach((turn, turnIndex) => {
      if (turn.userMessageOriginalIndex !== undefined) {
        indexByMessageIndex.set(turn.userMessageOriginalIndex, turnIndex);
      }
    });
    return indexByMessageIndex;
  }, [turnsWithCount]);

  const turnVirtualizer = useVirtualizer({
    count: turnsWithCount.length,
    getScrollElement: getScrollContainer,
    estimateSize: (index) => {
      const turn = turnsWithCount[index];
      if (!turn) return 180;
      return 120 + turn.toolCalls.length * 96 + turn.systemMessages.length * 56;
    },
    overscan: 8,
    enabled: shouldVirtualize,
  });

  // Track scroll position
  useEffect(() => {
    const container = getScrollContainer();
    if (!container) return;

    const handleScroll = () => {
      const { scrollHeight, scrollTop, clientHeight } = container;
      const scrollBottom = Math.ceil(scrollTop + clientHeight);
      isAtBottomRef.current = scrollHeight - scrollBottom <= 50;
    };

    container.addEventListener('scroll', handleScroll, { passive: true });
    handleScroll();

    return () => container.removeEventListener('scroll', handleScroll);
  }, [getScrollContainer]);

  // Detect new messages
  useEffect(() => {
    const currentMessagesLength = messages.length;
    const prevMessagesLength = prevMessagesLengthRef.current;
    const lastMessage = messages[messages.length - 1];
    const currentLastMessageHash = getMessageHash(lastMessage);
    const prevLastMessageHash = prevLastMessageHashRef.current;

    if (currentMessagesLength > prevMessagesLength) {
      const newCount = currentMessagesLength - prevMessagesLength;
      if (isAtBottomRef.current) {
        // Auto-scroll when new messages arrive and user is at bottom
        setTimeout(() => {
          autoScrollToBottom('auto');
        }, 50);
      }
      if (onNewMessages) {
        onNewMessages(newCount, isAtBottomRef.current);
      }
    } else if (
      currentMessagesLength === prevMessagesLength &&
      currentMessagesLength > 0 &&
      currentLastMessageHash !== prevLastMessageHash
    ) {
      // Content update (streaming)
      if (isAtBottomRef.current) {
        setTimeout(() => {
          autoScrollToBottom('auto');
        }, 50);
      }
    }

    prevMessagesLengthRef.current = currentMessagesLength;
    prevLastMessageHashRef.current = currentLastMessageHash;
  }, [messages, onNewMessages, autoScrollToBottom]);

  // Auto-scroll when turns change
  useEffect(() => {
    const currentTurnCount = turnsWithCount.length;
    const lastTurn = turnsWithCount[turnsWithCount.length - 1];
    const currentLastTurnHash = getTurnHash(lastTurn);
    const prevLastTurnHash = prevLastTurnHashRef.current;

    const hasNewTurn = currentTurnCount > prevTurnCountRef.current;
    const hasContentUpdate = currentLastTurnHash !== prevLastTurnHash && prevLastTurnHash !== '';

    if (isAtBottomRef.current && (hasNewTurn || hasContentUpdate)) {
      setTimeout(() => {
        autoScrollToBottom('auto');
      }, 50);
    }

    prevLastTurnHashRef.current = currentLastTurnHash;
    prevTurnCountRef.current = currentTurnCount;
  }, [turnsWithCount, autoScrollToBottom]);

  useEffect(() => {
    if (!shouldVirtualize) return;

    const handleJumpToUserMessage = (event: Event) => {
      const messageIndex = (event as CustomEvent<{ messageIndex?: number }>).detail?.messageIndex;
      if (messageIndex === undefined) return;

      const turnIndex = userMessageTurnIndex.get(messageIndex);
      if (turnIndex === undefined) return;

      turnVirtualizer.scrollToIndex(turnIndex, { align: 'start' });
    };

    window.addEventListener('conversation:jump-to-user-message', handleJumpToUserMessage);
    return () => {
      window.removeEventListener('conversation:jump-to-user-message', handleJumpToUserMessage);
    };
  }, [shouldVirtualize, turnVirtualizer, userMessageTurnIndex]);

  if (shouldVirtualize) {
    return (
      <div
        className={cn('relative', className)}
        style={{
          height: `${turnVirtualizer.getTotalSize()}px`,
        }}
      >
        {turnVirtualizer.getVirtualItems().map((virtualItem) => {
          const turn = turnsWithCount[virtualItem.index];
          return (
            <div
              key={virtualItem.key}
              data-index={virtualItem.index}
              ref={turnVirtualizer.measureElement}
              className="absolute left-0 top-0 w-full py-3"
              style={{
                transform: `translateY(${virtualItem.start}px)`,
              }}
            >
              <ConversationTurn
                turn={turn}
                appType={appType}
                onViewSubAgentSession={onViewSubAgentSession}
                userMessageIndex={turn.userMessageOriginalIndex}
              />
            </div>
          );
        })}
      </div>
    );
  }

  return (
    <div className={cn('space-y-6 relative', className)}>
      {turnsWithCount.map((turn, index) => (
        <ConversationTurn
          key={index}
          turn={turn}
          appType={appType}
          onViewSubAgentSession={onViewSubAgentSession}
          userMessageIndex={turn.userMessageOriginalIndex}
        />
      ))}
    </div>
  );
}

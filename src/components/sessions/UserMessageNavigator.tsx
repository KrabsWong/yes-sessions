/**
 * User Message Navigator Component
 *
 * Shows a mini navigation panel on the right side of the conversation view
 * that allows users to quickly jump to their sent messages.
 */

import { useState, useMemo, useCallback, useEffect } from 'react';
import { cn } from '@/lib/utils';
import type { SessionMessage } from '@/types/session';

interface UserMessageNavigatorProps {
  messages: SessionMessage[];
  className?: string;
}

interface UserMessageItem {
  originalIndex: number;
  content: string;
  timestamp: string;
}

const MAX_VISIBLE_ITEMS = 8;

function extractUserMessages(messages: SessionMessage[]): UserMessageItem[] {
  return messages
    .map((msg, originalIndex) => ({ msg, originalIndex }))
    .filter(
      ({ msg }) =>
        msg.type === 'user' &&
        Boolean((msg.content || msg.redacted_content || '').trim().length > 0)
    )
    .map(({ msg, originalIndex }) => ({
      originalIndex,
      content: msg.content || msg.redacted_content || '',
      timestamp: msg.timestamp,
    }));
}

function getUserMessageElementId(id: number): string {
  return `user-message-${id}`;
}

function getPreview(text: string, maxChars: number = 28): string {
  if (!text) return '...';
  const cleanText = text.replace(/\n/g, ' ').replace(/\s+/g, ' ').trim();
  if (cleanText.length <= maxChars) return cleanText;
  return cleanText.substring(0, maxChars).trim() + '...';
}

export function UserMessageNavigator({ messages, className }: UserMessageNavigatorProps) {
  const [isHovered, setIsHovered] = useState(false);
  const [activeIndex, setActiveIndex] = useState<number>(0);

  const userMessages = useMemo(() => extractUserMessages(messages), [messages]);

  useEffect(() => {
    const scrollContainer = document.getElementById('conversation-scroll-container');
    if (!scrollContainer || userMessages.length === 0) return;

    const handleScroll = () => {
      const containerRect = scrollContainer.getBoundingClientRect();
      const containerCenter = containerRect.top + containerRect.height / 2;

      let closestIdx = 0;
      let closestDistance = Infinity;

      userMessages.forEach((msg, idx) => {
        const element = document.getElementById(getUserMessageElementId(msg.originalIndex));
        if (element) {
          const rect = element.getBoundingClientRect();
          const elementCenter = rect.top + rect.height / 2;
          const distance = Math.abs(elementCenter - containerCenter);

          if (distance < closestDistance) {
            closestDistance = distance;
            closestIdx = idx;
          }
        }
      });

      setActiveIndex(closestIdx);
    };

    scrollContainer.addEventListener('scroll', handleScroll, { passive: true });
    handleScroll();

    return () => scrollContainer.removeEventListener('scroll', handleScroll);
  }, [userMessages]);

  const scrollToMessage = useCallback((item: UserMessageItem, targetIndex?: number) => {
    // Immediately set active index to the clicked item
    if (typeof targetIndex === 'number') {
      setActiveIndex(targetIndex);
    }

    const elementId = getUserMessageElementId(item.originalIndex);
    const element = document.getElementById(elementId);
    const scrollContainer = document.getElementById('conversation-scroll-container');

    if (element && scrollContainer) {
      const elementRect = element.getBoundingClientRect();
      const containerRect = scrollContainer.getBoundingClientRect();
      const containerCenter = containerRect.height / 2;
      const elementCenter = elementRect.height / 2;

      // Scroll so the element is centered in the viewport
      const scrollTop =
        elementRect.top -
        containerRect.top +
        scrollContainer.scrollTop -
        containerCenter +
        elementCenter;

      scrollContainer.scrollTo({
        top: Math.max(0, scrollTop),
        behavior: 'auto',
      });
    } else {
      window.dispatchEvent(
        new CustomEvent('conversation:jump-to-user-message', {
          detail: { messageIndex: item.originalIndex },
        })
      );
    }
  }, []);

  if (userMessages.length <= 1) {
    return null;
  }

  // For collapsed state: show window around active index (max 8 items)
  const getCollapsedItems = () => {
    const totalCount = userMessages.length;
    if (totalCount <= MAX_VISIBLE_ITEMS) {
      return { items: userMessages, startIndex: 0 };
    }

    let start = Math.max(0, activeIndex - Math.floor(MAX_VISIBLE_ITEMS / 2));
    let end = start + MAX_VISIBLE_ITEMS;

    if (end > totalCount) {
      end = totalCount;
      start = Math.max(0, end - MAX_VISIBLE_ITEMS);
    }

    return { items: userMessages.slice(start, end), startIndex: start };
  };

  const { items: collapsedItems, startIndex } = getCollapsedItems();

  return (
    <div
      className={cn('fixed right-3 z-40 flex flex-col items-center', className)}
      style={{
        top: '50%',
        transform: 'translateY(-50%)',
      }}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
    >
      {/* Collapsed: Show horizontal short lines - semi-transparent background */}
      <div
        className={cn(
          'flex flex-col items-center py-1.5 px-2 rounded-full backdrop-blur-sm transition-all duration-300',
          isHovered ? 'opacity-0 scale-95 pointer-events-none' : 'opacity-100 scale-100',
          'bg-background/50 border border-border/20'
        )}
        style={{ gap: '6px' }}
      >
        {collapsedItems.map((item, visibleIdx) => {
          const actualIdx = startIndex + visibleIdx;
          const isActive = actualIdx === activeIndex;

          return (
            <button
              key={item.originalIndex}
              onClick={() => scrollToMessage(item, actualIdx)}
              className={cn(
                'h-1 rounded-full transition-all duration-200',
                isActive
                  ? 'w-5 bg-primary'
                  : 'w-2.5 bg-muted-foreground/30 hover:bg-muted-foreground/50'
              )}
            />
          );
        })}
      </div>

      {/* Expanded: Show text with gradient fade - semi-transparent background */}
      <div
        className={cn(
          'absolute top-1/2 -translate-y-1/2 right-0',
          'bg-background/50 dark:bg-background/40 backdrop-blur-md',
          'rounded-lg shadow-lg border border-border/30 overflow-hidden',
          'transition-all duration-300 ease-out',
          isHovered ? 'opacity-100 scale-100' : 'opacity-0 scale-95 pointer-events-none'
        )}
        style={{ width: '180px', transformOrigin: 'right center' }}
      >
        {/* Top gradient fade */}
        {userMessages.length > MAX_VISIBLE_ITEMS && (
          <div className="absolute top-0 left-0 right-0 h-6 bg-gradient-to-b from-background/50 to-transparent z-10 pointer-events-none" />
        )}

        {/* Scrollable content - show all messages but limit visible height */}
        <div
          className="overflow-y-auto py-2 px-1.5"
          style={{ maxHeight: `${MAX_VISIBLE_ITEMS * 32}px` }} // Max 8 items visible, scrollable
        >
          <div className="space-y-0.5">
            {userMessages.map((item, idx) => {
              const isActive = idx === activeIndex;
              const preview = getPreview(item.content, 28);

              return (
                <button
                  key={item.originalIndex}
                  onClick={() => scrollToMessage(item, idx)}
                  className={cn(
                    'w-full text-left px-2.5 py-1.5 rounded-md text-xs transition-all duration-150',
                    isActive
                      ? 'bg-primary/20 text-primary font-medium'
                      : 'text-muted-foreground hover:bg-white/10 dark:hover:bg-white/5 hover:text-foreground'
                  )}
                  title={preview}
                >
                  <span className="truncate block">{preview}</span>
                </button>
              );
            })}
          </div>
        </div>

        {/* Bottom gradient fade */}
        {userMessages.length > MAX_VISIBLE_ITEMS && (
          <div className="absolute bottom-0 left-0 right-0 h-6 bg-gradient-to-t from-background/50 to-transparent z-10 pointer-events-none" />
        )}
      </div>
    </div>
  );
}

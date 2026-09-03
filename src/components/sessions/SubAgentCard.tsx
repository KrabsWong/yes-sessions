import { useEffect, useRef, useState } from 'react';
import {
  AlertCircle,
  Bot,
  ChevronDown,
  ChevronUp,
  ExternalLink,
  LoaderCircle,
  Maximize2,
  Wrench,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { cn } from '@/lib/utils';
import { sessionsApi } from '@/lib/api/sessions';
import { formatTimestamp } from './ConversationView/utils';
import { AssistantMessage, SystemMessage, UserMessage } from './ConversationView/MessageTypes';
import type { AppType } from '@/types';
import type { SessionDetail, SessionMessage } from '@/types/session';

interface InlineSessionMessagesProps {
  detail: SessionDetail;
  appType: string;
  onViewSession?: (sessionId: string, appType: string) => void;
}

function InlineSessionMessages({ detail, appType, onViewSession }: InlineSessionMessagesProps) {
  const { t } = useTranslation();
  return (
    <div className="space-y-4">
      {detail.messages.map((message, index) => {
        const key = `${message.type}-${message.timestamp}-${index}`;
        if (message.type === 'user') {
          return (
            <UserMessage
              key={key}
              content={message.content || message.redacted_content || ''}
              timestamp={message.timestamp}
              appType={appType}
              model={message.model}
            />
          );
        }
        if (message.type === 'assistant') {
          return (
            <AssistantMessage
              key={key}
              content={message.content || message.redacted_content || ''}
              reasoningContent={message.reasoning_content}
              timestamp={message.timestamp}
              appType={appType}
              model={message.model}
            />
          );
        }
        if (message.type === 'system') {
          return (
            <SystemMessage
              key={key}
              content={message.content || message.redacted_content || ''}
              timestamp={message.timestamp}
              metadata={message.metadata}
              model={message.model}
            />
          );
        }

        const rawOutput = message.tool_output?.output;
        const output =
          typeof rawOutput === 'string'
            ? rawOutput
            : rawOutput === undefined
              ? ''
              : JSON.stringify(rawOutput, null, 2);
        const childId = message.metadata?.childSessionId;
        return (
          <div key={key} className="rounded-md border border-border/70 bg-primary-muted/60 p-2.5">
            <div className="flex items-center gap-2 text-xs font-medium">
              <Wrench className="h-3.5 w-3.5 text-muted-foreground" />
              <span>{message.tool_name || 'tool'}</span>
              <span className="ml-auto text-[10px] text-muted-foreground">
                {formatTimestamp(message.timestamp)}
              </span>
            </div>
            {(message.content || message.reasoning_content) && (
              <div className="mt-2 space-y-1 text-xs text-muted-foreground">
                {message.reasoning_content && <p className="italic">{message.reasoning_content}</p>}
                {message.content && <p className="whitespace-pre-wrap">{message.content}</p>}
              </div>
            )}
            {message.type === 'tool_use' && message.tool_input && (
              <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-all text-xs text-muted-foreground">
                {JSON.stringify(message.tool_input, null, 2)}
              </pre>
            )}
            {message.type === 'tool_result' && output && (
              <pre className="mt-2 max-h-56 overflow-auto whitespace-pre-wrap break-all text-xs text-muted-foreground">
                {output}
              </pre>
            )}
            {childId && onViewSession && (
              <button
                type="button"
                onClick={() =>
                  onViewSession(childId, message.metadata?.childSessionAppType || appType)
                }
                className="mt-2 flex items-center gap-1 text-xs text-primary hover:text-primary-hover"
              >
                <ExternalLink className="h-3 w-3" />
                {t('sessions.viewSubAgentSession')}
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}

interface SubAgentCardProps {
  toolUse: SessionMessage | null;
  toolResult: SessionMessage | null;
  onViewSession?: (sessionId: string, appType: string) => void;
  className?: string;
}

const MAX_OUTPUT_LINES = 20;

export function SubAgentCard({ toolUse, toolResult, onViewSession, className }: SubAgentCardProps) {
  const { t } = useTranslation();
  const [isOutputExpanded, setIsOutputExpanded] = useState(false);
  const [isConversationExpanded, setIsConversationExpanded] = useState(false);
  const [inlineSession, setInlineSession] = useState<SessionDetail | null>(null);
  const [isLoadingInlineSession, setIsLoadingInlineSession] = useState(false);
  const [inlineSessionError, setInlineSessionError] = useState(false);
  const inlineRequestVersionRef = useRef(0);

  const toolName = toolUse?.tool_name || toolResult?.tool_name || 'Agent';
  const toolInput = toolUse?.tool_input || {};
  const childSessionId = toolResult?.metadata?.childSessionId;
  const childSessionAppType = toolResult?.metadata?.childSessionAppType || 'codebuddy';

  useEffect(() => {
    inlineRequestVersionRef.current++;
    setIsConversationExpanded(false);
    setInlineSession(null);
    setIsLoadingInlineSession(false);
    setInlineSessionError(false);
  }, [childSessionId, childSessionAppType]);

  const toggleInlineConversation = async (): Promise<void> => {
    if (isConversationExpanded) {
      setIsConversationExpanded(false);
      return;
    }
    setIsConversationExpanded(true);
    if (!childSessionId || inlineSession || isLoadingInlineSession) return;
    const requestVersion = ++inlineRequestVersionRef.current;
    setIsLoadingInlineSession(true);
    setInlineSessionError(false);
    try {
      const detail = await sessionsApi.getDetail(childSessionId, childSessionAppType as AppType);
      if (requestVersion !== inlineRequestVersionRef.current) return;
      setInlineSession(detail);
      setInlineSessionError(!detail);
    } catch {
      if (requestVersion !== inlineRequestVersionRef.current) return;
      setInlineSessionError(true);
    } finally {
      if (requestVersion === inlineRequestVersionRef.current) {
        setIsLoadingInlineSession(false);
      }
    }
  };

  const description =
    (toolInput.description as string) ||
    (toolInput.task as string) ||
    (toolInput.prompt as string) ||
    t('sessions.subAgentDefaultDesc', 'Sub-agent task');
  const subAgentType = (toolInput.subagent_type as string) || (toolInput.type as string);
  const rawModel =
    (toolInput.model as string) || (toolResult?.metadata?.model as string) || toolResult?.model;
  const subAgentModel: string | undefined =
    rawModel && rawModel !== 'default' ? rawModel : undefined;

  const hasResult = !!toolResult;
  const resultStatus = toolResult?.metadata?.subtype;
  const status = resultStatus || (hasResult ? 'completed' : 'running');

  const rawOutput = toolResult?.tool_output?.output;
  const outputContent =
    typeof rawOutput === 'string'
      ? rawOutput
      : rawOutput === undefined
        ? ''
        : JSON.stringify(rawOutput, null, 2);
  const outputLines = outputContent.split('\n');
  const totalOutputLines = outputLines.length;
  const shouldCollapseOutput = totalOutputLines > MAX_OUTPUT_LINES;
  const displayOutput =
    shouldCollapseOutput && !isOutputExpanded
      ? outputLines.slice(0, MAX_OUTPUT_LINES).join('\n') + '\n\n...'
      : outputContent;

  return (
    <div
      className={cn(
        'border rounded-lg overflow-hidden bg-gradient-to-br from-purple-50/50 to-blue-50/50 dark:from-purple-900/20 dark:to-blue-900/20',
        className
      )}
    >
      <div className="flex items-center gap-2 px-3 py-2 bg-purple-100/50 dark:bg-purple-900/30 border-b border-purple-200 dark:border-purple-800">
        <Bot className="h-4 w-4 text-purple-600 dark:text-purple-400" />
        <span className="font-medium text-sm text-purple-700 dark:text-purple-300">
          {t('sessions.subAgent', 'Sub Agent')}
        </span>
        {subAgentType && (
          <span className="text-xs px-2 py-0.5 rounded-full bg-purple-200 dark:bg-purple-800 text-purple-700 dark:text-purple-300">
            {subAgentType}
          </span>
        )}
        {subAgentModel && (
          <span
            className="text-xs px-2 py-0.5 rounded-full bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 ml-1"
            title={t('sessions.model')}
          >
            {subAgentModel}
          </span>
        )}
        <span className="text-xs text-muted-foreground ml-auto">
          {formatTimestamp(toolUse?.timestamp || toolResult?.timestamp || '')}
        </span>
        <span
          className={cn(
            'text-xs px-1.5 py-0.5 rounded-full',
            status === 'completed'
              ? 'bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-400'
              : status === 'running'
                ? 'bg-amber-100 text-amber-700 dark:bg-amber-900/30 dark:text-amber-400'
                : 'bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-400'
          )}
        >
          {t(`sessions.${status}`, status)}
        </span>
      </div>

      <div className="p-3 space-y-3">
        <div className="text-sm text-foreground">
          <span className="text-xs text-muted-foreground uppercase tracking-wider block mb-1">
            {t('sessions.task', 'Task')}
          </span>
          <p className="line-clamp-3">{description}</p>
        </div>

        {hasResult && outputContent && (
          <div className="border-t border-purple-200/50 dark:border-purple-800/50 pt-3">
            <span className="text-xs text-muted-foreground uppercase tracking-wider block mb-2">
              {t('sessions.output', 'Output')}
            </span>
            <pre className="text-xs font-mono bg-primary-muted rounded p-2 whitespace-pre-wrap break-all">
              {displayOutput}
            </pre>
            {shouldCollapseOutput && (
              <button
                onClick={() => setIsOutputExpanded(!isOutputExpanded)}
                className="flex items-center gap-1.5 mt-2 text-xs text-primary hover:text-primary-hover transition-colors"
              >
                {isOutputExpanded ? (
                  <>
                    <ChevronUp className="h-3.5 w-3.5" />
                    {t('sessions.collapse', 'Collapse')}
                  </>
                ) : (
                  <>
                    <Maximize2 className="h-3.5 w-3.5" />
                    {t('sessions.expandAll')} ({totalOutputLines} {t('sessions.lines', 'lines')})
                  </>
                )}
              </button>
            )}
          </div>
        )}

        {toolName !== 'Agent' && (
          <div className="text-xs text-muted-foreground">
            {t('sessions.agentType', 'Agent')}: {toolName}
          </div>
        )}

        {childSessionId && (
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
            <button
              type="button"
              onClick={() => void toggleInlineConversation()}
              className="flex items-center justify-center gap-1.5 rounded-md bg-purple-100 px-3 py-1.5 text-xs font-medium text-purple-700 transition-colors hover:bg-purple-200 dark:bg-purple-900/30 dark:text-purple-300 dark:hover:bg-purple-900/50"
            >
              {isConversationExpanded ? (
                <ChevronUp className="h-3.5 w-3.5" />
              ) : (
                <ChevronDown className="h-3.5 w-3.5" />
              )}
              {isConversationExpanded
                ? t('sessions.collapseSubAgentSession')
                : t('sessions.expandSubAgentSession')}
            </button>
            {onViewSession && (
              <button
                type="button"
                onClick={() => onViewSession(childSessionId, childSessionAppType)}
                className="flex items-center justify-center gap-1.5 rounded-md bg-primary-muted px-3 py-1.5 text-xs font-medium text-primary transition-colors hover:bg-primary-light"
              >
                <ExternalLink className="h-3.5 w-3.5" />
                {t('sessions.viewSubAgentSession', 'View Sub-Agent Session')}
              </button>
            )}
          </div>
        )}

        {isConversationExpanded && (
          <div className="border-t border-purple-200/60 pt-3 dark:border-purple-800/60">
            {isLoadingInlineSession ? (
              <div className="flex items-center justify-center gap-2 py-5 text-xs text-muted-foreground">
                <LoaderCircle className="h-4 w-4 animate-spin" />
                {t('sessions.loadingConversation')}
              </div>
            ) : inlineSessionError || !inlineSession ? (
              <div className="flex items-center justify-center gap-2 py-5 text-xs text-destructive">
                <AlertCircle className="h-4 w-4" />
                {t('sessions.error')}
              </div>
            ) : (
              <div className="rounded-md bg-background/70 p-3">
                <InlineSessionMessages
                  detail={inlineSession}
                  appType={childSessionAppType}
                  onViewSession={onViewSession}
                />
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

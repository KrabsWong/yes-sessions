/**
 * CodeBuddy session service.
 *
 * CodeBuddy stores primary transcripts at projects/<project>/<session>.jsonl and
 * sub-agent transcripts below projects/<project>/<session>/subagents/*.jsonl.
 * The JSONL schema is append-only and has changed over time, so normalization in
 * this file deliberately accepts unknown fields and validates values at runtime.
 */

import fs from 'fs';
import os from 'os';
import path from 'path';
import log from 'electron-log';
import type { Session, SessionDetail, SessionMessage } from '@/types/session';

const CODEBUDDY_DIR = path.join(os.homedir(), '.codebuddy');
const PROJECTS_DIR = path.join(CODEBUDDY_DIR, 'projects');
const SESSIONS_DIR = path.join(CODEBUDDY_DIR, 'sessions');
const MAX_EMBEDDED_IMAGE_BYTES = 10 * 1024 * 1024;
const SUMMARY_READ_CONCURRENCY = 4;

interface CodebuddySessionFile {
  sessionId?: string;
  lastHeartbeat?: number;
  updatedAt?: number;
  meta?: { currentTopic?: string };
}

interface CodebuddyContentItem {
  type?: string;
  text?: unknown;
  blob_path?: unknown;
  mime?: unknown;
}

interface CodebuddyMessageEntry {
  id?: string;
  timestamp?: unknown;
  type?: string;
  role?: string;
  cwd?: unknown;
  sessionId?: unknown;
  content?: unknown;
  rawContent?: unknown;
  message?: { content?: unknown };
  name?: unknown;
  arguments?: unknown;
  callId?: unknown;
  output?: unknown;
  status?: unknown;
  providerData?: {
    model?: unknown;
    agent?: unknown;
    isSubAgent?: unknown;
    toolResult?: unknown;
  };
}

interface SessionFileRef {
  path: string;
  projectCwd: string;
  id: string;
  internalSessionId?: string;
  parentSessionId?: string;
  agentType?: string;
  kind: 'main' | 'subagent';
  size: number;
  createdAt: number;
  updatedAt: number;
}

interface SessionSummary {
  count: number;
  firstMessage: string;
  lastMessage: string;
  createdAt?: number;
  updatedAt?: number;
  hasRecord: boolean;
}

interface SummaryState extends SessionSummary {
  pendingReasoning: boolean;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function parseEntry(line: string): CodebuddyMessageEntry | null {
  try {
    const value = JSON.parse(line) as unknown;
    return isRecord(value) ? (value as CodebuddyMessageEntry) : null;
  } catch {
    return null;
  }
}

function toTime(value: unknown): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'string') {
    const parsed = new Date(value).getTime();
    return Number.isFinite(parsed) ? parsed : undefined;
  }
  return undefined;
}

function toIsoTime(value: unknown, fallback: number): string {
  return new Date(toTime(value) ?? fallback).toISOString();
}

function getContentItems(value: unknown): CodebuddyContentItem[] {
  if (!Array.isArray(value)) return [];
  return value.filter(isRecord) as CodebuddyContentItem[];
}

function extractText(value: unknown, acceptedTypes?: Set<string>): string {
  if (typeof value === 'string') return value;
  return getContentItems(value)
    .filter((item) => !acceptedTypes || (item.type ? acceptedTypes.has(item.type) : true))
    .map((item) => (typeof item.text === 'string' ? item.text : ''))
    .filter(Boolean)
    .join('\n');
}

function extractReasoningText(entry: CodebuddyMessageEntry): string {
  return (
    extractText(entry.rawContent, new Set(['reasoning_text', 'text', 'input_text'])) ||
    extractText(entry.content, new Set(['reasoning_text', 'text', 'input_text']))
  );
}

function extractMessageText(entry: CodebuddyMessageEntry): string {
  const acceptedTypes =
    entry.role === 'assistant'
      ? new Set(['output_text', 'text'])
      : new Set(['input_text', 'text', 'output_text']);
  return extractText(entry.content, acceptedTypes) || extractText(entry.message?.content);
}

const CODEBUDDY_CONTROL_BLOCKS = [
  /<system-reminder\b(?=[^>]*\bdata-role\s*=\s*(?:"command-caveat"|'command-caveat'|command-caveat(?=\s|\/?>)))[^>]*>[\s\S]*?<\/system-reminder\s*>/gi,
  /<system-reminder>[\s\S]*?<\/system-reminder\s*>/gi,
  /<local-command-stdout\b[^>]*>[\s\S]*?<\/local-command-stdout\s*>/gi,
  /<local-command-stderr\b[^>]*>[\s\S]*?<\/local-command-stderr\s*>/gi,
  /<command-name\b[^>]*>[\s\S]*?<\/command-name\s*>/gi,
];

function cleanCodebuddyUserText(text: string): string {
  let cleaned = text;
  for (const pattern of CODEBUDDY_CONTROL_BLOCKS) {
    cleaned = cleaned.replace(pattern, '');
  }
  return cleaned.replace(/\n{3,}/g, '\n\n').trim();
}

function parseToolInput(value: unknown): Record<string, unknown> {
  let parsed = value;
  if (typeof value === 'string') {
    try {
      parsed = JSON.parse(value) as unknown;
    } catch {
      return { arguments: value };
    }
  }
  if (isRecord(parsed)) return parsed;
  if (parsed === undefined || parsed === null || parsed === '') return {};
  return { value: parsed };
}

function stringifyOutput(value: unknown): string {
  if (typeof value === 'string') return value;
  if (isRecord(value) && typeof value.text === 'string') return value.text;
  if (isRecord(value) && Array.isArray(value.content)) {
    const content = extractText(value.content);
    if (content) return content;
  }
  if (value === undefined) return '';
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function getString(value: unknown): string | undefined {
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

function extractChildSessionId(value: unknown): string | undefined {
  if (isRecord(value)) {
    for (const key of ['childSessionId', 'subAgentSessionId', 'sessionId', 'agentId']) {
      const candidate = getString(value[key]);
      if (candidate) return candidate;
    }
  }
  const text = stringifyOutput(value);
  if (!text) return undefined;
  try {
    const parsed = JSON.parse(text) as unknown;
    const nested = extractChildSessionId(parsed);
    if (nested) return nested;
  } catch {
    // Plain-text Agent results are the common CodeBuddy representation.
  }
  const agentIds = text.match(/\bagent-[a-z0-9_-]+\b/gi);
  if (agentIds?.length) return agentIds[agentIds.length - 1];
  return text.match(
    /(?:childSessionId|subAgentSessionId|sessionId|session)["']?\s*[:=]\s*["']?([a-f0-9-]{36})/i
  )?.[1];
}

function extractStructuredChildSessionId(entry: CodebuddyMessageEntry): string | undefined {
  const toolResult = entry.providerData?.toolResult;
  if (!isRecord(toolResult)) return undefined;
  const subAgent = toolResult.subAgent;
  if (!isRecord(subAgent)) return undefined;
  return getString(subAgent.sessionId);
}

function createSummaryState(): SummaryState {
  return {
    count: 0,
    firstMessage: '',
    lastMessage: '',
    hasRecord: false,
    pendingReasoning: false,
  };
}

function flushSummaryReasoning(state: SummaryState): void {
  if (state.pendingReasoning) {
    state.count++;
    state.pendingReasoning = false;
  }
}

function updateSummary(state: SummaryState, entry: CodebuddyMessageEntry): void {
  state.hasRecord = true;
  const timestamp = toTime(entry.timestamp);
  if (timestamp !== undefined) {
    state.createdAt ??= timestamp;
    state.updatedAt = timestamp;
  }
  if (entry.type === 'reasoning') {
    if (extractReasoningText(entry)) state.pendingReasoning = true;
    return;
  }
  if (entry.type === 'message' && entry.role === 'user') {
    const text = cleanCodebuddyUserText(extractMessageText(entry));
    const hasImage = getContentItems(entry.content).some((item) => item.type === 'image_blob_ref');
    if (!text && !hasImage) return;
    flushSummaryReasoning(state);
    state.count++;
    if (text) {
      state.firstMessage ||= text.slice(0, 100);
      state.lastMessage = text.slice(0, 100);
    }
    return;
  }
  if (
    (entry.type === 'message' && (entry.role === 'assistant' || entry.role === 'system')) ||
    entry.type === 'assistant'
  ) {
    if (extractMessageText(entry) || state.pendingReasoning) state.count++;
    state.pendingReasoning = false;
    return;
  }
  if (entry.type === 'function_call') {
    state.count++;
    state.pendingReasoning = false;
    return;
  }
  if (entry.type === 'function_call_result') {
    flushSummaryReasoning(state);
    state.count++;
  }
}

export function normalizeCodebuddyEntries(
  entries: CodebuddyMessageEntry[],
  fallbackTimestamp: number,
  resolveImage: (item: CodebuddyContentItem) => string | null = () => null
): SessionMessage[] {
  const messages: SessionMessage[] = [];
  const pendingAgentCalls = new Map<string, Record<string, unknown>>();
  let currentModel: string | undefined;
  let pendingReasoning = '';

  const pushPendingReasoning = (timestamp: unknown): void => {
    if (!pendingReasoning) return;
    messages.push({
      type: 'assistant',
      timestamp: toIsoTime(timestamp, fallbackTimestamp),
      reasoning_content: pendingReasoning,
      model: currentModel,
    });
    pendingReasoning = '';
  };

  for (const entry of entries) {
    currentModel = getString(entry.providerData?.model) || currentModel;
    const timestamp = toIsoTime(entry.timestamp, fallbackTimestamp);

    if (entry.type === 'reasoning') {
      const reasoning = extractReasoningText(entry);
      if (reasoning) pendingReasoning = [pendingReasoning, reasoning].filter(Boolean).join('\n\n');
      continue;
    }

    if (entry.type === 'message' && (entry.role === 'user' || entry.role === 'system')) {
      let text = extractMessageText(entry);
      let images: string[] = [];
      if (entry.role === 'user') {
        text = cleanCodebuddyUserText(text);
        images = getContentItems(entry.content)
          .filter((item) => item.type === 'image_blob_ref')
          .map((item) => {
            const dataUrl = resolveImage(item);
            const imagePath = getString(item.blob_path);
            const fileName = imagePath ? path.basename(imagePath) : '';
            if (!fileName) return '';
            return dataUrl ? `![${fileName}](${dataUrl})` : `📎 ${fileName}`;
          });
        if (!text && images.every((image) => !image)) continue;
      }
      pushPendingReasoning(entry.timestamp);
      text = [text, ...images].filter(Boolean).join('\n\n');
      if (text) {
        const status = getString(entry.status);
        messages.push({
          type: entry.role === 'system' ? 'system' : 'user',
          timestamp,
          content: text,
          metadata: status ? { subtype: status } : undefined,
          model: currentModel,
        });
      }
      continue;
    }

    if ((entry.type === 'message' && entry.role === 'assistant') || entry.type === 'assistant') {
      const text = extractMessageText(entry);
      if (text || pendingReasoning) {
        const status = getString(entry.status);
        messages.push({
          type: 'assistant',
          timestamp,
          content: text || undefined,
          reasoning_content: pendingReasoning || undefined,
          metadata: status ? { subtype: status } : undefined,
          model: currentModel,
        });
      }
      pendingReasoning = '';
      continue;
    }

    if (entry.type === 'function_call') {
      const toolName = getString(entry.name) || 'tool';
      const callId = getString(entry.callId) || getString(entry.id) || `call_${messages.length}`;
      const toolInput = parseToolInput(entry.arguments);
      messages.push({
        type: 'tool_use',
        timestamp,
        tool_name: toolName,
        tool_input: toolInput,
        callId,
        reasoning_content: pendingReasoning || undefined,
        model: currentModel,
      });
      pendingReasoning = '';
      if (toolName.toLowerCase().includes('agent')) pendingAgentCalls.set(callId, toolInput);
      continue;
    }

    if (entry.type === 'function_call_result') {
      pushPendingReasoning(entry.timestamp);
      const toolName = getString(entry.name) || 'tool';
      const callId = getString(entry.callId);
      const outputText = stringifyOutput(entry.output);
      const pendingInput = callId ? pendingAgentCalls.get(callId) : undefined;
      const isAgentResult = toolName.toLowerCase().includes('agent') || Boolean(pendingInput);
      const childSessionId = isAgentResult
        ? extractStructuredChildSessionId(entry) ||
          extractChildSessionId(entry.output) ||
          extractChildSessionId(pendingInput)
        : undefined;
      if (callId) pendingAgentCalls.delete(callId);
      const status = getString(entry.status);
      messages.push({
        type: 'tool_result',
        timestamp,
        tool_name: toolName,
        content: outputText.slice(0, 300) + (outputText.length > 300 ? '...' : ''),
        tool_output: { output: outputText },
        callId,
        metadata: {
          ...(status ? { subtype: status } : {}),
          ...(childSessionId
            ? { childSessionId, childSessionAppType: 'codebuddy', model: currentModel }
            : {}),
        },
        model: currentModel,
      });
    }
  }

  pushPendingReasoning(fallbackTimestamp);
  return messages;
}

class CodebuddySessionService {
  private sessionFileById = new Map<string, SessionFileRef>();
  private identityCache = new Map<
    string,
    {
      size: number;
      mtimeMs: number;
      identity: { cwd?: string; sessionId?: string; agentType?: string };
    }
  >();
  private summaryCache = new Map<
    string,
    { size: number; mtimeMs: number; summary: SessionSummary }
  >();
  private detailCache:
    | { path: string; size: number; mtimeMs: number; detail: SessionDetail }
    | undefined;

  isAvailable(): boolean {
    try {
      return fs.existsSync(PROJECTS_DIR);
    } catch {
      return false;
    }
  }

  async getAllSessions(): Promise<Session[]> {
    try {
      if (!fs.existsSync(PROJECTS_DIR)) return [];
      const activeSessions = this.readActiveSessions();
      const refs = this.discoverSessionFiles();
      this.indexSessionFiles(refs);
      const sessionMap = new Map<string, Session & { contentSize: number }>();
      const refsToSummarize = refs.filter((ref) => ref.size > 0);
      const summarized = new Array<{ ref: SessionFileRef; summary: SessionSummary }>(
        refsToSummarize.length
      );
      let nextRefIndex = 0;
      await Promise.all(
        Array.from(
          { length: Math.min(SUMMARY_READ_CONCURRENCY, refsToSummarize.length) },
          async () => {
            while (nextRefIndex < refsToSummarize.length) {
              const index = nextRefIndex++;
              const ref = refsToSummarize[index];
              summarized[index] = { ref, summary: await this.streamSessionSummary(ref) };
            }
          }
        )
      );

      for (const { ref, summary } of summarized) {
        if (!summary.hasRecord || summary.count === 0) continue;
        const active =
          activeSessions.get(ref.id) ||
          (ref.internalSessionId ? activeSessions.get(ref.internalSessionId) : undefined);
        const session: Session & { contentSize: number } = {
          id: ref.id,
          appType: 'codebuddy',
          fileName: active?.meta?.currentTopic || summary.firstMessage || ref.id,
          filePath: ref.path,
          directory: ref.projectCwd,
          createdAt: summary.createdAt ?? ref.createdAt,
          updatedAt:
            toTime(active?.updatedAt) ??
            toTime(active?.lastHeartbeat) ??
            summary.updatedAt ??
            ref.updatedAt,
          messageCount: summary.count,
          firstMessage: summary.firstMessage,
          lastMessage: active?.meta?.currentTopic || summary.lastMessage,
          uuid: ref.internalSessionId || ref.id,
          kind: ref.kind,
          parentSessionId: ref.parentSessionId,
          agentType: ref.agentType,
          contentSize: ref.size,
        };
        const existing = sessionMap.get(session.id);
        if (!existing || existing.contentSize < session.contentSize)
          sessionMap.set(session.id, session);
      }

      return Array.from(sessionMap.values())
        .sort((a, b) => b.updatedAt - a.updatedAt)
        .map(({ contentSize: _contentSize, ...session }) => session);
    } catch (error) {
      log.error('Failed to get CodeBuddy sessions:', error);
      return [];
    }
  }

  async getSessionDetail(sessionId: string): Promise<SessionDetail | null> {
    try {
      if (!fs.existsSync(PROJECTS_DIR)) return null;
      let ref = this.sessionFileById.get(sessionId);
      if (!ref || !fs.existsSync(ref.path)) {
        const refs = this.discoverSessionFiles();
        this.indexSessionFiles(refs);
        ref = this.sessionFileById.get(sessionId);
      }
      if (!ref) return null;

      const stats = await fs.promises.stat(ref.path);
      if (stats.size === 0) return null;
      if (
        this.detailCache?.path === ref.path &&
        this.detailCache.size === stats.size &&
        this.detailCache.mtimeMs === stats.mtimeMs
      ) {
        return this.detailCache.detail;
      }

      const content = await fs.promises.readFile(ref.path, 'utf-8');
      const lines = content.split('\n').filter((line) => line.trim());
      const entries = lines
        .map(parseEntry)
        .filter((entry): entry is CodebuddyMessageEntry => !!entry);
      if (entries.length === 0) return null;
      const messages = normalizeCodebuddyEntries(entries, ref.updatedAt, (item) =>
        this.readImageDataUrl(item)
      );
      if (messages.length === 0) return null;

      const firstMessage = this.findUserPreview(messages, false);
      const lastMessage = this.findUserPreview(messages, true) || firstMessage;
      const detail: SessionDetail = {
        id: ref.id,
        appType: 'codebuddy',
        fileName: firstMessage || ref.id,
        filePath: ref.path,
        directory: ref.projectCwd,
        createdAt: toTime(entries[0]?.timestamp) ?? ref.createdAt,
        updatedAt: toTime(entries[entries.length - 1]?.timestamp) ?? ref.updatedAt,
        messageCount: messages.length,
        firstMessage,
        lastMessage,
        uuid: ref.internalSessionId || ref.id,
        kind: ref.kind,
        parentSessionId: ref.parentSessionId,
        agentType: ref.agentType,
        messages,
      };
      this.detailCache = { path: ref.path, size: stats.size, mtimeMs: stats.mtimeMs, detail };
      return detail;
    } catch (error) {
      log.error(`Failed to get CodeBuddy session detail ${sessionId}:`, error);
      throw error;
    }
  }

  async getStats(): Promise<{
    totalSessions: number;
    totalMessages: number;
    firstSessionDate?: number;
    lastSessionDate?: number;
  }> {
    const sessions = await this.getAllSessions();
    if (sessions.length === 0) return { totalSessions: 0, totalMessages: 0 };
    return {
      totalSessions: sessions.length,
      totalMessages: sessions.reduce((sum, session) => sum + session.messageCount, 0),
      firstSessionDate: Math.min(...sessions.map((session) => session.createdAt)),
      lastSessionDate: Math.max(...sessions.map((session) => session.updatedAt)),
    };
  }

  private discoverSessionFiles(): SessionFileRef[] {
    const refs: SessionFileRef[] = [];
    for (const projectEntry of fs.readdirSync(PROJECTS_DIR, { withFileTypes: true })) {
      if (!projectEntry.isDirectory()) continue;
      const projectPath = path.join(PROJECTS_DIR, projectEntry.name);
      const decodedCwd = this.decodeProjectDir(projectEntry.name);
      this.walkJsonl(projectPath, (filePath) => {
        try {
          const stats = fs.statSync(filePath);
          const identity = this.readFileIdentity(filePath, stats.size, stats.mtimeMs);
          const relativeParts = path.relative(projectPath, filePath).split(path.sep);
          const subagentsIndex = relativeParts.lastIndexOf('subagents');
          const kind = subagentsIndex >= 0 ? 'subagent' : 'main';
          const fileId = path.basename(filePath, '.jsonl');
          refs.push({
            path: filePath,
            projectCwd: identity.cwd || decodedCwd,
            id: fileId,
            internalSessionId: identity.sessionId,
            parentSessionId:
              kind === 'subagent' && subagentsIndex > 0
                ? relativeParts[subagentsIndex - 1]
                : undefined,
            agentType: identity.agentType,
            kind,
            size: stats.size,
            createdAt: stats.birthtimeMs || stats.mtimeMs,
            updatedAt: stats.mtimeMs,
          });
        } catch (error) {
          log.warn(`Failed to inspect CodeBuddy session file ${filePath}:`, error);
        }
      });
    }
    const canonicalIdByAlias = new Map<string, string>();
    for (const ref of refs) canonicalIdByAlias.set(ref.id, ref.id);
    for (const ref of refs) {
      if (ref.internalSessionId && !canonicalIdByAlias.has(ref.internalSessionId)) {
        canonicalIdByAlias.set(ref.internalSessionId, ref.id);
      }
    }
    for (const ref of refs) {
      if (ref.parentSessionId) {
        ref.parentSessionId = canonicalIdByAlias.get(ref.parentSessionId) || ref.parentSessionId;
      }
    }
    return refs;
  }

  private walkJsonl(dirPath: string, visit: (filePath: string) => void): void {
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(dirPath, { withFileTypes: true });
    } catch (error) {
      log.warn(`Failed to read CodeBuddy directory ${dirPath}:`, error);
      return;
    }
    for (const entry of entries) {
      const entryPath = path.join(dirPath, entry.name);
      if (entry.isDirectory()) this.walkJsonl(entryPath, visit);
      else if (entry.isFile() && entry.name.endsWith('.jsonl')) visit(entryPath);
    }
  }

  private readFileIdentity(
    filePath: string,
    size: number,
    mtimeMs: number
  ): { cwd?: string; sessionId?: string; agentType?: string } {
    const cached = this.identityCache.get(filePath);
    if (cached?.size === size && cached.mtimeMs === mtimeMs) return cached.identity;

    const fd = fs.openSync(filePath, 'r');
    const identity: { cwd?: string; sessionId?: string; agentType?: string } = {};
    try {
      const buffer = Buffer.alloc(64 * 1024);
      const bytesRead = fs.readSync(fd, buffer, 0, buffer.length, 0);
      for (const line of buffer.toString('utf-8', 0, bytesRead).split('\n')) {
        if (!line.trim()) continue;
        const entry = parseEntry(line);
        if (!entry) continue;
        identity.cwd ||= getString(entry.cwd);
        identity.sessionId ||= getString(entry.sessionId);
        if (entry.providerData?.isSubAgent === true) {
          identity.agentType ||= getString(entry.providerData.agent);
        }
        if (identity.cwd && identity.sessionId && identity.agentType) break;
      }
    } finally {
      fs.closeSync(fd);
    }
    this.identityCache.set(filePath, { size, mtimeMs, identity });
    return identity;
  }

  private indexSessionFiles(refs: SessionFileRef[]): void {
    this.sessionFileById.clear();
    const currentPaths = new Set(refs.map((ref) => ref.path));
    for (const filePath of this.identityCache.keys()) {
      if (!currentPaths.has(filePath)) this.identityCache.delete(filePath);
    }
    for (const filePath of this.summaryCache.keys()) {
      if (!currentPaths.has(filePath)) this.summaryCache.delete(filePath);
    }
    for (const ref of refs.sort((a, b) => b.size - a.size)) {
      if (!this.sessionFileById.has(ref.id)) this.sessionFileById.set(ref.id, ref);
      if (ref.internalSessionId && !this.sessionFileById.has(ref.internalSessionId)) {
        this.sessionFileById.set(ref.internalSessionId, ref);
      }
    }
  }

  private streamSessionSummary(ref: SessionFileRef): Promise<SessionSummary> {
    const cached = this.summaryCache.get(ref.path);
    if (cached?.size === ref.size && cached.mtimeMs === ref.updatedAt) {
      return Promise.resolve(cached.summary);
    }

    return new Promise((resolve) => {
      const state = createSummaryState();
      const stream = fs.createReadStream(ref.path, {
        encoding: 'utf-8',
        highWaterMark: 64 * 1024,
      });
      let leftover = '';
      const processLine = (line: string): void => {
        if (!line.trim()) return;
        const entry = parseEntry(line);
        if (entry) updateSummary(state, entry);
      };
      stream.on('data', (chunk: string | Buffer) => {
        const lines = (leftover + chunk.toString()).split('\n');
        leftover = lines.pop() || '';
        lines.forEach(processLine);
      });
      stream.on('end', () => {
        processLine(leftover);
        flushSummaryReasoning(state);
        const { pendingReasoning: _pendingReasoning, ...summary } = state;
        this.summaryCache.set(ref.path, {
          size: ref.size,
          mtimeMs: ref.updatedAt,
          summary,
        });
        resolve(summary);
      });
      stream.on('error', () =>
        resolve({ count: 0, firstMessage: '', lastMessage: '', hasRecord: false })
      );
    });
  }

  private readActiveSessions(): Map<string, CodebuddySessionFile> {
    const sessions = new Map<string, CodebuddySessionFile>();
    if (!fs.existsSync(SESSIONS_DIR)) return sessions;
    for (const entry of fs.readdirSync(SESSIONS_DIR, { withFileTypes: true })) {
      if (!entry.isFile() || !entry.name.endsWith('.json')) continue;
      try {
        const value = JSON.parse(
          fs.readFileSync(path.join(SESSIONS_DIR, entry.name), 'utf-8')
        ) as unknown;
        if (!isRecord(value)) continue;
        const session = value as CodebuddySessionFile;
        if (session.sessionId) sessions.set(session.sessionId, session);
      } catch (error) {
        log.warn(`Failed to parse CodeBuddy active session ${entry.name}:`, error);
      }
    }
    return sessions;
  }

  private readImageDataUrl(item: CodebuddyContentItem): string | null {
    const imagePath = getString(item.blob_path);
    if (!imagePath) return null;
    const mimeType = getString(item.mime) || this.getImageMimeType(path.extname(imagePath));
    if (!mimeType || !mimeType.startsWith('image/')) return null;
    try {
      const resolvedPath = fs.realpathSync(imagePath);
      const resolvedRoot = fs.realpathSync(CODEBUDDY_DIR);
      if (resolvedPath !== resolvedRoot && !resolvedPath.startsWith(resolvedRoot + path.sep)) {
        return null;
      }
      const stats = fs.statSync(resolvedPath);
      if (!stats.isFile() || stats.size > MAX_EMBEDDED_IMAGE_BYTES) return null;
      return `data:${mimeType};base64,${fs.readFileSync(resolvedPath).toString('base64')}`;
    } catch {
      return null;
    }
  }

  private getImageMimeType(extension: string): string | null {
    const mimeTypes: Record<string, string> = {
      '.png': 'image/png',
      '.jpg': 'image/jpeg',
      '.jpeg': 'image/jpeg',
      '.gif': 'image/gif',
      '.webp': 'image/webp',
      '.svg': 'image/svg+xml',
      '.bmp': 'image/bmp',
      '.ico': 'image/x-icon',
      '.avif': 'image/avif',
    };
    return mimeTypes[extension.toLowerCase()] || null;
  }

  private findUserPreview(messages: SessionMessage[], reverse: boolean): string {
    const source = reverse ? [...messages].reverse() : messages;
    const content =
      source.find((message) => message.type === 'user' && message.content)?.content || '';
    return content
      .replace(/!\[[^\]]*]\(data:image\/[^)]+\)/g, '')
      .trim()
      .slice(0, 100);
  }

  private decodeProjectDir(dirName: string): string {
    if (dirName.startsWith('Users-')) {
      const parts = dirName.split('-');
      if (parts.length >= 2) return `/Users/${parts[1]}/${parts.slice(2).join('/')}`;
    }
    return '/' + dirName.replace(/-/g, '/');
  }
}

export const codebuddySessionService = new CodebuddySessionService();

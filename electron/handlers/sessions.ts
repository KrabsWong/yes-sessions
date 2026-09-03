/**
 * Sessions IPC Handlers
 *
 * Handles all session-related IPC communication
 */

import { ipcRegistry } from '../ipc/registry';
import { sessionServiceRegistry } from '../services/session/registry';
import { claudeSessionService } from '../services/session/claude';
import { opencodeSessionService } from '../services/session/opencode';
import { codebuddySessionService } from '../services/session/codebuddy';
import { codexSessionService } from '../services/session/codex';
import { resumeSessionInTerminal, getTerminalInfo } from '../services/terminal/launcher';
import { APP_SESSION_SUPPORT } from '../../src/config/apps';
import type { AppSupportSummary, AppType, SessionStatsSummary } from '../../src/types';
import log from 'electron-log';
import {
  appSupportResult,
  appTypeArg,
  optionalStringArg,
  sessionDetailResult,
  sessionStatsResult,
  sessionsResult,
  stringArg,
  terminalInfoResult,
  validateArgs,
} from '../ipc/validation';

// 注册所有 session 服务到 registry
function registerSessionServices(): void {
  // Claude
  sessionServiceRegistry.register({
    appType: 'claude',
    getAllSessions: () => claudeSessionService.getAllSessions(),
    getSessionDetail: (sessionId: string) => claudeSessionService.getSessionDetail(sessionId),
    getStats: () => claudeSessionService.getStats(),
    isAvailable: () => claudeSessionService.isAvailable(),
  });

  // OpenCode
  sessionServiceRegistry.register({
    appType: 'opencode',
    getAllSessions: () => opencodeSessionService.getAllSessions(),
    getSessionDetail: (sessionId: string) => opencodeSessionService.getSessionDetail(sessionId),
    getStats: () => opencodeSessionService.getStats(),
    isAvailable: () => opencodeSessionService.isAvailable(),
  });

  // CodeBuddy
  sessionServiceRegistry.register({
    appType: 'codebuddy',
    getAllSessions: () => codebuddySessionService.getAllSessions(),
    getSessionDetail: (sessionId: string) => codebuddySessionService.getSessionDetail(sessionId),
    getStats: () => codebuddySessionService.getStats(),
    isAvailable: () => codebuddySessionService.isAvailable(),
  });

  // Codex
  sessionServiceRegistry.register({
    appType: 'codex',
    getAllSessions: () => codexSessionService.getAllSessions(),
    getSessionDetail: (sessionId: string) => codexSessionService.getSessionDetail(sessionId),
    getStats: () => codexSessionService.getStats(),
    isAvailable: () => codexSessionService.isAvailable(),
  });

  log.info('Session services registered:', sessionServiceRegistry.getSupportedAppTypes());
}

/**
 * Initialize Sessions IPC handlers
 */
export function registerSessionsHandlers(): void {
  // 先注册所有服务
  registerSessionServices();

  // Get sessions for an app - 使用 Registry
  ipcRegistry.register(
    'sessions:getAll',
    async (_event, ...args: unknown[]) => {
      const [appType] = args as [AppType];

      try {
        const service = sessionServiceRegistry.get(appType);
        if (service) {
          return await service.getAllSessions();
        }
        // Return empty array for unsupported apps
        return [];
      } catch (error) {
        log.error('Failed to get sessions:', error);
        throw error;
      }
    },
    {
      validateArgs: validateArgs(appTypeArg('appType')),
      validateResult: sessionsResult(),
    }
  );

  // Get session detail from the explicitly selected provider.
  ipcRegistry.register(
    'sessions:getDetail',
    async (_event, ...args: unknown[]) => {
      const [sessionId, appType] = args as [string, AppType];

      try {
        const service = sessionServiceRegistry.get(appType);
        return service ? await service.getSessionDetail(sessionId) : null;
      } catch (error) {
        log.error('Failed to get session detail:', error);
        throw error;
      }
    },
    {
      validateArgs: validateArgs(stringArg('sessionId'), appTypeArg('appType')),
      validateResult: sessionDetailResult(),
    }
  );

  // Get session stats - 使用 Registry
  ipcRegistry.register(
    'sessions:getStats',
    async (_event, ...args: unknown[]) => {
      const [appType] = args as [AppType];

      try {
        const service = sessionServiceRegistry.get(appType);
        if (service) {
          return await service.getStats();
        }
        return { totalSessions: 0, totalMessages: 0 } satisfies SessionStatsSummary;
      } catch (error) {
        log.error('Failed to get session stats:', error);
        throw error;
      }
    },
    {
      validateArgs: validateArgs(appTypeArg('appType')),
      validateResult: sessionStatsResult(),
    }
  );

  // Check if app is supported - 使用 Registry + 静态配置
  ipcRegistry.register(
    'sessions:getSupportStatus',
    async (_event, ...args: unknown[]) => {
      const [appType] = args as [AppType];

      // 使用 Registry 检查是否支持
      const service = sessionServiceRegistry.get(appType);

      const config = APP_SESSION_SUPPORT[appType];
      if (!config) {
        return {
          supported: false,
          status: 'not_supported',
          isAvailable: false,
          notAvailableReason: 'not_supported',
        } satisfies AppSupportSummary;
      }

      return {
        supported: config.supported,
        status: config.status,
        isAvailable: service?.isAvailable() ?? false,
      } satisfies AppSupportSummary;
    },
    {
      validateArgs: validateArgs(appTypeArg('appType')),
      validateResult: appSupportResult(),
    }
  );

  // Resume session in terminal - 直接调用，不涉及服务选择
  ipcRegistry.register(
    'sessions:resume',
    async (_event, ...args: unknown[]) => {
      const [sessionId, appType, workingDir] = args as [string, AppType, string | undefined];

      try {
        await resumeSessionInTerminal(appType, sessionId, workingDir);
        return { success: true };
      } catch (error) {
        log.error('Failed to resume session:', error);
        throw error;
      }
    },
    {
      validateArgs: validateArgs(
        stringArg('sessionId'),
        appTypeArg('appType'),
        optionalStringArg('workingDir')
      ),
    }
  );

  // Get terminal info (for UI display)
  ipcRegistry.register(
    'sessions:getTerminalInfo',
    async () => {
      return getTerminalInfo();
    },
    { validateResult: terminalInfoResult() }
  );

  log.info('Sessions IPC handlers registered');
}

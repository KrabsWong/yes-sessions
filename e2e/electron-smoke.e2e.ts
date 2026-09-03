import {
  _electron as electron,
  expect,
  test,
  type ElectronApplication,
  type Page,
} from '@playwright/test';
import { execFile } from 'child_process';
import { mkdtemp, mkdir, realpath, rm, writeFile } from 'fs/promises';
import os from 'os';
import path from 'path';
import { promisify } from 'util';
import { fileURLToPath } from 'url';

type ApiResponse<T> = {
  success: boolean;
  data?: T;
  error?: {
    code: string;
    message: string;
    details?: unknown;
  };
};

type Session = {
  id: string;
  appType: string;
};

type ElectronApiWindow = Window & {
  electronAPI?: {
    invoke: (channel: string, ...args: unknown[]) => Promise<unknown>;
  };
};

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(__dirname, '..');
const execFileAsync = promisify(execFile);

function testEnv(overrides: Record<string, string> = {}): Record<string, string> {
  const env: Record<string, string> = {};

  for (const [key, value] of Object.entries(process.env)) {
    if (value !== undefined) {
      env[key] = value;
    }
  }

  env.NODE_ENV = 'test';
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = '1';
  return { ...env, ...overrides };
}

async function createCodebuddyFixture(options: { withGitChanges?: boolean } = {}): Promise<{
  homeDir: string;
  projectDir: string;
  packagePath: string;
  sessionId: string;
  childSessionId: string;
}> {
  const homeDir = await mkdtemp(path.join(os.tmpdir(), 'yes-sessions-e2e-home-'));
  const projectDir = path.join(homeDir, 'fixture-project');
  const packagePath = path.join(projectDir, 'package.json');
  const codebuddyProjectDir = path.join(homeDir, '.codebuddy', 'projects', 'fixture-project');
  const sessionId = 'fixture-session';
  const childSessionId = 'agent-fixture-child';
  const start = Date.UTC(2026, 4, 25, 9, 0, 0);

  await mkdir(projectDir, { recursive: true });
  await mkdir(codebuddyProjectDir, { recursive: true });
  await writeFile(packagePath, '{\n  "name": "yes-sessions",\n  "fixture": true\n}\n');

  if (options.withGitChanges) {
    await execFileAsync('git', ['init'], { cwd: projectDir });
    await execFileAsync('git', ['config', 'user.email', 'e2e@example.com'], { cwd: projectDir });
    await execFileAsync('git', ['config', 'user.name', 'Yes Sessions E2E'], { cwd: projectDir });
    await writeFile(packagePath, '{\n  "name": "yes-sessions",\n  "fixture": false\n}\n');
    await execFileAsync('git', ['add', 'package.json'], { cwd: projectDir });
    await execFileAsync('git', ['commit', '-m', 'Initial fixture'], { cwd: projectDir });
    await writeFile(packagePath, '{\n  "name": "yes-sessions",\n  "fixture": true\n}\n');
  }

  const lines = [
    {
      id: 'command-caveat',
      timestamp: start - 3,
      type: 'message',
      role: 'user',
      cwd: projectDir,
      content: [
        {
          type: 'input_text',
          text: '<system-reminder data-role="command-caveat">Ignore local command messages.</system-reminder>',
        },
      ],
    },
    {
      id: 'command-name',
      timestamp: start - 2,
      type: 'message',
      role: 'user',
      cwd: projectDir,
      content: [{ type: 'input_text', text: '<command-name>/clear</command-name>' }],
    },
    {
      id: 'command-output',
      timestamp: start - 1,
      type: 'message',
      role: 'user',
      cwd: projectDir,
      content: [{ type: 'input_text', text: '<local-command-stdout></local-command-stdout>' }],
    },
    {
      id: 'command-error',
      timestamp: start - 1,
      type: 'message',
      role: 'user',
      cwd: projectDir,
      content: [
        { type: 'input_text', text: '<local-command-stderr>hidden error</local-command-stderr>' },
      ],
    },
    {
      id: 'm1',
      timestamp: start,
      type: 'message',
      role: 'user',
      cwd: projectDir,
      providerData: { model: 'fixture-model' },
      content: [
        {
          type: 'input_text',
          text: '<system-reminder data-role="command-caveat">Ignore this.</system-reminder>\nSummarize fixture project',
        },
      ],
    },
    {
      id: 'call-1',
      timestamp: start + 1000,
      type: 'function_call',
      name: 'Read',
      callId: 'call-1',
      arguments: JSON.stringify({ file_path: path.join(projectDir, 'package.json') }),
    },
    {
      id: 'result-1',
      timestamp: start + 2000,
      type: 'function_call_result',
      name: 'Read',
      callId: 'call-1',
      output: { type: 'text', text: '{ "name": "yes-sessions" }' },
    },
    {
      id: 'm2',
      timestamp: start + 3000,
      type: 'message',
      role: 'assistant',
      content: [{ type: 'output_text', text: 'Fixture package is yes-sessions.' }],
    },
    {
      id: 'agent-call',
      timestamp: start + 4000,
      type: 'function_call',
      name: 'Agent',
      callId: 'agent-call',
      arguments: JSON.stringify({
        description: 'Inspect fixture rendering',
        subagent_type: 'Explore',
      }),
    },
    {
      id: 'agent-result',
      timestamp: start + 5000,
      type: 'function_call_result',
      name: 'Agent',
      callId: 'agent-call',
      status: 'completed',
      output: { type: 'text', text: 'Fixture sub-agent completed.' },
      providerData: { toolResult: { subAgent: { sessionId: childSessionId } } },
    },
  ];

  await writeFile(
    path.join(codebuddyProjectDir, `${sessionId}.jsonl`),
    lines.map((line) => JSON.stringify(line)).join('\n')
  );
  const childDir = path.join(codebuddyProjectDir, sessionId, 'subagents');
  await mkdir(childDir, { recursive: true });
  await writeFile(
    path.join(childDir, `${childSessionId}.jsonl`),
    [
      {
        id: 'child-user',
        sessionId: 'fixture-child-internal-session',
        timestamp: start + 4100,
        type: 'message',
        role: 'user',
        cwd: projectDir,
        content: [{ type: 'input_text', text: 'Inspect fixture rendering deeply' }],
        providerData: { isSubAgent: true, agent: 'Explore', model: 'fixture-model' },
      },
      {
        id: 'child-assistant',
        sessionId: 'fixture-child-internal-session',
        timestamp: start + 4900,
        type: 'message',
        role: 'assistant',
        content: [{ type: 'output_text', text: 'Nested fixture investigation result.' }],
        providerData: { isSubAgent: true, agent: 'Explore', model: 'fixture-model' },
      },
    ]
      .map((line) => JSON.stringify(line))
      .join('\n')
  );

  return {
    homeDir,
    projectDir: await realpath(projectDir),
    packagePath: await realpath(packagePath),
    sessionId,
    childSessionId,
  };
}

async function createLargeCodebuddyFixture(options: { turns: number }): Promise<{
  homeDir: string;
  projectDir: string;
  sessionId: string;
}> {
  const homeDir = await mkdtemp(path.join(os.tmpdir(), 'yes-sessions-e2e-large-home-'));
  const projectDir = path.join(homeDir, 'large-fixture-project');
  const codebuddyProjectDir = path.join(homeDir, '.codebuddy', 'projects', 'large-fixture-project');
  const sessionId = 'large-fixture-session';
  const start = Date.UTC(2026, 4, 25, 10, 0, 0);

  await mkdir(projectDir, { recursive: true });
  await mkdir(codebuddyProjectDir, { recursive: true });

  const lines: unknown[] = [];
  for (let index = 0; index < options.turns; index++) {
    const timestamp = start + index * 3000;
    const callId = `call-${index}`;
    lines.push(
      {
        id: `user-${index}`,
        timestamp,
        type: 'message',
        role: 'user',
        cwd: projectDir,
        providerData: { model: 'fixture-model' },
        content: [
          {
            type: 'input_text',
            text: `Large fixture request ${index}: ${'inspect '.repeat(8)}the project state.`,
          },
        ],
      },
      {
        id: callId,
        timestamp: timestamp + 500,
        type: 'function_call',
        name: 'Bash',
        callId,
        arguments: JSON.stringify({ command: `printf "turn ${index}"` }),
      },
      {
        id: `result-${index}`,
        timestamp: timestamp + 1000,
        type: 'function_call_result',
        name: 'Bash',
        callId,
        output: {
          type: 'text',
          text: Array.from(
            { length: 12 },
            (_, lineIndex) => `turn ${index} output ${lineIndex}`
          ).join('\n'),
        },
      },
      {
        id: `assistant-${index}`,
        timestamp: timestamp + 1500,
        type: 'message',
        role: 'assistant',
        content: [
          {
            type: 'output_text',
            text: `Large fixture response ${index}.\n\n${'- item\n'.repeat(6)}`,
          },
        ],
      }
    );
  }

  await writeFile(
    path.join(codebuddyProjectDir, `${sessionId}.jsonl`),
    lines.map((line) => JSON.stringify(line)).join('\n')
  );

  return {
    homeDir,
    projectDir: await realpath(projectDir),
    sessionId,
  };
}

async function createLargeOpenCodeFixture(options: { turns: number }): Promise<{
  homeDir: string;
  projectDir: string;
  sessionId: string;
}> {
  const homeDir = await mkdtemp(path.join(os.tmpdir(), 'yes-sessions-e2e-opencode-home-'));
  const projectDir = path.join(homeDir, 'opencode-fixture-project');
  const dbDir = path.join(homeDir, '.local', 'share', 'opencode');
  const dbPath = path.join(dbDir, 'opencode.db');
  const sqlPath = path.join(homeDir, 'opencode-fixture.sql');
  const sessionId = 'opencode-large-fixture-session';
  const start = Date.UTC(2026, 4, 25, 11, 0, 0);

  await mkdir(projectDir, { recursive: true });
  await mkdir(dbDir, { recursive: true });

  const escapeSql = (value: string): string => value.replace(/'/g, "''");
  const sql: string[] = [
    'CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, title TEXT, time_created INTEGER, time_updated INTEGER, time_archived INTEGER, summary_additions INTEGER, summary_deletions INTEGER, summary_files INTEGER);',
    'CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);',
    'CREATE TABLE part (id TEXT PRIMARY KEY, session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT);',
    `INSERT INTO session (id, directory, title, time_created, time_updated, time_archived, summary_additions, summary_deletions, summary_files) VALUES ('${sessionId}', '${escapeSql(projectDir)}', 'OpenCode large fixture', ${start}, ${
      start + options.turns * 3000
    }, NULL, 0, 0, 0);`,
  ];

  for (let index = 0; index < options.turns; index++) {
    const userMessageId = `user-${index}`;
    const assistantMessageId = `assistant-${index}`;
    const timestamp = start + index * 3000;
    const userData = JSON.stringify({ role: 'user', model: 'fixture-model' });
    const assistantData = JSON.stringify({ role: 'assistant', model: 'fixture-model' });
    const userPart = JSON.stringify({
      type: 'text',
      text: `OpenCode fixture request ${index}: ${'inspect '.repeat(8)}the project state.`,
    });
    const assistantPart = JSON.stringify({
      type: 'text',
      text: `OpenCode fixture response ${index}.\n\n${'- item\n'.repeat(8)}`,
    });

    sql.push(
      `INSERT INTO message (id, session_id, time_created, data) VALUES ('${userMessageId}', '${sessionId}', ${timestamp}, '${escapeSql(
        userData
      )}');`,
      `INSERT INTO part (id, session_id, message_id, time_created, data) VALUES ('user-part-${index}', '${sessionId}', '${userMessageId}', ${timestamp}, '${escapeSql(
        userPart
      )}');`,
      `INSERT INTO message (id, session_id, time_created, data) VALUES ('${assistantMessageId}', '${sessionId}', ${
        timestamp + 1000
      }, '${escapeSql(assistantData)}');`,
      `INSERT INTO part (id, session_id, message_id, time_created, data) VALUES ('assistant-part-${index}', '${sessionId}', '${assistantMessageId}', ${
        timestamp + 1000
      }, '${escapeSql(assistantPart)}');`
    );
  }

  await writeFile(sqlPath, sql.join('\n'));
  await execFileAsync('sqlite3', [dbPath, `.read ${sqlPath}`]);

  return {
    homeDir,
    projectDir: await realpath(projectDir),
    sessionId,
  };
}

async function waitForMainWindow(app: ElectronApplication): Promise<Page> {
  const deadline = Date.now() + 15_000;

  while (Date.now() < deadline) {
    for (const page of app.windows()) {
      const url = page.url();
      if (url.includes('dist/index.html') || url.includes('localhost:5173')) {
        await page.waitForLoadState('domcontentloaded');
        return page;
      }
    }

    await app.waitForEvent('window', { timeout: 500 }).catch(() => undefined);
  }

  throw new Error('Main Electron window did not load');
}

function expectSuccessfulResponse<T>(response: ApiResponse<T>, channel: string): T {
  expect(response.success, `${channel} failed: ${response.error?.message ?? 'unknown error'}`).toBe(
    true
  );
  return response.data as T;
}

async function selectApp(page: Page, appName: string): Promise<void> {
  const selector = page.getByRole('combobox');
  await selector.click();
  const exactOption = page.getByRole('option', { name: appName, exact: true });
  if ((await exactOption.count()) === 1) {
    await exactOption.click();
    return;
  }

  const matchingOptions = page.locator('[role="option"]').filter({ hasText: appName });
  expect(await matchingOptions.count()).toBeGreaterThan(0);
  await matchingOptions.nth(0).click();
}

async function selectCodebuddyApp(page: Page): Promise<void> {
  await selectApp(page, 'Codebuddy');
}

async function selectOpenCodeApp(page: Page): Promise<void> {
  await selectApp(page, 'OpenCode');
}

test('launches the built Electron app and serves core IPC channels', async () => {
  const launchStartedAt = Date.now();
  const electronApp = await electron.launch({
    args: [appRoot],
    env: testEnv(),
  });

  try {
    const mainWindow = await waitForMainWindow(electronApp);
    const mainWindowReadyMs = Date.now() - launchStartedAt;
    console.log(`[perf] startup mainWindowReady=${mainWindowReadyMs}ms`);
    expect(mainWindowReadyMs).toBeLessThan(5000);

    await expect
      .poll(() =>
        mainWindow.evaluate(() => {
          const win = window as ElectronApiWindow;
          return typeof win.electronAPI?.invoke === 'function';
        })
      )
      .toBe(true);

    const responses = await mainWindow.evaluate(async () => {
      const win = window as ElectronApiWindow;
      if (!win.electronAPI) {
        throw new Error('electronAPI is not available');
      }

      const invoke = (channel: string, ...args: unknown[]) =>
        win.electronAPI!.invoke(channel, ...args);

      const version = await invoke('app:getVersion');
      const settings = await invoke('settings:get');
      const terminalInfo = await invoke('sessions:getTerminalInfo');
      const supportStatus = await invoke('sessions:getSupportStatus', 'codebuddy');
      const stats = await invoke('sessions:getStats', 'codebuddy');
      const sessions = await invoke('sessions:getAll', 'codebuddy');

      let detail: unknown = null;
      const sessionData = (sessions as ApiResponse<Session[]>).data;
      if (
        (sessions as ApiResponse<Session[]>).success &&
        Array.isArray(sessionData) &&
        sessionData[0]
      ) {
        detail = await invoke('sessions:getDetail', sessionData[0].id, sessionData[0].appType);
      }

      return {
        version,
        settings,
        terminalInfo,
        supportStatus,
        stats,
        sessions,
        detail,
      };
    });

    const version = expectSuccessfulResponse<string>(
      responses.version as ApiResponse<string>,
      'app:getVersion'
    );
    expect(version).toMatch(/^\d+\.\d+\.\d+/);

    const settings = expectSuccessfulResponse<Record<string, unknown>>(
      responses.settings as ApiResponse<Record<string, unknown>>,
      'settings:get'
    );
    expect(settings).toHaveProperty('theme');
    expect(settings).toHaveProperty('language');

    const terminalInfo = expectSuccessfulResponse<Record<string, unknown>>(
      responses.terminalInfo as ApiResponse<Record<string, unknown>>,
      'sessions:getTerminalInfo'
    );
    expect(terminalInfo).toHaveProperty('preferred');

    const supportStatus = expectSuccessfulResponse<Record<string, unknown>>(
      responses.supportStatus as ApiResponse<Record<string, unknown>>,
      'sessions:getSupportStatus'
    );
    expect(supportStatus).toHaveProperty('supported');
    expect(supportStatus).toHaveProperty('isAvailable');

    const stats = expectSuccessfulResponse<Record<string, unknown>>(
      responses.stats as ApiResponse<Record<string, unknown>>,
      'sessions:getStats'
    );
    expect(typeof stats.totalSessions).toBe('number');
    expect(typeof stats.totalMessages).toBe('number');

    const sessions = expectSuccessfulResponse<Session[]>(
      responses.sessions as ApiResponse<Session[]>,
      'sessions:getAll'
    );
    expect(Array.isArray(sessions)).toBe(true);

    if (responses.detail) {
      const detail = expectSuccessfulResponse<Record<string, unknown>>(
        responses.detail as ApiResponse<Record<string, unknown>>,
        'sessions:getDetail'
      );
      expect(detail).toHaveProperty('messages');
      expect(Array.isArray(detail.messages)).toBe(true);
    }
  } finally {
    await electronApp.close();
  }
});

test('opens settings and switches between settings tabs', async () => {
  const electronApp = await electron.launch({
    args: [appRoot],
    env: testEnv(),
  });

  try {
    const mainWindow = await waitForMainWindow(electronApp);

    await mainWindow.getByTestId('settings-button').click();
    await expect(mainWindow.getByTestId('settings-dialog')).toBeVisible();
    await expect(mainWindow.getByTestId('settings-panel-general')).toBeVisible();

    await mainWindow.getByTestId('settings-tab-experience').click();
    await expect(mainWindow.getByTestId('settings-panel-experience')).toBeVisible();

    await mainWindow.getByTestId('settings-tab-terminal').click();
    await expect(mainWindow.getByTestId('settings-panel-terminal')).toBeVisible();

    await mainWindow.keyboard.press('Escape');
    await expect(mainWindow.getByTestId('settings-dialog')).toBeHidden();
  } finally {
    await electronApp.close();
  }
});

test('renders a fixture Codebuddy session from list to conversation detail', async () => {
  const fixture = await createCodebuddyFixture();
  const electronApp = await electron.launch({
    args: [appRoot],
    env: testEnv({
      HOME: fixture.homeDir,
      USERPROFILE: fixture.homeDir,
    }),
  });

  try {
    const mainWindow = await waitForMainWindow(electronApp);
    await selectCodebuddyApp(mainWindow);
    const statsTrigger = mainWindow.getByTestId('session-stats-trigger');
    const statsTooltip = mainWindow.getByTestId('session-stats-tooltip');
    await statsTrigger.focus();
    await expect(statsTooltip).toBeVisible();
    await statsTrigger.evaluate((element) => (element as HTMLElement).blur());
    await expect(statsTooltip).toBeHidden();
    await statsTrigger.hover();
    await expect(statsTooltip).toBeVisible();
    await expect(statsTooltip).toContainText(/Session statistics|会话统计/);
    await expect(statsTooltip).toContainText(/Total Sessions|会话总数/);
    await expect(statsTooltip).toContainText(/Sub Agent|子 Agent/);
    await expect(statsTooltip).toContainText(/Total Messages|消息总数/);
    const sessionCard = mainWindow.locator(
      `[data-testid="session-card"][data-session-id="${fixture.sessionId}"]`
    );

    await expect(sessionCard).toBeVisible();
    await expect(sessionCard).toContainText('Summarize fixture project');
    await expect(sessionCard).not.toContainText('command-caveat');
    await expect(
      mainWindow.locator(
        `[data-testid="session-card"][data-session-id="${fixture.childSessionId}"]`
      )
    ).toHaveCount(0);

    await sessionCard.getByTestId('toggle-subagents').click();
    const childSessionCard = mainWindow.locator(
      `[data-testid="session-card"][data-session-id="${fixture.childSessionId}"]`
    );
    await expect(childSessionCard).toBeVisible();
    await expect(childSessionCard).toContainText('Explore');

    await sessionCard.locator('button').first().click();

    const conversation = mainWindow.getByTestId('conversation-detail');
    await expect(conversation).toContainText('Summarize fixture project');
    await expect(conversation).not.toContainText('command-caveat');
    await expect(conversation).toContainText('Read');
    await expect(conversation).toContainText('package.json');
    await expect(conversation).toContainText('Fixture package is yes-sessions.');
    await expect(conversation).toContainText(/Sub Agent|子 Agent/);
    await expect(conversation).toContainText('Explore');

    await conversation
      .getByRole('button', { name: /Expand sub-agent conversation|展开子 Agent 对话/ })
      .click();
    await expect(conversation).toContainText('Nested fixture investigation result.');

    await conversation
      .getByRole('button', { name: /View Sub-Agent Session|查看子 Agent 会话/ })
      .click();
    await expect(mainWindow.getByText(/Back to parent session|返回主会话/)).toBeVisible();
    await expect(conversation).toContainText('Nested fixture investigation result.');

    await mainWindow.getByRole('button', { name: /Back to parent session|返回主会话/ }).click();
    await expect(conversation).toContainText('Summarize fixture project');
  } finally {
    await electronApp.close();
    await rm(fixture.homeDir, { recursive: true, force: true });
  }
});

test('opens file preview window and reads a fixture project file', async () => {
  const fixture = await createCodebuddyFixture();
  const electronApp = await electron.launch({
    args: [appRoot],
    env: testEnv({
      HOME: fixture.homeDir,
      USERPROFILE: fixture.homeDir,
    }),
  });

  try {
    const mainWindow = await waitForMainWindow(electronApp);
    await selectCodebuddyApp(mainWindow);
    await expect(
      mainWindow.locator(`[data-testid="session-card"][data-session-id="${fixture.sessionId}"]`)
    ).toBeVisible();

    const previewWindowPromise = electronApp.waitForEvent('window');
    await mainWindow.getByTestId('open-file-preview-button').click();
    const previewWindow = await previewWindowPromise;
    await previewWindow.waitForLoadState('domcontentloaded');

    await expect(previewWindow.getByTestId('file-preview-window')).toBeVisible();
    await expect(previewWindow.getByTestId('file-preview-empty')).toBeVisible();

    await previewWindow
      .locator(`[data-testid="file-tree-file"][data-file-path="${fixture.packagePath}"]`)
      .click();
    await expect(previewWindow.getByTestId('file-preview-panel')).toBeVisible();
    await expect(previewWindow.getByTestId('file-preview-content')).toContainText('yes-sessions');
    await expect(previewWindow.getByTestId('file-preview-content')).toContainText('"fixture"');
  } finally {
    await electronApp.close();
    await rm(fixture.homeDir, { recursive: true, force: true });
  }
});

test('opens git diff in the file preview window for a fixture repository change', async () => {
  const fixture = await createCodebuddyFixture({ withGitChanges: true });
  const electronApp = await electron.launch({
    args: [appRoot],
    env: testEnv({
      HOME: fixture.homeDir,
      USERPROFILE: fixture.homeDir,
    }),
  });

  try {
    const mainWindow = await waitForMainWindow(electronApp);
    await selectCodebuddyApp(mainWindow);
    await expect(
      mainWindow.locator(`[data-testid="session-card"][data-session-id="${fixture.sessionId}"]`)
    ).toBeVisible();

    const previewWindowPromise = electronApp.waitForEvent('window');
    await mainWindow.getByTestId('open-file-preview-button').click();
    const previewWindow = await previewWindowPromise;
    await previewWindow.waitForLoadState('domcontentloaded');

    await previewWindow.getByTestId('git-tab').click();
    await expect(previewWindow.getByTestId('git-diff-view')).toBeVisible();

    await previewWindow
      .locator('[data-testid="git-change-row"][data-file-path="package.json"]')
      .click();
    await expect(previewWindow.getByTestId('git-diff-preview')).toBeVisible();
    await expect(previewWindow.getByTestId('git-diff-preview')).toContainText('"fixture": false');
    await expect(previewWindow.getByTestId('git-diff-preview')).toContainText('"fixture": true');
  } finally {
    await electronApp.close();
    await rm(fixture.homeDir, { recursive: true, force: true });
  }
});

test('opens and scrolls a large fixture session without visible jank', async () => {
  const fixture = await createLargeCodebuddyFixture({ turns: 900 });
  const electronApp = await electron.launch({
    args: [appRoot],
    env: testEnv({
      HOME: fixture.homeDir,
      USERPROFILE: fixture.homeDir,
    }),
  });

  try {
    const mainWindow = await waitForMainWindow(electronApp);
    await selectCodebuddyApp(mainWindow);
    const sessionCard = mainWindow.locator(
      `[data-testid="session-card"][data-session-id="${fixture.sessionId}"]`
    );
    await expect(sessionCard).toBeVisible();

    const openStartedAt = Date.now();
    await sessionCard.click();

    const conversation = mainWindow.getByTestId('conversation-detail');
    await expect(conversation).toContainText('Large fixture request 0');
    const openDurationMs = Date.now() - openStartedAt;

    const scrollMetrics = await mainWindow.evaluate(async () => {
      const container = document.querySelector<HTMLElement>('[data-testid="conversation-detail"]');
      if (!container) {
        throw new Error('conversation-detail not found');
      }

      return new Promise<{
        frames: number;
        maxFrameMs: number;
        averageFrameMs: number;
        longFrames: number;
        scrollHeight: number;
      }>((resolve) => {
        const frameDeltas: number[] = [];
        const totalFrames = 90;
        let frame = 0;
        let previous = performance.now();

        const step = () => {
          const now = performance.now();
          frameDeltas.push(now - previous);
          previous = now;
          frame++;
          container.scrollTop =
            ((container.scrollHeight - container.clientHeight) * frame) / totalFrames;

          if (frame < totalFrames) {
            requestAnimationFrame(step);
            return;
          }

          const maxFrameMs = Math.max(...frameDeltas);
          const averageFrameMs =
            frameDeltas.reduce((sum, delta) => sum + delta, 0) / frameDeltas.length;

          resolve({
            frames: frameDeltas.length,
            maxFrameMs,
            averageFrameMs,
            longFrames: frameDeltas.filter((delta) => delta > 50).length,
            scrollHeight: container.scrollHeight,
          });
        };

        requestAnimationFrame(step);
      });
    });

    console.log(
      `[perf] large-session open=${openDurationMs}ms averageFrame=${scrollMetrics.averageFrameMs.toFixed(
        1
      )}ms maxFrame=${scrollMetrics.maxFrameMs.toFixed(1)}ms longFrames=${
        scrollMetrics.longFrames
      } scrollHeight=${scrollMetrics.scrollHeight}px`
    );

    expect(openDurationMs).toBeLessThan(1500);
    expect(scrollMetrics.scrollHeight).toBeGreaterThan(5000);
    expect(scrollMetrics.frames).toBe(90);
    expect(scrollMetrics.averageFrameMs).toBeLessThan(25);
    expect(scrollMetrics.longFrames).toBeLessThanOrEqual(3);
  } finally {
    await electronApp.close();
    await rm(fixture.homeDir, { recursive: true, force: true });
  }
});

test('opens and scrolls a large OpenCode fixture session without visible jank', async () => {
  const fixture = await createLargeOpenCodeFixture({ turns: 900 });
  const electronApp = await electron.launch({
    args: [appRoot],
    env: testEnv({
      HOME: fixture.homeDir,
      USERPROFILE: fixture.homeDir,
    }),
  });

  try {
    const mainWindow = await waitForMainWindow(electronApp);
    await selectOpenCodeApp(mainWindow);
    const sessionCard = mainWindow.locator(
      `[data-testid="session-card"][data-session-id="${fixture.sessionId}"]`
    );
    await expect(sessionCard).toBeVisible();

    const openStartedAt = Date.now();
    await sessionCard.click();

    const conversation = mainWindow.getByTestId('conversation-detail');
    await expect(conversation).toContainText('OpenCode fixture request 0');
    const openDurationMs = Date.now() - openStartedAt;

    const scrollMetrics = await mainWindow.evaluate(async () => {
      const container = document.querySelector<HTMLElement>('[data-testid="conversation-detail"]');
      if (!container) {
        throw new Error('conversation-detail not found');
      }

      return new Promise<{
        frames: number;
        maxFrameMs: number;
        averageFrameMs: number;
        longFrames: number;
        scrollHeight: number;
      }>((resolve) => {
        const frameDeltas: number[] = [];
        const totalFrames = 90;
        let frame = 0;
        let previous = performance.now();

        const step = () => {
          const now = performance.now();
          frameDeltas.push(now - previous);
          previous = now;
          frame++;
          container.scrollTop =
            ((container.scrollHeight - container.clientHeight) * frame) / totalFrames;

          if (frame < totalFrames) {
            requestAnimationFrame(step);
            return;
          }

          const maxFrameMs = Math.max(...frameDeltas);
          const averageFrameMs =
            frameDeltas.reduce((sum, delta) => sum + delta, 0) / frameDeltas.length;

          resolve({
            frames: frameDeltas.length,
            maxFrameMs,
            averageFrameMs,
            longFrames: frameDeltas.filter((delta) => delta > 50).length,
            scrollHeight: container.scrollHeight,
          });
        };

        requestAnimationFrame(step);
      });
    });

    console.log(
      `[perf] opencode-large-session open=${openDurationMs}ms averageFrame=${scrollMetrics.averageFrameMs.toFixed(
        1
      )}ms maxFrame=${scrollMetrics.maxFrameMs.toFixed(1)}ms longFrames=${
        scrollMetrics.longFrames
      } scrollHeight=${scrollMetrics.scrollHeight}px`
    );

    expect(openDurationMs).toBeLessThan(1500);
    expect(scrollMetrics.scrollHeight).toBeGreaterThan(5000);
    expect(scrollMetrics.frames).toBe(90);
    expect(scrollMetrics.averageFrameMs).toBeLessThan(25);
    expect(scrollMetrics.longFrames).toBeLessThanOrEqual(3);
  } finally {
    await electronApp.close();
    await rm(fixture.homeDir, { recursive: true, force: true });
  }
});

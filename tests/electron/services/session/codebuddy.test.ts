import fs from 'fs';
import os from 'os';
import path from 'path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { sessionDetailResult } from '@electron/ipc/validation';

const tempRoots: string[] = [];

vi.mock('electron-log', () => ({
  default: { error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

function writeJsonl(filePath: string, records: unknown[]): void {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, records.map((record) => JSON.stringify(record)).join('\n'));
}

async function importServiceWithHome(homePath: string) {
  vi.resetModules();
  vi.doMock('os', () => ({
    default: { homedir: () => homePath },
    homedir: () => homePath,
  }));
  return import('@electron/services/session/codebuddy');
}

describe('CodeBuddy session service', () => {
  afterEach(() => {
    vi.doUnmock('os');
    for (const root of tempRoots.splice(0)) {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it('normalizes modern records and recursively discovers sub-agent sessions', async () => {
    const homePath = fs.mkdtempSync(path.join(os.tmpdir(), 'yes-sessions-codebuddy-'));
    tempRoots.push(homePath);

    const codebuddyHome = path.join(homePath, '.codebuddy');
    const projectDir = path.join(codebuddyHome, 'projects', 'Users-example-work-my-project');
    const mainSessionId = '11111111-1111-4111-8111-111111111111';
    const childInternalId = '22222222-2222-4222-8222-222222222222';
    const mainFile = path.join(projectDir, `${mainSessionId}.jsonl`);
    const childFile = path.join(projectDir, mainSessionId, 'subagents', 'agent-child.jsonl');
    const grandchildFile = path.join(
      projectDir,
      childInternalId,
      'subagents',
      'agent-grandchild.jsonl'
    );
    const imagePath = path.join(codebuddyHome, 'blobs', 'ab', 'attached.png');
    const cwd = path.join(homePath, 'work', 'my-project');
    const start = 1_800_000_000_000;

    fs.mkdirSync(path.dirname(imagePath), { recursive: true });
    fs.writeFileSync(imagePath, Buffer.from('image-bytes'));
    writeJsonl(mainFile, [
      {
        id: 'user-1',
        sessionId: mainSessionId,
        cwd,
        timestamp: start,
        type: 'message',
        role: 'user',
        content: [
          { type: 'input_text', text: 'first block' },
          { type: 'input_text', text: 'second block' },
          { type: 'image_blob_ref', blob_path: imagePath, mime: 'image/png' },
        ],
      },
      {
        id: 'reasoning-1',
        sessionId: mainSessionId,
        cwd,
        timestamp: start + 1,
        type: 'reasoning',
        rawContent: [{ type: 'reasoning_text', text: 'inspect the repository' }],
        providerData: { model: 'codebuddy-model' },
      },
      {
        id: 'assistant-1',
        sessionId: mainSessionId,
        cwd,
        timestamp: start + 2,
        type: 'message',
        role: 'assistant',
        content: [{ type: 'output_text', text: 'I will delegate this.' }],
        providerData: { model: 'codebuddy-model' },
      },
      {
        id: 'agent-call',
        sessionId: mainSessionId,
        cwd,
        timestamp: start + 3,
        type: 'function_call',
        name: 'Agent',
        callId: 'call-agent',
        arguments: JSON.stringify({ description: 'inspect', prompt: 'review' }),
      },
      {
        id: 'agent-result',
        sessionId: mainSessionId,
        cwd,
        timestamp: start + 4,
        type: 'function_call_result',
        name: 'Agent',
        callId: 'call-agent',
        status: 'completed',
        output: { type: 'text', text: 'Completed successfully' },
        providerData: {
          toolResult: { subAgent: { sessionId: 'agent-child' } },
        },
      },
      {
        id: 'string-call',
        sessionId: mainSessionId,
        cwd,
        timestamp: start + 5,
        type: 'function_call',
        name: 'AskUserQuestion',
        callId: 'call-string',
        arguments: JSON.stringify('continue?'),
      },
    ]);
    writeJsonl(childFile, [
      {
        id: 'child-user',
        sessionId: childInternalId,
        cwd,
        timestamp: start + 10,
        type: 'message',
        role: 'user',
        content: [{ type: 'input_text', text: 'child task' }],
        providerData: { isSubAgent: true, agent: 'Explore' },
      },
      {
        id: 'child-assistant',
        sessionId: childInternalId,
        cwd,
        timestamp: start + 11,
        type: 'message',
        role: 'assistant',
        content: [{ type: 'output_text', text: 'child result' }],
      },
    ]);
    writeJsonl(grandchildFile, [
      {
        id: 'grandchild-user',
        sessionId: '33333333-3333-4333-8333-333333333333',
        cwd,
        timestamp: start + 20,
        type: 'message',
        role: 'user',
        content: [{ type: 'input_text', text: 'nested child task' }],
        providerData: { isSubAgent: true, agent: 'Plan' },
      },
    ]);
    fs.writeFileSync(path.join(projectDir, 'empty.jsonl'), '');

    const sessionsDir = path.join(codebuddyHome, 'sessions');
    fs.mkdirSync(sessionsDir, { recursive: true });
    fs.writeFileSync(
      path.join(sessionsDir, 'main.json'),
      JSON.stringify({
        sessionId: mainSessionId,
        updatedAt: start + 100,
        meta: { currentTopic: 'Active topic' },
      })
    );
    fs.writeFileSync(
      path.join(sessionsDir, 'child.json'),
      JSON.stringify({ sessionId: childInternalId, updatedAt: start + 101 })
    );

    const { codebuddySessionService } = await importServiceWithHome(homePath);
    const sessions = await codebuddySessionService.getAllSessions();

    expect(new Set(sessions.map((session) => session.id))).toEqual(
      new Set([mainSessionId, 'agent-child', 'agent-grandchild'])
    );
    expect(sessions.find((session) => session.id === mainSessionId)).toMatchObject({
      fileName: 'Active topic',
      directory: cwd,
      kind: 'main',
      updatedAt: start + 100,
    });
    expect(sessions.find((session) => session.id === 'agent-child')).toMatchObject({
      uuid: childInternalId,
      kind: 'subagent',
      parentSessionId: mainSessionId,
      agentType: 'Explore',
      updatedAt: start + 101,
    });
    expect(sessions.some((session) => session.id === 'empty')).toBe(false);
    expect(sessions.find((session) => session.id === 'agent-grandchild')).toMatchObject({
      kind: 'subagent',
      parentSessionId: 'agent-child',
      agentType: 'Plan',
    });

    const createReadStreamSpy = vi.spyOn(fs, 'createReadStream');
    const cachedSessions = await codebuddySessionService.getAllSessions();
    expect(cachedSessions.map((session) => session.id)).toEqual(
      sessions.map((session) => session.id)
    );
    expect(createReadStreamSpy).not.toHaveBeenCalled();
    createReadStreamSpy.mockRestore();

    const detail = await codebuddySessionService.getSessionDetail(mainSessionId);
    expect(detail).not.toBeNull();
    expect(() => sessionDetailResult()(detail)).not.toThrow();
    expect(detail?.messages[0].content).toContain('first block\nsecond block');
    expect(detail?.messages[0].content).toMatch(/!\[attached\.png]\(data:image\/png;base64,/);
    expect(detail?.messages[1]).toMatchObject({
      type: 'assistant',
      content: 'I will delegate this.',
      reasoning_content: 'inspect the repository',
      model: 'codebuddy-model',
    });
    expect(detail?.messages[3].metadata).toMatchObject({
      childSessionId: 'agent-child',
      childSessionAppType: 'codebuddy',
    });
    expect(detail?.messages[4].tool_input).toEqual({ value: 'continue?' });

    const childDetail = await codebuddySessionService.getSessionDetail(childInternalId);
    expect(childDetail).toMatchObject({ id: 'agent-child', uuid: childInternalId });
    expect(childDetail?.messages.map((message) => message.content)).toEqual([
      'child task',
      'child result',
    ]);
  });
});

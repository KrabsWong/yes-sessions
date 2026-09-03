import { describe, expect, it } from 'vitest';
import { hasSpecialParser, parseMessageContent } from '@/components/sessions/parsers';

describe('message content parsers', () => {
  it('parses OpenCode file reference blocks and strips line numbers', () => {
    const content = [
      'Before file',
      '<path>src/index.ts</path>',
      '<type>text</type>',
      '<content>1: const value = 1;',
      '2: export default value;</content>',
      'After file',
    ].join('\n');

    expect(parseMessageContent(content, 'opencode')).toEqual([
      { type: 'text', content: 'Before file' },
      {
        type: 'file',
        content: 'const value = 1;\nexport default value;',
        metadata: {
          path: 'src/index.ts',
          type: 'text',
        },
      },
      { type: 'text', content: 'After file' },
    ]);
  });

  it('cleans Claude Code pasted/loading markers before parsing', () => {
    const content = [
      '[Pasted ~20 lines]',
      'Loading from /tmp/example...',
      '<path>README.md</path><type>text</type><content>1: # Title</content>',
    ].join('\n');

    expect(parseMessageContent(content, 'claude')).toEqual([
      {
        type: 'file',
        content: '# Title',
        metadata: {
          path: 'README.md',
          type: 'text',
        },
      },
    ]);
  });

  it('removes Codebuddy system wrapper content', () => {
    const content = [
      '<system-reminder data-role="command-caveat">Ignore this</system-reminder>',
      '<command-name>/clear</command-name>',
      '<local-command-stdout>hidden</local-command-stdout>',
      '<system-reminder data-role="tool-hint">Keep this hint</system-reminder>',
      '<system-reminder data-role="error-recovery">Keep recovery guidance</system-reminder>',
      '<system-reminder data-role=command-caveat-extra>Keep similar role</system-reminder>',
      'Visible request',
    ].join('\n');

    expect(parseMessageContent(content, 'codebuddy')).toEqual([
      {
        type: 'text',
        content:
          '<system-reminder data-role="tool-hint">Keep this hint</system-reminder>\n<system-reminder data-role="error-recovery">Keep recovery guidance</system-reminder>\n<system-reminder data-role=command-caveat-extra>Keep similar role</system-reminder>\nVisible request',
      },
    ]);
  });

  it('keeps short Codebuddy prompts', () => {
    expect(parseMessageContent('好', 'codebuddy')).toEqual([{ type: 'text', content: '好' }]);
    expect(parseMessageContent('ok', 'codebuddy')).toEqual([{ type: 'text', content: 'ok' }]);
  });

  it('falls back to plain text for apps without a special parser', () => {
    expect(hasSpecialParser('unknown-app')).toBe(false);
    expect(parseMessageContent('raw text', 'unknown-app')).toEqual([
      { type: 'text', content: 'raw text' },
    ]);
  });
});

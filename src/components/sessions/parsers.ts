/**
 * Message Content Parsers
 *
 * 用于处理不同应用中消息内容的特殊格式
 */

export interface ParsedContent {
  type: 'text' | 'file' | 'tool' | 'code';
  content: string;
  metadata?: Record<string, string>;
}

export type ContentParser = (content: string) => ParsedContent[];

/**
 * 去除文件内容中的行号
 * 例如: "1: content\n2: content" -> "content\ncontent"
 */
function stripLineNumbers(content: string): string {
  // 匹配行首的数字和冒号，如 "123: " 或 "1:"
  return content
    .split('\n')
    .map((line) => line.replace(/^\d+:\s?/, ''))
    .join('\n');
}

/**
 * OpenCode 文件引用解析器
 * 处理格式: <path>...</path>\n<type>...</type>\n<content>...</content>
 */
const opencodeFileParser: ContentParser = (content: string) => {
  const results: ParsedContent[] = [];
  const fileBlockRegex =
    /<path>([\s\S]*?)<\/path>\s*<type>([\s\S]*?)<\/type>\s*<content>([\s\S]*?)<\/content>/g;

  let lastIndex = 0;
  let match;

  while ((match = fileBlockRegex.exec(content)) !== null) {
    // 添加匹配前的普通文本
    if (match.index > lastIndex) {
      const textBefore = content.slice(lastIndex, match.index).trim();
      if (textBefore) {
        results.push({
          type: 'text',
          content: textBefore,
        });
      }
    }

    // 添加文件引用块
    const [_, filePath, fileType, fileContent] = match;
    // 去除行号后再存储
    const cleanedContent = stripLineNumbers(fileContent.trim());
    results.push({
      type: 'file',
      content: cleanedContent,
      metadata: {
        path: filePath.trim(),
        type: fileType.trim(),
      },
    });

    lastIndex = fileBlockRegex.lastIndex;
  }

  // 添加剩余文本
  if (lastIndex < content.length) {
    const remainingText = content.slice(lastIndex).trim();
    if (remainingText) {
      results.push({
        type: 'text',
        content: remainingText,
      });
    }
  }

  // 如果没有匹配到任何文件块，返回原始内容
  if (results.length === 0) {
    results.push({
      type: 'text',
      content: content,
    });
  }

  return results;
};

/**
 * Claude Code 内容解析器
 * 1. 先清理系统生成的加载文本
 * 2. 然后解析文件引用（与 OpenCode 格式相同）
 */
const claudeCodeParser: ContentParser = (content: string) => {
  // 第一步：清理系统文本
  const patterns = [
    /\[Pasted ~\d+ lines?\]/g,
    /Loading from\s+\S+\.\.\.?/g,
    /\d+\s+bytes?\s+from\s+\S+/g,
    /\[Uploaded file:[^\]]*\]/g,
  ];

  let cleanedContent = content;
  for (const pattern of patterns) {
    cleanedContent = cleanedContent.replace(pattern, '');
  }
  cleanedContent = cleanedContent.replace(/\n{3,}/g, '\n\n').trim();

  // 第二步：复用 OpenCode 的文件解析逻辑
  return opencodeFileParser(cleanedContent);
};

/**
 * Codebuddy 内容解析器
 * 处理 Codebuddy 的特殊消息格式
 */
const codebuddyParser: ContentParser = (content: string) => {
  // 清理系统生成的文本
  const patterns = [
    // 粘贴提示
    /\[Pasted ~\d+ lines?\]/g,
    // command caveat 及旧版无属性 system-reminder
    /<system-reminder\b(?=[^>]*\bdata-role\s*=\s*(?:"command-caveat"|'command-caveat'|command-caveat(?=\s|\/?>)))[^>]*>[\s\S]*?<\/system-reminder\s*>/gi,
    /<system-reminder>[\s\S]*?<\/system-reminder\s*>/gi,
    // local-command 相关标签
    /<local-command-stdout\b[^>]*>[\s\S]*?<\/local-command-stdout\s*>/gi,
    /<local-command-stderr\b[^>]*>[\s\S]*?<\/local-command-stderr\s*>/gi,
    /<command-name\b[^>]*>[\s\S]*?<\/command-name\s*>/gi,
    // 清理空标签
    /<\w+><\/\w+>/g,
  ];

  let cleanedContent = content;
  for (const pattern of patterns) {
    cleanedContent = cleanedContent.replace(pattern, '');
  }
  cleanedContent = cleanedContent.replace(/\n{3,}/g, '\n\n').trim();

  // Keep short prompts such as "ok", "1", or Chinese single-character replies.
  if (!cleanedContent) {
    return [
      {
        type: 'text',
        content: '', // 返回空内容，让上层组件决定如何展示
      },
    ];
  }

  return [
    {
      type: 'text',
      content: cleanedContent,
    },
  ];
};

/**
 * 默认解析器 - 将内容作为纯文本处理
 */
const defaultParser: ContentParser = (content: string) => [
  {
    type: 'text',
    content: content,
  },
];

/**
 * 内置解析器表
 */
const parserRegistry: Record<string, ContentParser> = {
  opencode: opencodeFileParser,
  claude: claudeCodeParser,
  codebuddy: codebuddyParser,
};

/**
 * 解析消息内容
 * @param content - 原始消息内容
 * @param appType - 应用类型
 * @returns 解析后的内容块数组
 */
export function parseMessageContent(content: string, appType?: string): ParsedContent[] {
  const parser = appType ? parserRegistry[appType] || defaultParser : defaultParser;
  return parser(content);
}

/**
 * 检查是否需要特殊解析
 * @param appType - 应用类型
 * @returns 是否有注册的解析器
 */
export function hasSpecialParser(appType?: string): boolean {
  if (!appType) return false;
  return appType in parserRegistry;
}

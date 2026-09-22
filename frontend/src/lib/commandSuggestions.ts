// 终端输入的命令模糊建议（P1-1）：对命令历史 + 快速命令做大小写不敏感的
// 子序列匹配（fzf 风格），连续命中、词首命中、完全前缀依次加分。纯函数——
// 数据来源（commandHistory / quickCommands）与浮层渲染都留在调用方。

export interface CommandSuggestionQuickSource {
  name: string;
  command: string;
}

export interface CommandSuggestionSources {
  history: readonly string[];
  quickCommands: readonly CommandSuggestionQuickSource[];
}

export type CommandSuggestionSourceKind = "history" | "quick";

export interface CommandSuggestion {
  command: string;
  score: number;
  source: CommandSuggestionSourceKind;
  /** 命中字符在命令串中的下标（按升序），供浮层做匹配高亮。 */
  indices: number[];
}

export interface SearchCommandsOptions {
  limit?: number;
  minLength?: number;
  maxLength?: number;
}

export const SEARCH_COMMANDS_DEFAULTS = {
  limit: 12,
  minLength: 2,
  maxLength: 64,
} as const;

const SCORE_MATCH = 1;
const SCORE_CONSECUTIVE = 6;
const SCORE_WORD_START = 10;
const SCORE_QUICK_SOURCE = 4;
const SCORE_PREFIX = 40;
const SCORE_EXACT = 20;
const SCORE_RECENT = 5;
const RECENT_WINDOW = 5;

export interface SubsequenceMatch {
  score: number;
  indices: number[];
}

/** 非字母数字即视为词边界（空格、-、/、.、_ 等）。 */
function isWordStart(command: string, index: number): boolean {
  if (index === 0) return true;
  const previous = command.charAt(index - 1);
  return !/[a-z0-9]/i.test(previous);
}

/**
 * Case-insensitive subsequence match of `query` inside `command`.
 * Returns null when the query is not a subsequence; otherwise the ordered
 * match indices plus a score where consecutive runs, word starts and full
 * prefixes are progressively rewarded.
 */
export function scoreSubsequence(query: string, command: string): SubsequenceMatch | null {
  const needle = query.toLowerCase();
  const haystack = command.toLowerCase();
  const indices: number[] = [];
  let score = 0;
  let cursor = 0;
  for (let i = 0; i < needle.length; i += 1) {
    const found = haystack.indexOf(needle.charAt(i), cursor);
    if (found === -1) return null;
    score += SCORE_MATCH;
    if (indices.length && found === indices[indices.length - 1] + 1) score += SCORE_CONSECUTIVE;
    if (isWordStart(command, found)) score += SCORE_WORD_START;
    indices.push(found);
    cursor = found + 1;
  }
  if (haystack.startsWith(needle)) score += SCORE_PREFIX;
  if (haystack === needle) score += SCORE_EXACT;
  return { score, indices };
}

/** 查询长度门（含两端）：过短噪音大、过长失去建议意义。 */
export function commandSuggestionQueryAcceptable(query: string, minLength: number, maxLength: number): boolean {
  const length = query.length;
  return length >= minLength && length <= maxLength;
}

/**
 * Fuzzy-search commands across history and quick-command sources.
 * Rows are sorted by score (desc, stable — history rows win ties), deduped by
 * command text, and capped at `limit`. Queries outside the length bounds or
 * blank queries yield an empty list.
 */
export function searchCommands(
  query: string,
  sources: CommandSuggestionSources,
  options: SearchCommandsOptions = {},
): CommandSuggestion[] {
  const limit = options.limit ?? SEARCH_COMMANDS_DEFAULTS.limit;
  const minLength = options.minLength ?? SEARCH_COMMANDS_DEFAULTS.minLength;
  const maxLength = options.maxLength ?? SEARCH_COMMANDS_DEFAULTS.maxLength;
  const trimmed = query.trim();
  if (!trimmed || !commandSuggestionQueryAcceptable(trimmed, minLength, maxLength)) return [];

  const rows: CommandSuggestion[] = [];
  const seen = new Set<string>();
  const push = (command: string, source: CommandSuggestionSourceKind, recency: number) => {
    const key = command;
    if (!key || seen.has(key)) return;
    const match = scoreSubsequence(trimmed, key);
    if (!match) return;
    seen.add(key);
    rows.push({
      command: key,
      score: match.score + (source === "quick" ? SCORE_QUICK_SOURCE : 0) + Math.max(0, SCORE_RECENT - recency),
      source,
      indices: match.indices,
    });
  };
  sources.history.forEach((command, index) => push(command, "history", index));
  sources.quickCommands.forEach((item, index) => push(item.command, "quick", index));

  return rows.sort((a, b) => b.score - a.score).slice(0, Math.max(0, limit));
}

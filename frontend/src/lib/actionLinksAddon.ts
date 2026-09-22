// 动作链接的 xterm.js 接入层（P1-2）：把 matchActionLinks 的纯命中包装成
// ILinkProvider 供 terminal.registerLinkProvider 消费。本模块不执行命令——
// 点击行为经回调上抛，由 App.vue 决定（填充输入行 / 预览命令文本）。
// 命中装饰（虚线下划线）不走这里：xterm 的 link 装饰只在悬停时出现，常驻
// 虚线由 App.vue 的 decoration 引擎按关键词高亮同款防自激策略绘制。

import type { ILink, ILinkProvider, Terminal } from "@xterm/xterm";
import { matchActionLinks, ACTION_LINK_MATCHES_PER_LINE_LIMIT, type ActionLinkMatch, type ActionLinkMatcherToggles } from "./actionLinksMatcher";

/** 提供链路事件给 App.vue：命中对象 + 原始鼠标事件（Alt 判定用）。 */
export interface ActionLinkProviderCallbacks {
  /** 点击命中（xterm 只在无拖选的 mouseup 上触发）。 */
  onActivate(match: ActionLinkMatch, event: MouseEvent): void;
  /** 悬停命中（tooltip 预览命令文本）。 */
  onHover?(match: ActionLinkMatch, event: MouseEvent): void;
  /** 离开命中（收起 tooltip）。 */
  onLeave?(): void;
}

export interface ActionLinkProviderOptions {
  /** 三类匹配器开关；缺省全开。 */
  matchers?: ActionLinkMatcherToggles;
  callbacks: ActionLinkProviderCallbacks;
}

/**
 * 构造 ILinkProvider。xterm 会对视口内每一行调用 provideLinks（悬停与
 * 修饰键命中测试时），这里每次现算——纯正则扫描在 2048 字符行上限内开销
 * 可忽略，不值得引入行缓存失效的正确性负担。
 */
export function createActionLinkProvider(terminal: Terminal, options: ActionLinkProviderOptions): ILinkProvider {
  const matchers = options.matchers;
  return {
    provideLinks(bufferLineNumber: number, callback: (links: ILink[] | undefined) => void) {
      const line = terminal.buffer.active.getLine(bufferLineNumber);
      const text = line?.translateToString(true) ?? "";
      if (!text) {
        callback(undefined);
        return;
      }
      const matches = matchActionLinks(text, matchers).slice(0, ACTION_LINK_MATCHES_PER_LINE_LIMIT);
      if (!matches.length) {
        callback(undefined);
        return;
      }
      // IBufferRange 的 x 是 1-based，end 为 exclusive（与 @xterm/addon-web-links
      // 的 range 构造同一口径：end.x = 0-based end + 1）。
      const links = matches.map((match): ILink => ({
        range: {
          start: { x: match.start + 1, y: bufferLineNumber },
          end: { x: match.end + 1, y: bufferLineNumber },
        },
        text: match.text,
        // 悬停时 xterm 自带 pointer 光标 + 实线下划线；常驻虚线由 decoration 层画。
        decorations: { pointerCursor: true, underline: false },
        activate: (event: MouseEvent) => options.callbacks.onActivate(match, event),
        hover: (event: MouseEvent) => options.callbacks.onHover?.(match, event),
        leave: () => options.callbacks.onLeave?.(),
      }));
      callback(links);
    },
  };
}

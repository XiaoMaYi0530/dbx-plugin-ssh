// AI 终端同步执行（agent terminal mode）前端契约：类型 + 纯函数。
// 协议来源：ssh/docs/IMPL_PLAN_AGENT_TERMINAL.zh-CN.md §3（事件/RPC 契约钉死）。
// 事件 payload 均为 camelCase；requestedAt 为 unix 秒，timeoutSecs 为秒。

/** 连接级 agentTerminalMode 三档；顺序即设置弹窗下拉顺序，值即协议值。 */
export const AGENT_MODES = ["off", "auto", "strict"] as const;

export type AgentTerminalMode = (typeof AGENT_MODES)[number];

export type AgentRisk = "low" | "elevated";

export type AgentFinishStatus = "done" | "timeout" | "denied";

/** `ssh/agent/prompt` 事件 payload：审批挑战（超时默认拒绝）。 */
export interface AgentPromptPayload {
  challengeId: string;
  sessionId: string;
  tool: string;
  command: string;
  risk: AgentRisk;
  /** unix 秒；后端发出时刻。 */
  requestedAt: number;
  timeoutSecs: number;
}

/** `ssh/agent/notice` 事件 payload：低危命令直接注入终端时的告知。 */
export interface AgentNoticePayload {
  sessionId: string;
  tool: string;
  command: string;
  risk: AgentRisk;
}

/** `ssh/agent/finish` 事件 payload：一次终端路由执行收尾。 */
export interface AgentFinishPayload {
  sessionId: string;
  status: AgentFinishStatus;
}

/**
 * 审批弹窗剩余秒数：requestedAt（秒）+ timeoutSecs − now（毫秒→秒），下限 0。
 * 到 0 前端自动收起弹窗（后端超时语义同样是拒绝）。
 */
export function approvalRemainingSecs(
  payload: Pick<AgentPromptPayload, "requestedAt" | "timeoutSecs">,
  nowMs: number,
): number {
  const remaining = payload.requestedAt + payload.timeoutSecs - nowMs / 1000;
  return remaining > 0 ? remaining : 0;
}

/**
 * 审批挑战队列助手：跨会话并发审批排队（后端同会话已串行化，前端弹窗一次只渲染队首）。
 * 均为不可变语义——始终返回新数组，从不修改入参。
 */

/** 按 challengeId 去重追加：已存在则不追加（返回等价浅拷贝），否则追加到队尾。 */
export function enqueueAgentPrompt<Q extends { challengeId: string }>(
  queue: readonly Q[],
  payload: Q,
): Q[] {
  if (queue.some((item) => item.challengeId === payload.challengeId)) return [...queue];
  return [...queue, payload];
}

/** 移除指定 challengeId 的挑战；未命中返回等价浅拷贝。 */
export function dropAgentPrompt<Q extends { challengeId: string }>(
  queue: readonly Q[],
  challengeId: string,
): Q[] {
  return queue.filter((item) => item.challengeId !== challengeId);
}

/** 按 challengeId 查找队列中的挑战。 */
export function findAgentPrompt<Q extends { challengeId: string }>(
  queue: readonly Q[],
  challengeId: string,
): Q | undefined {
  return queue.find((item) => item.challengeId === challengeId);
}

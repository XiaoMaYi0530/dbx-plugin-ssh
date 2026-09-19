// SFTP 传输任务排序（issue #18）：面板里同一张卡"一会儿在上、一会儿在中、
// 一会儿在下"的根因是活跃列表按对象插入序渲染，而插入序来自后端 HashMap
// 迭代序（任意）+ 事件到达序（终态行永久堆积）。这里给出一个稳定、可解释
// 的全序规则：
//   1. 活跃任务（queued/running）在前，按开始时间升序（先开始的排最上）；
//      无开始时间的 live 行用 joinedAt（本工作台首次出现时间）兜底。
//   2. 终态任务（completed/cancelled/failed）排在活跃任务之后，按开始时间
//      降序（与历史区"最新在前"同向）。
//   3. 同键用 taskId 兜底，任何输入都得到同一顺序。
// 零 UI / 零 sidecar 依赖，单测见 transferOrder.spec.ts。

export interface TransferOrderInput {
  taskId: string;
  status: string;
  /** 落盘历史的开始时间（unix ms），活跃 live 行可能缺失。 */
  startedAt?: number;
  /** 本工作台首次见到该任务的时间（unix ms）。 */
  joinedAt?: number;
}

export function isLiveTransferStatus(status: string): boolean {
  return status === "queued" || status === "running";
}

function orderKey(task: TransferOrderInput): number {
  if (typeof task.startedAt === "number" && Number.isFinite(task.startedAt)) return task.startedAt;
  if (typeof task.joinedAt === "number" && Number.isFinite(task.joinedAt)) return task.joinedAt;
  return Number.MAX_SAFE_INTEGER;
}

export function compareTransferTasks(left: TransferOrderInput, right: TransferOrderInput): number {
  const leftLive = isLiveTransferStatus(left.status);
  const rightLive = isLiveTransferStatus(right.status);
  if (leftLive !== rightLive) return leftLive ? -1 : 1;
  const liveFirst = leftLive;
  let result = orderKey(left) - orderKey(right);
  // 活跃区升序（先开始在前）；终态区取反（最新开始在前，与历史区同向）。
  if (!liveFirst) result = -result;
  if (result !== 0) return result;
  if (left.taskId === right.taskId) return 0;
  return left.taskId < right.taskId ? -1 : 1;
}

export function sortTransferTasks<T extends TransferOrderInput>(tasks: readonly T[]): T[] {
  return [...tasks].sort(compareTransferTasks);
}

// GPU / Ascend NPU 监控视图层（IMPL_PLAN Task P1-4，纯函数不连 SSH）。
//
// 类型与 sidecar `backend/src/metrics_gpu.rs` 的 JSON 输出一一对应：
// `ssh/metrics` 文档的 `gpu` / `npu` 两个顶层 section。旧 sidecar 不带这两个
// 键，监控面板整体隐藏该区；`available=false` 时显示弱化的"未检测到"提示。

export interface GpuProcessView {
  uuid: string;
  pid: number;
  /** 进程占用的 GPU 显存（字节；nvidia-smi MiB 列已换算）。 */
  mem: number | null;
  name: string;
}

export interface GpuInfoView {
  index: number;
  /** 稳定设备标识（MIG 场景下也唯一），进程按它归属到卡。 */
  uuid: string;
  name: string;
  driver: string | null;
  /** 摄氏度。 */
  temperature: number | null;
  /** GPU 利用率 %。 */
  utilization: number | null;
  /** 显存控制器利用率 %。 */
  memUtil: number | null;
  totalMem: number | null;
  usedMem: number | null;
  freeMem: number | null;
  /** 板卡功耗 W（`[Not Supported]` 的卡为 null）。 */
  powerDraw: number | null;
  powerLimit: number | null;
  /** 风扇转速 %（被动散热卡为 null）。 */
  fan: number | null;
  /** 性能状态，如 `P0`（满载）/ `P8`（空闲）。 */
  pstate: string | null;
  processes: GpuProcessView[];
}

export interface GpuOverviewView {
  available: boolean;
  gpus: GpuInfoView[];
}

export interface NpuProcessView {
  pid: number;
  name: string;
  /** 进程表的 Device ID 列（NPU 序号）。 */
  npuIndex: number;
  /** 进程占用的设备内存（字节）。 */
  mem: number | null;
}

export interface NpuDeviceView {
  /** `ascend:{npuIndex}:{chipId}`，一卡多芯片时每芯一个设备。 */
  deviceKey: string;
  npuIndex: number;
  chipId: number;
  /** 卡型号，如 `910B4` / `310P3`。 */
  name: string;
  health: string | null;
  /** 板卡功耗 W。 */
  power: number | null;
  /** 芯片温度 °C。 */
  temperature: number | null;
  /** AI Core 利用率 %。 */
  aicore: number | null;
  /** `hbm`（910 系）或 `memory`（310 系 DDR），决定文案。 */
  memoryLabel: string | null;
  usedMem: number | null;
  totalMem: number | null;
  processes: NpuProcessView[];
}

export interface NpuOverviewView {
  available: boolean;
  /** CANN toolkit/nnae/nnrt 版本串，读不到 install.info 时为 null。 */
  cann: string | null;
  devices: NpuDeviceView[];
}

/** used/total 的百分比（0-100 夹紧）；total 缺失或非正时为 null（不画条）。 */
export function memoryPercent(used?: number | null, total?: number | null): number | null {
  if (!total || total <= 0) return null;
  return Math.min(100, Math.max(0, ((used ?? 0) / total) * 100));
}

/** 监控区可见性：任一 accelerator section 键存在才渲染（旧 sidecar 整区隐藏）。 */
export function acceleratorSectionsPresent(
  gpu: GpuOverviewView | undefined,
  npu: NpuOverviewView | undefined,
): boolean {
  return gpu !== undefined || npu !== undefined;
}

import type { BalanceItem, PlanEntryTarget, QuotaInfo } from "./api";
import { isAccountLow } from "./glm52";

/**
 * 帐号内套餐入口判定。
 *
 * ZCode 通过 setting.json 的 modelProviderFamilySelectedKeys[family] 在两个
 * 套餐入口之间选择消耗哪个套餐（本工具实测结论）：
 * - `coding-plan:builtin:<family>-start-plan` → Start/Global Plan（token 桶）；
 * - `coding-plan:builtin:<family>-coding-plan` → GLM Coding Plan（积分桶）。
 *
 * 低额度自动切换的顺序因此是：当前入口余额不足 → 先切到同帐号内有余额的
 * 另一个入口（切套餐）；所有入口都低于阈值 → 才切换帐号；暂停模式则改为
 * 拦截请求等待人工处理。
 */

/** 余额条目归属的套餐入口；不参与判定的条目（其他模型/工具额度）返回 null。 */
export function entryOfBalance(item: BalanceItem): PlanEntryTarget | null {
  if (item.unit_type === "point") return "coding-plan";
  if (item.unit_type === "tool") return null;
  const name = item.show_name.trim().toLowerCase();
  const monitored = ["glm-5.3", "glm-5.3-flash"].some((model) => name.includes(model));
  return monitored ? "start-plan" : null;
}

export interface EntryHealth {
  /** 该入口是否有可判定的余额数据（无数据 = 帐号可能不含该套餐）。 */
  evaluable: boolean;
  /** evaluable 且剩余低于阈值。 */
  low: boolean;
  /** 相对阈值的余量（>1 充足，<=1 危险）；不可评估时为 Infinity。 */
  headroom: number;
}

function balanceRemaining(item: BalanceItem): number {
  if (Number.isFinite(item.remaining_units)) return Math.max(0, item.remaining_units);
  const total = Number.isFinite(item.total_units) ? Math.max(0, item.total_units) : 0;
  const used = Number.isFinite(item.used_units) ? Math.max(0, item.used_units) : 0;
  return Math.max(0, total - used);
}

function entryItems(quota: QuotaInfo | undefined, entry: PlanEntryTarget): BalanceItem[] {
  return (quota?.balances ?? []).filter(
    (item): item is BalanceItem => entryOfBalance(item) === entry
  );
}

/**
 * 单个套餐入口的健康状态：
 * - start-plan 入口（token 量纲）：监控模型（GLM-5.3/5.3-Flash）剩余之和；
 * - coding-plan 入口（积分量纲）：各积分桶剩余的最小值（最紧窗口决定可用性）。
 */
export function entryHealth(
  quota: QuotaInfo | undefined,
  entry: PlanEntryTarget,
  tokenThresholdWan: number,
  pointThreshold: number
): EntryHealth {
  const items = entryItems(quota, entry);
  if (items.length === 0) {
    return { evaluable: false, low: false, headroom: Number.POSITIVE_INFINITY };
  }
  if (entry === "coding-plan") {
    const min = Math.min(...items.map(balanceRemaining));
    return {
      evaluable: true,
      low: min < pointThreshold,
      headroom: pointThreshold > 0 ? min / pointThreshold : Number.POSITIVE_INFINITY,
    };
  }
  const sum = items.reduce((acc, item) => acc + balanceRemaining(item), 0);
  const threshold = tokenThresholdWan * 10_000;
  return {
    evaluable: true,
    low: sum < threshold,
    headroom: threshold > 0 ? sum / threshold : Number.POSITIVE_INFINITY,
  };
}

/** ZCode 当前选中入口；active_provider 缺失或指向非套餐入口（如 apikey）时为 null。 */
export function currentPlanEntry(
  activeProvider: string | null | undefined
): PlanEntryTarget | null {
  const providerId = (activeProvider ?? "").split(":").pop() ?? "";
  if (providerId.includes("-coding-plan")) return "coding-plan";
  if (providerId.includes("-start-plan")) return "start-plan";
  return null;
}

/**
 * 当前入口是否低于阈值。两类情况回退到整账号判定（token 与积分任一
 * 量纲低于阈值即视为低）：
 * 1. 入口无法识别（active_provider 缺失或指向非套餐入口）；
 * 2. 入口可识别但无该套餐的余额数据（evaluable=false），例如账号只
 *    订阅了 Coding Plan、却把当前入口设到了 start-plan。这种情况下
 *    当前入口"没数据=看似充足"会让自动切换失灵，必须回退。
 */
export function isCurrentEntryLow(
  quota: QuotaInfo | undefined,
  tokenThresholdWan: number,
  pointThreshold: number
): boolean {
  const entry = currentPlanEntry(quota?.active_provider);
  if (!entry) return isAccountLow(quota, tokenThresholdWan, pointThreshold);
  const health = entryHealth(quota, entry, tokenThresholdWan, pointThreshold);
  if (!health.evaluable) return isAccountLow(quota, tokenThresholdWan, pointThreshold);
  return health.low;
}

/**
 * 帐号内可切换到的套餐入口：当前入口之外的另一个入口余额充足时返回它；
 * 没有（不可评估或同样低于阈值）返回 null —— 此时才允许切换帐号。
 */
export function pickPlanSwitchTarget(
  quota: QuotaInfo | undefined,
  current: PlanEntryTarget,
  tokenThresholdWan: number,
  pointThreshold: number
): PlanEntryTarget | null {
  const other: PlanEntryTarget = current === "coding-plan" ? "start-plan" : "coding-plan";
  const health = entryHealth(quota, other, tokenThresholdWan, pointThreshold);
  return health.evaluable && !health.low ? other : null;
}

/**
 * 逐一验证候选目标：按给定顺序用最新拉取的余额逐个复核，返回第一个
 * 通过验证的目标；全部不通过（或余额拉取失败）返回 null —— 调用方
 * 必须放弃切换。用于低额度切换前确认"待切换的套餐/帐户确实有余额"，
 * 避免依据过期缓存把请求切到已耗尽的套餐/帐户上。
 *
 * - `ordered`：候选及验证顺序（调用方先按余量等排序）；
 * - `refresh`：拉取该目标的最新余额（抛错视为该目标不可验证 → 不合格）；
 * - `passes`：用最新余额判定该目标是否可切。
 * 每个候选都会调用 refresh（哪怕前面的失败了），直到某个通过为止。
 */
export async function firstVerifiedTarget<T, F>(
  ordered: readonly T[],
  refresh: (item: T) => Promise<F>,
  passes: (item: T, fresh: F | null) => boolean
): Promise<T | null> {
  for (const item of ordered) {
    let fresh: F | null = null;
    try {
      fresh = await refresh(item);
    } catch {
      fresh = null;
    }
    if (passes(item, fresh)) return item;
  }
  return null;
}

/** 供展示/日志用：把入口映射为稳定标识（文案由 i18n 负责）。 */
export function planEntryLabelKey(entry: PlanEntryTarget): "planEntryStart" | "planEntryCoding" {
  return entry === "coding-plan" ? "planEntryCoding" : "planEntryStart";
}

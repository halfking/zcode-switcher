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

/** 单候选验证的默认超时（毫秒）：验证要拉一次真实余额，网络挂起时兜底。 */
export const DEFAULT_VERIFY_TIMEOUT_MS = 15_000;
/** 默认并发上限：余额请求打太快会撞上游限流，3 是延迟与限流之间的折中。 */
export const DEFAULT_VERIFY_CONCURRENCY = 3;

/** 候选验证进度（随验证推进回报给 UI）。 */
export interface VerifyProgress {
  /** 已出结果的候选数（合格 + 不合格） */
  done: number;
  total: number;
  /** 已判定不合格的候选数（余额不足 / 拉取失败 / 超时） */
  failed: number;
  /** 仍在验证中的候选在 `ordered` 里的下标 */
  activeIndexes: number[];
}

export interface VerifyOptions {
  /** 同时验证的候选数上限，最小 1；默认 DEFAULT_VERIFY_CONCURRENCY */
  concurrency?: number;
  /** 单候选验证超时毫秒数，<=0 表示不限时；默认 DEFAULT_VERIFY_TIMEOUT_MS */
  timeoutMs?: number;
  /** 每次有候选开始/出结果时回调一次 */
  onProgress?: (progress: VerifyProgress) => void;
}

function withTimeout<F>(promise: Promise<F>, timeoutMs: number): Promise<F> {
  return new Promise<F>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`verify timeout (${timeoutMs}ms)`)),
      timeoutMs
    );
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      }
    );
  });
}

/**
 * 验证候选目标：用最新拉取的余额复核，返回通过验证的目标中优先级最高
 * （在 `ordered` 里下标最小）的一个；全部不通过（或余额拉取失败/超时）
 * 返回 null —— 调用方必须放弃切换。用于低额度切换前确认"待切换的
 * 套餐/帐户确实有余额"，避免依据过期缓存把请求切到已耗尽的套餐/帐户上。
 *
 * - `ordered`：候选及优先级顺序（调用方先按余量等排序）；
 * - `refresh`：拉取该目标的最新余额（抛错或超时视为该目标不可验证 → 不合格）；
 * - `passes`：用最新余额判定该目标是否可切；
 * - `options.concurrency`：同时在飞的验证数上限；`options.timeoutMs`：单候选
 *   超时；`options.onProgress`：进度回调。
 *
 * 并发不改变选择语义：排在前面的候选未出结果时，即使后面的候选先通过
 * 也要等前面的判定完（前通过则前胜出），所以胜者永远是"有余额的最高
 * 优先级候选"。确定胜者后剩余在飞的验证不再回报进度，其结果被忽略。
 */
export async function firstVerifiedTarget<T, F>(
  ordered: readonly T[],
  refresh: (item: T) => Promise<F>,
  passes: (item: T, fresh: F | null) => boolean,
  options: VerifyOptions = {}
): Promise<T | null> {
  const total = ordered.length;
  if (total === 0) return null;
  const concurrency = Math.max(
    1,
    Math.min(options.concurrency ?? DEFAULT_VERIFY_CONCURRENCY, total)
  );
  const timeoutMs = options.timeoutMs ?? DEFAULT_VERIFY_TIMEOUT_MS;
  const onProgress = options.onProgress;

  // resolved[i] / results[i]：候选 i 是否已出结果及其最新余额（null=拉取失败/超时）
  const resolved: boolean[] = new Array(total).fill(false);
  const results: (F | null)[] = new Array(total).fill(null);
  const active = new Set<number>();
  let next = 0;
  let done = 0;
  let failed = 0;
  let settled = false;

  const emit = () =>
    onProgress?.({
      done,
      total,
      failed,
      activeIndexes: [...active].sort((a, b) => a - b),
    });

  /** 胜者 = 下标最小且已通过验证的候选；前面还有在飞的就必须等它判定。 */
  const settleIfDecided = (resolve: (winner: T | null) => void) => {
    for (let i = 0; i < total; i += 1) {
      if (!resolved[i]) return;
      if (passes(ordered[i], results[i])) {
        settled = true;
        resolve(ordered[i]);
        return;
      }
    }
    settled = true;
    resolve(null);
  };

  return new Promise<T | null>((resolve) => {
    const verifyOne = async (index: number) => {
      let fresh: F | null = null;
      try {
        const pending = refresh(ordered[index]);
        fresh = timeoutMs > 0 ? await withTimeout(pending, timeoutMs) : await pending;
      } catch {
        fresh = null;
      }
      results[index] = fresh;
      resolved[index] = true;
      active.delete(index);
      done += 1;
      if (!passes(ordered[index], fresh)) failed += 1;
      if (settled) return; // 胜者已定：不再回报进度，结果作废
      emit();
      settleIfDecided(resolve);
      if (!settled) launch();
    };

    const launch = () => {
      while (!settled && next < total && active.size < concurrency) {
        const index = next;
        next += 1;
        active.add(index);
        emit();
        void verifyOne(index);
      }
    };

    launch();
  });
}

/** 供展示/日志用：把入口映射为稳定标识（文案由 i18n 负责）。 */
export function planEntryLabelKey(entry: PlanEntryTarget): "planEntryStart" | "planEntryCoding" {
  return entry === "coding-plan" ? "planEntryCoding" : "planEntryStart";
}

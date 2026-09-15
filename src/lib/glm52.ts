import type { BalanceItem, ProfileView, QuotaInfo } from "./api";

// 监听的模型：按 show_name（忽略大小写）子串匹配账号的额度条目。
// "glm-5.3-flash" 本身包含 "glm-5.3"，显式列出以表达两个监听对象。
export const MONITORED_MODELS = ["glm-5.3", "glm-5.3-flash"] as const;

function isMonitoredBalance(item: BalanceItem): boolean {
  // 积分制条目（GLM Coding 个人套餐）与 token 阈值不同量纲，不参与切换判定。
  if (item.unit_type === "point") return false;
  const name = item.show_name.trim().toLowerCase();
  return MONITORED_MODELS.some((model) => name.includes(model));
}

export function monitoredBalances(quota?: QuotaInfo): BalanceItem[] {
  return quota?.balances?.filter(isMonitoredBalance) ?? [];
}

/**
 * 判断额度条目是否属于 ZCode 当前选中的供应者（用于"使用中"标记）。
 *
 * active_provider 形如 "coding-plan:builtin:bigmodel-start-plan"：
 * - 积分条目（unit_type=point，个人套餐）→ 命中 coding-plan 供应者；
 * - token 条目按 show_name 的套餐短名前缀匹配（Start·/Coding·）。
 * Global Build 等桌面端无对应供应者的套餐不参与标记。
 */
export function isBalanceActive(
  item: BalanceItem,
  activeProvider?: string | null
): boolean {
  if (!activeProvider) return false;
  const provider = activeProvider.toLowerCase();
  if (item.unit_type === "point") return provider.includes("coding-plan");
  const name = item.show_name.trim().toLowerCase();
  if (name.startsWith("start·")) return provider.includes("start-plan");
  if (name.startsWith("coding·")) return provider.includes("coding-plan");
  return false;
}

function balanceRemaining(item: BalanceItem): number {
  if (Number.isFinite(item.remaining_units)) return Math.max(0, item.remaining_units);
  const total = Number.isFinite(item.total_units) ? Math.max(0, item.total_units) : 0;
  const used = Number.isFinite(item.used_units) ? Math.max(0, item.used_units) : 0;
  return Math.max(0, total - used);
}

// 剩余额度 = 监听模型各额度条目剩余之和；无任何匹配条目时返回 null。
export function glm52Remaining(quota?: QuotaInfo): number | null {
  const items = monitoredBalances(quota);
  if (items.length === 0) return null;
  return items.reduce((sum, item) => sum + balanceRemaining(item), 0);
}

export function formatQuotaUnits(n: number): string {
  if (!Number.isFinite(n)) return "-";
  const abs = Math.abs(n);
  if (abs >= 1e8) return `${(n / 1e8).toFixed(2)} 亿`;
  if (abs >= 1e4) return `${(n / 1e4).toFixed(2)} 万`;
  return Math.round(n).toLocaleString();
}

// 按剩余额度动态调整监测间隔：越接近切换阈值刷新越快，充裕时放慢轮询。
export const DYNAMIC_REFRESH_MIN_MS = 5_000;
export const DYNAMIC_REFRESH_MID_MS = 20_000;
export const DYNAMIC_REFRESH_MAX_MS = 60_000;

export function dynamicQuotaRefreshIntervalMs(
  remaining: number | null,
  thresholdWan: number
): number {
  if (remaining === null || !Number.isFinite(remaining)) {
    return DYNAMIC_REFRESH_MID_MS;
  }
  const threshold = Math.max(1, thresholdWan) * 10_000;
  if (remaining <= threshold) return DYNAMIC_REFRESH_MIN_MS;
  if (remaining <= threshold * 3) return DYNAMIC_REFRESH_MID_MS;
  return DYNAMIC_REFRESH_MAX_MS;
}

export interface Glm52PoolStats {
  totalAccounts: number;
  usedAccounts: number;
  remainingAccounts: number;
  usedUnits: number;
  totalUnits: number;
}

export function computeGlm52PoolStats(
  profiles: ProfileView[],
  quotas: Record<string, QuotaInfo>,
  thresholdWan: number
): Glm52PoolStats {
  const thresholdUnits = thresholdWan * 10_000;
  let usedAccounts = 0;
  let usedUnits = 0;
  let rawTotalUnits = 0;
  let usedBelowThresholdUnits = 0;

  for (const profile of profiles) {
    const items = monitoredBalances(quotas[profile.id]);
    if (items.length === 0) continue;

    const total = items.reduce(
      (sum, item) => sum + (Number.isFinite(item.total_units) ? Math.max(0, item.total_units) : 0),
      0
    );
    const remaining = items.reduce((sum, item) => sum + balanceRemaining(item), 0);
    const used = items.reduce(
      (sum, item) => sum + (Number.isFinite(item.used_units) ? Math.max(0, item.used_units) : 0),
      0
    );

    rawTotalUnits += total;
    usedUnits += used;

    if (remaining < thresholdUnits) {
      usedAccounts += 1;
      usedBelowThresholdUnits += thresholdUnits - remaining;
    }
  }

  const totalUnits = Math.max(
    usedUnits,
    rawTotalUnits - profiles.length * thresholdUnits + usedBelowThresholdUnits
  );

  return {
    totalAccounts: profiles.length,
    usedAccounts,
    remainingAccounts: Math.max(0, profiles.length - usedAccounts),
    usedUnits,
    totalUnits,
  };
}

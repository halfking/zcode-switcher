import type { BalanceItem, ProfileView, QuotaInfo } from "./api";

/**
 * 额度守护：监测 GLM-5.3 / GLM-5.3-Flash 的 5 小时或周余额。
 * 最低剩余百分比低于阈值时，本地网关暂停转发所有请求，保护账号。
 */
export const GUARD_THRESHOLD_PERCENT = 2;

/** 守护桶：period 为 5h/weekly，或无周期但模型名含 glm-5.3（含 Flash）。 */
export function isGuardBucket(item: BalanceItem): boolean {
  const period = item.period?.trim().toLowerCase();
  if (period) return period === "5h" || period === "weekly";
  return item.show_name.trim().toLowerCase().includes("glm-5.3");
}

function bucketPercent(item: BalanceItem): number | null {
  const total = Number.isFinite(item.total_units) ? Math.max(0, item.total_units) : 0;
  if (total <= 0) return null;
  const remaining = Number.isFinite(item.remaining_units)
    ? Math.max(0, item.remaining_units)
    : Math.max(0, total - (Number.isFinite(item.used_units) ? item.used_units : 0));
  return Math.min(100, Math.max(0, (remaining / total) * 100));
}

/** 受监控桶中的最低剩余百分比；无可监控桶时返回 null。 */
export function minGuardPercent(quota?: QuotaInfo): number | null {
  let min: number | null = null;
  for (const item of quota?.balances ?? []) {
    if (!isGuardBucket(item)) continue;
    const percent = bucketPercent(item);
    if (percent === null) continue;
    if (min === null || percent < min) min = percent;
  }
  return min;
}

/**
 * 自适应检测频度：低于阈值（已暂停）或临近阈值时加快到 1 分钟，
 * 轻度关注时 3 分钟，充裕时放宽到 10 分钟；无数据时尽快补测。
 */
export function guardIntervalMs(percent: number | null | undefined): number {
  if (percent === null || percent === undefined) return 60_000;
  if (percent < 5) return 60_000;
  if (percent < 15) return 3 * 60_000;
  return 10 * 60_000;
}

export function formatQuotaUnits(n: number): string {
  if (!Number.isFinite(n)) return "-";
  const abs = Math.abs(n);
  if (abs >= 1e8) return `${(n / 1e8).toFixed(2)} 亿`;
  if (abs >= 1e4) return `${(n / 1e4).toFixed(2)} 万`;
  return Math.round(n).toLocaleString();
}

export interface GuardPoolStats {
  totalAccounts: number;
  /** 5h/周最低余额已低于守护阈值的账号数 */
  exhaustedAccounts: number;
  remainingAccounts: number;
  usedUnits: number;
  totalUnits: number;
}

export function computeGuardPoolStats(
  profiles: ProfileView[],
  quotas: Record<string, QuotaInfo>
): GuardPoolStats {
  let exhaustedAccounts = 0;
  let usedUnits = 0;
  let totalUnits = 0;

  for (const profile of profiles) {
    const quota = quotas[profile.id];
    const buckets = (quota?.balances ?? []).filter(isGuardBucket);
    if (buckets.length === 0) continue;

    let bucketTotal = 0;
    let bucketUsed = 0;
    let minPercent: number | null = null;
    for (const item of buckets) {
      const total = Number.isFinite(item.total_units) ? Math.max(0, item.total_units) : 0;
      const remaining = Number.isFinite(item.remaining_units)
        ? Math.max(0, item.remaining_units)
        : Math.max(0, total - (Number.isFinite(item.used_units) ? item.used_units : 0));
      const used = Number.isFinite(item.used_units)
        ? Math.max(0, item.used_units)
        : Math.max(0, total - remaining);
      bucketTotal += total;
      bucketUsed += used;
      const percent = bucketPercent(item);
      if (percent !== null && (minPercent === null || percent < minPercent)) {
        minPercent = percent;
      }
    }
    usedUnits += bucketUsed;
    totalUnits += bucketTotal;
    if (minPercent !== null && minPercent < GUARD_THRESHOLD_PERCENT) {
      exhaustedAccounts += 1;
    }
  }

  return {
    totalAccounts: profiles.length,
    exhaustedAccounts,
    remainingAccounts: Math.max(0, profiles.length - exhaustedAccounts),
    usedUnits,
    totalUnits,
  };
}

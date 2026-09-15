import type { BalanceItem, PlanSummary, ProfileView, QuotaInfo } from "./api";

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
 * active_provider（selectedKey）形如 "coding-plan:builtin:bigmodel-start-plan"：
 * 前缀 "coding-plan:" 是 key 类型，真正的供应者在最后一段，必须取最后一段
 * 匹配，否则 start-plan 的 key 会被误判成 coding-plan。
 */
export function isBalanceActive(
  item: BalanceItem,
  activeProvider?: string | null
): boolean {
  if (!activeProvider) return false;
  const providerId = activeProvider.split(":").pop() ?? "";
  if (!providerId) return false;
  if (item.unit_type === "point") {
    return providerId.includes("bigmodel-coding-plan");
  }
  const name = item.show_name.trim().toLowerCase();
  if (name.startsWith("start·")) return providerId.includes("bigmodel-start-plan");
  if (name.startsWith("coding·")) return providerId.includes("bigmodel-coding-plan");
  return false;
}

/**
 * 判断套餐是否为当前选中的供应者对应的套餐（与后端 plan_is_current 同语义）。
 * 后端已填 is_current；此函数用于旧缓存等后端字段缺失时的兜底判断。
 *
 * 用套餐名匹配而非 plan_id：Global Build 的 plan_id 也挂在 start-plan
 * 体系下（zcode-v3-start-plan-0914），按 id 匹配会把 Global Build 误标。
 */
export function isPlanCurrent(
  plan: PlanSummary,
  activeProvider?: string | null
): boolean {
  if (!activeProvider) return false;
  const providerId = activeProvider.split(":").pop() ?? "";
  if (!providerId) return false;
  const name = plan.name.toLowerCase();
  const planId = (plan.plan_id ?? "").toLowerCase();
  if (providerId.includes("bigmodel-start-plan")) {
    if (name) return name.includes("start") && !name.includes("global");
    return planId.includes("start-plan") && !planId.includes("global");
  }
  if (providerId.includes("bigmodel-coding-plan")) {
    if (name) return name.includes("coding");
    return planId.startsWith("personal:") || planId.includes("coding");
  }
  return false;
}

export interface PlanBalanceGroup {
  plan: PlanSummary | null;
  items: BalanceItem[];
}

/** 去掉余额条目名里的套餐短名前缀（组头已显示套餐名，条目名无需重复）。 */
export function stripPlanPrefix(name: string): string {
  return name.replace(/^(global|start|coding|team)·/i, "");
}

/**
 * 把余额条目按套餐分组：plans[] 顺序为准，条目按 plan_id 归组；
 * 没有 plan_id 或不在 plans 列表的条目归入末尾的杂项目（plan=null）。
 * 用于展开视图的"套餐 → 余额"分组展示。
 */
export function groupBalancesByPlan(quota: QuotaInfo): PlanBalanceGroup[] {
  const groups: PlanBalanceGroup[] = [];
  const byPlanId = new Map<string, PlanBalanceGroup>();
  for (const plan of quota.plans ?? []) {
    const group: PlanBalanceGroup = { plan, items: [] };
    groups.push(group);
    if (plan.plan_id) byPlanId.set(plan.plan_id, group);
  }
  const misc: PlanBalanceGroup = { plan: null, items: [] };
  for (const item of quota.balances ?? []) {
    const group = item.plan_id ? byPlanId.get(item.plan_id) : undefined;
    (group ?? misc).items.push(item);
  }
  if (misc.items.length > 0) groups.push(misc);
  // 空套餐组（无任何条目）也保留：让用户看到套餐存在但服务端暂无额度明细。
  return groups;
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

// 积分条目（GLM Coding 个人套餐的 5h 积分、周积分等，unit_type === "point"）。
export function pointBalances(quota?: QuotaInfo): BalanceItem[] {
  return quota?.balances?.filter((item) => item.unit_type === "point") ?? [];
}

/**
 * 账号可用积分 = 各积分桶剩余的最小值（最紧的桶决定账号可用性：
 * 任一窗口的积分耗尽都会被限流），无积分条目时返回 null。
 */
export function glmPointRemaining(quota?: QuotaInfo): number | null {
  const items = pointBalances(quota);
  if (items.length === 0) return null;
  return Math.min(...items.map(balanceRemaining));
}

/**
 * 当前账号是否应触发切换：token 与积分任一量纲跌破对应阈值即触发。
 * 条目缺失的量纲不参与判定（该账号可能根本不含此量纲的套餐）。
 */
export function isAccountLow(
  quota: QuotaInfo | undefined,
  tokenThresholdWan: number,
  pointThreshold: number
): boolean {
  const token = glm52Remaining(quota);
  if (token !== null && token < tokenThresholdWan * 10_000) return true;
  const point = glmPointRemaining(quota);
  if (point !== null && point < pointThreshold) return true;
  return false;
}

/**
 * 候选账号判定：账号已知的每个量纲都必须高于阈值，且至少有一个量纲
 * 有数据（排除状态未知的账号）。低于阈值的账号永远不会成为切换目标，
 * 这保证不会在耗尽的账号之间来回循环切换。
 */
export function isSwitchableCandidate(
  quota: QuotaInfo | undefined,
  tokenThresholdWan: number,
  pointThreshold: number
): boolean {
  if (!quota || quota.error) return false;
  const token = glm52Remaining(quota);
  const point = glmPointRemaining(quota);
  if (token === null && point === null) return false;
  if (token !== null && token <= tokenThresholdWan * 10_000) return false;
  if (point !== null && point <= pointThreshold) return false;
  return true;
}

/**
 * 各量纲相对阈值的余量（取最小者），用于候选排序：余量最大的账号
 * 最“安全”，切过去之后最不容易立刻再次触发切换。
 */
export function accountHeadroom(
  quota: QuotaInfo | undefined,
  tokenThresholdWan: number,
  pointThreshold: number
): number {
  let headroom = Number.POSITIVE_INFINITY;
  const token = glm52Remaining(quota);
  if (token !== null && tokenThresholdWan > 0) {
    headroom = Math.min(headroom, token / (tokenThresholdWan * 10_000));
  }
  const point = glmPointRemaining(quota);
  if (point !== null && pointThreshold > 0) {
    headroom = Math.min(headroom, point / pointThreshold);
  }
  return headroom;
}

export function formatQuotaUnits(n: number): string {
  if (!Number.isFinite(n)) return "-";
  const abs = Math.abs(n);
  if (abs >= 1e8) return `${(n / 1e8).toFixed(2)} 亿`;
  if (abs >= 1e4) return `${(n / 1e4).toFixed(2)} 万`;
  return Math.round(n).toLocaleString();
}

// 按剩余额度动态调整监测间隔：越接近切换阈值刷新越快，充裕时放慢轮询。
// token 与积分取"相对阈值余量"更紧的一端定档；已自动暂停时回到慢速档。
export const DYNAMIC_REFRESH_MIN_MS = 5_000;
export const DYNAMIC_REFRESH_MID_MS = 20_000;
export const DYNAMIC_REFRESH_MAX_MS = 60_000;

export function dynamicQuotaRefreshIntervalMs(
  quota: QuotaInfo | undefined,
  tokenThresholdWan: number,
  pointThreshold: number,
  paused: boolean
): number {
  if (paused) return DYNAMIC_REFRESH_MAX_MS;
  const headroom = accountHeadroom(quota, tokenThresholdWan, pointThreshold);
  if (!Number.isFinite(headroom)) return DYNAMIC_REFRESH_MID_MS;
  if (headroom <= 1) return DYNAMIC_REFRESH_MIN_MS;
  if (headroom <= 3) return DYNAMIC_REFRESH_MID_MS;
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
  thresholdWan: number,
  pointThreshold = 0
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

    // token 或积分任一量纲低于阈值即计入"已用完"账号。
    if (remaining < thresholdUnits || isAccountLow(quotas[profile.id], thresholdWan, pointThreshold)) {
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

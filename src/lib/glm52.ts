import type { BalanceItem, PlanSummary, ProfileView, QuotaInfo, ResetCardType } from "./api";

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

// 百分比 token 条目（quota/limit 的 TOKENS_LIMIT 桶，unit_type === "percentage"，
// 如"周Token"）：只有已用百分比、无绝对值，剩余以 0-100 计。
export function percentBalances(quota?: QuotaInfo): BalanceItem[] {
  return quota?.balances?.filter((item) => item.unit_type === "percentage") ?? [];
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
 * 百分比 token 维度的剩余（%）：各窗口剩余的最小值（最紧的窗口决定
 * 可用性，与积分桶同语义），无百分比条目时返回 null。
 */
export function glmPercentRemaining(quota?: QuotaInfo): number | null {
  const items = percentBalances(quota);
  if (items.length === 0) return null;
  return Math.min(...items.map(balanceRemaining));
}

/**
 * 百分比 token 维度的切换阈值（%）：剩余百分比低于该值触发切换。
 * 百分比套餐（quota/limit 只有 TOKENS_LIMIT 的账号）此前完全无法参与
 * 守护判定，该阈值让这类账号与 token/积分量纲同规则切换。
 */
export const DEFAULT_PERCENT_THRESHOLD = 10;
export const PERCENT_THRESHOLD_MIN = 1;
export const PERCENT_THRESHOLD_MAX = 50;

/**
 * 当前账号是否应触发切换：token、积分、百分比任一量纲跌破对应阈值即
 * 触发。条目缺失的量纲不参与判定（该账号可能根本不含此量纲的套餐）。
 */
export function isAccountLow(
  quota: QuotaInfo | undefined,
  tokenThresholdWan: number,
  pointThreshold: number,
  percentThreshold: number = DEFAULT_PERCENT_THRESHOLD
): boolean {
  const token = glm52Remaining(quota);
  if (token !== null && token < tokenThresholdWan * 10_000) return true;
  const point = glmPointRemaining(quota);
  if (point !== null && point < pointThreshold) return true;
  const percent = glmPercentRemaining(quota);
  if (percent !== null && percent < percentThreshold) return true;
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
  pointThreshold: number,
  percentThreshold: number = DEFAULT_PERCENT_THRESHOLD
): boolean {
  if (!quota || quota.error) return false;
  const token = glm52Remaining(quota);
  const point = glmPointRemaining(quota);
  const percent = glmPercentRemaining(quota);
  if (token === null && point === null && percent === null) return false;
  if (token !== null && token <= tokenThresholdWan * 10_000) return false;
  if (point !== null && point <= pointThreshold) return false;
  if (percent !== null && percent <= percentThreshold) return false;
  return true;
}

/**
 * 各量纲相对阈值的余量（取最小者），用于候选排序：余量最大的账号
 * 最“安全”，切过去之后最不容易立刻再次触发切换。
 */
export function accountHeadroom(
  quota: QuotaInfo | undefined,
  tokenThresholdWan: number,
  pointThreshold: number,
  percentThreshold: number = DEFAULT_PERCENT_THRESHOLD
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
  const percent = glmPercentRemaining(quota);
  if (percent !== null && percentThreshold > 0) {
    headroom = Math.min(headroom, percent / percentThreshold);
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

// ---------------------------------------------------------------------------
// 重置卡决策：用量见底（默认 ≤1%）时先用卡回满窗口，赶在窗口真正打尽、
// 请求被拒导致业务中断之前；选卡看周余额（默认 ≤5% 或用尽用周卡，因为
// 周卡同时回满 5 小时窗口；否则用 5 小时卡）。
// ---------------------------------------------------------------------------

/** 指定周期（"5h"/"weekly"/"monthly"）积分窗口的剩余百分比（0-100）；
 * 该账号没有此窗口的积分桶时返回 null。 */
export function pointWindowPercent(
  quota: QuotaInfo | undefined,
  period: "5h" | "weekly" | "monthly"
): number | null {
  for (const item of quota?.balances ?? []) {
    if (item.unit_type !== "point" || item.period !== period) continue;
    const total = Number.isFinite(item.total_units) ? item.total_units : 0;
    if (total <= 0) return 0;
    const remaining = balanceRemaining(item);
    return Math.min(100, Math.max(0, (remaining / total) * 100));
  }
  return null;
}

export interface ResetCardDecision {
  type: ResetCardType;
}

/**
 * 是否应该使用重置卡、用哪张。纯规则（阈值可配，百分比按窗口总量计）：
 *
 * 1. 触发：5 小时或周窗口任一剩余百分比 ≤ triggerPercent（默认 1）。
 *    赶在窗口打尽之前用卡，避免请求被拒中断业务；两个窗口都健康时
 *    绝不动卡（不浪费）。
 * 2. 排除：月积分窗口存在且同样见底时不用卡——重置卡只回满 5 小时与
 *    周窗口，月窗口打尽时用卡救不回来，留给常规切换/人工处理。
 * 3. 选卡：周余额 ≤ weeklyPercent（默认 5）或账号根本没有健康的周窗口
 *    量纲 → 用周卡（同时回满 5 小时，官方语义）；周窗口充足只是 5 小时
 *    见底 → 用 5 小时卡（把"两层用"的周卡留给真正需要的时候）。
 * 4. 没有任何积分窗口数据（未订阅/数据缺失）→ 不触发。
 */
export function decideResetCard(
  quota: QuotaInfo | undefined,
  triggerPercent: number,
  weeklyPercent: number
): ResetCardDecision | null {
  const p5 = pointWindowPercent(quota, "5h");
  const pw = pointWindowPercent(quota, "weekly");
  const pm = pointWindowPercent(quota, "monthly");
  if (p5 === null && pw === null) return null;
  // 月窗口见底：重置卡无能为力。
  if (pm !== null && pm <= triggerPercent) return null;
  const windows = [p5, pw].filter((v): v is number => v !== null);
  if (Math.min(...windows) > triggerPercent) return null;
  if (pw !== null && pw <= weeklyPercent) return { type: "WEEK" };
  return { type: "FIVE_HOUR" };
}

// ---------------------------------------------------------------------------
// 重置窗口（5 小时 / 周 / 月）：卡片计数与到点自动重置的判定辅助。
// ---------------------------------------------------------------------------

/** 按重置窗口统计的积分卡片数量（5h / 周 / 月各几张）。 */
export interface WindowCardCounts {
  fiveHour: number;
  weekly: number;
  monthly: number;
}

/**
 * 统计账号池里各重置窗口的积分卡片数量（unit_type=point 的额度桶，
 * 即 5小时积分 / 周积分 / 月积分卡）。token 桶与工具额度不参与计数。
 */
export function countWindowCards(quotas: Record<string, QuotaInfo>): WindowCardCounts {
  const counts = { fiveHour: 0, weekly: 0, monthly: 0 };
  for (const quota of Object.values(quotas)) {
    for (const item of quota?.balances ?? []) {
      if (item.unit_type !== "point") continue;
      if (item.period === "5h") counts.fiveHour += 1;
      else if (item.period === "weekly") counts.weekly += 1;
      else if (item.period === "monthly") counts.monthly += 1;
    }
  }
  return counts;
}

/** 一个尚未反映到缓存数据的窗口重置（缓存拉取时间早于重置时刻）。 */
export interface PendingWindowReset {
  profileId: string;
  /** 重置时刻（Unix 秒） */
  resetAt: number;
}

/**
 * 找出"到点后需要刷新"的窗口重置：额度桶带 next_reset_at，且缓存数据的
 * 拉取时间（fetched_at）早于该时刻 —— 说明重置发生时界面还是旧窗口，
 * 到点后刷新该账号即可让卡片剩余额与耗尽状态自动复位。
 * fetched_at >= reset_at 的桶：本次拉取已包含重置后的新窗口，无需处理。
 */
export function pendingWindowResets(
  quotas: Record<string, QuotaInfo>,
  nowSec: number
): PendingWindowReset[] {
  const out: PendingWindowReset[] = [];
  for (const [profileId, quota] of Object.entries(quotas)) {
    if (!quota || quota.error) continue;
    const fetchedAt = quota.fetched_at ?? 0;
    for (const item of quota.balances ?? []) {
      const resetAt = item.next_reset_at ?? 0;
      // 已过期很久仍没刷出来（>7 天）多半是上游时间戳异常，交给常规轮询。
      if (resetAt <= 0 || resetAt < nowSec - 7 * 86400) continue;
      if (fetchedAt >= resetAt) continue;
      out.push({ profileId, resetAt });
    }
  }
  return out;
}

/**
 * 收集"重置已过点且尚未反映到缓存"的账号 id（去重；同一账号多个桶
 * 到点只刷新一次）。settleMs 是过点后的等待余量：给服务端翻转窗口
 * 留时间，避免到点瞬间拉到的还是旧窗口数据。
 */
export function dueWindowResetProfileIds(
  quotas: Record<string, QuotaInfo>,
  nowMs: number,
  settleMs: number
): string[] {
  const ids = new Set<string>();
  for (const p of pendingWindowResets(quotas, nowMs / 1000)) {
    if (p.resetAt * 1000 + settleMs <= nowMs) ids.add(p.profileId);
  }
  return [...ids];
}

/**
 * 距重置时刻的剩余时长（秒）→ 短文案。只做展示，粒度到分；
 * 超过 48 小时以"天+小时"表达。expired<=0 返回空串（不展示）。
 */
export function formatResetCountdown(secondsLeft: number): string {
  if (!Number.isFinite(secondsLeft) || secondsLeft <= 0) return "";
  const totalMin = Math.round(secondsLeft / 60);
  if (totalMin < 1) return "<1m";
  const days = Math.floor(totalMin / 1440);
  const hours = Math.floor((totalMin % 1440) / 60);
  const minutes = totalMin % 60;
  if (days > 0) return `${days}d${hours}h`;
  if (hours > 0) return `${hours}h${minutes}m`;
  return `${minutes}m`;
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
  paused: boolean,
  percentThreshold: number = DEFAULT_PERCENT_THRESHOLD
): number {
  if (paused) return DYNAMIC_REFRESH_MAX_MS;
  const headroom = accountHeadroom(
    quota,
    tokenThresholdWan,
    pointThreshold,
    percentThreshold
  );
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
  pointThreshold = 0,
  percentThreshold: number = DEFAULT_PERCENT_THRESHOLD
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
    if (
      remaining < thresholdUnits ||
      isAccountLow(quotas[profile.id], thresholdWan, pointThreshold, percentThreshold)
    ) {
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

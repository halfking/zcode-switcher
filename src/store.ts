import { create } from "zustand";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  api,
  type ApiFormat,
  type AccountPoolEntryView,
  type CustomProviderView,
  type PlanEntryTarget,
  type ProfileView,
  type ProxyStatus,
  type QuotaInfo,
} from "./lib/api";
import {
  accountHeadroom,
  glm52Remaining,
  isSwitchableCandidate,
} from "./lib/glm52";
import {
  currentPlanEntry,
  DEFAULT_VERIFY_CONCURRENCY,
  DEFAULT_VERIFY_TIMEOUT_MS,
  firstVerifiedTarget,
  isCurrentEntryLow,
  pickPlanSwitchTarget,
  planEntryLabelKey,
} from "./lib/quotaGuard";
import { primaryQuotaRemaining } from "./lib/quotaSort";
import { getTexts, type Language } from "./i18n";

export type ToastKind = "info" | "success" | "error" | "warn";
export type Theme = "dark" | "light";
export type AccountViewMode = "card" | "list";
export type AccountSortMode =
  | "name-asc"
  | "name-desc"
  | "quota-desc"
  | "quota-asc"
  | "expiry-asc";
/**
 * 低额度时的动作方式：
 * - "switch"：帐号内先切套餐（Start Plan 入口 ↔ Coding Plan 入口），
 *   所有套餐都低于阈值后才切换帐号；
 * - "pause"：低于阈值时拦截模型请求并暂停一切自动切换，等待人工处理。
 */
export type LowQuotaAction = "switch" | "pause";
const DEFAULT_ACCOUNT_SORT_MODE: AccountSortMode = "name-asc";

const FLOATING_WINDOW_STORAGE_KEY = "zcs:floatingWindowMode";

const GLM52_THRESHOLD_WAN_MIN = 10;
const GLM52_THRESHOLD_WAN_MAX = 500;
const GLM52_DEFAULT_THRESHOLD_WAN = 200;
// 积分阈值（GLM Coding 个人套餐的积分桶，如 5h 积分 2000/周积分 1 万）。
const GLM52_POINT_THRESHOLD_MIN = 10;
const GLM52_POINT_THRESHOLD_MAX = 5000;
const GLM52_DEFAULT_POINT_THRESHOLD = 200;
const DEFAULT_LOW_QUOTA_ACTION: LowQuotaAction = "switch";
// 帐号内套餐切换的防抖：成功后 60 秒内不重复切换，失败后 10 分钟再重试。
const PLAN_SWITCH_COOLDOWN_MS = 60_000;
const PLAN_SWITCH_RETRY_COOLDOWN_MS = 10 * 60_000;
const DEFAULT_QUOTA_REFRESH_INTERVAL_MINUTES = 10;
const DEFAULT_ACTIVE_QUOTA_REFRESH_INTERVAL_MINUTES = 1;
const ACTIVE_QUOTA_REFRESH_INTERVAL_MINUTES_KEY = "zcs:activeQuotaRefreshIntervalMinutes";
const DEFAULT_THEME: Theme = "light";
const QUOTA_CACHE_KEY = "zcs:quotas";
const DEFAULT_PROXY_PORT = 17860;
const PROXY_PORT_KEY = "zcs:proxyPort";
const PROXY_GATEWAY_KEY = "zcs:proxyGatewayKey";

interface ToastMsg {
  id: number;
  text: string;
  kind: ToastKind;
}

/**
 * 低额度切换"切账号"阶段的候选验证进度（UI 展示；会话内状态）。
 * `active` 为正在验证中的账号名（并发上限内的在飞候选）。
 */
export interface SwitchVerifyProgress {
  total: number;
  done: number;
  failed: number;
  active: string[];
}

interface AppState {
  profiles: ProfileView[];
  /** 账号 id → 额度信息 */
  quotas: Record<string, QuotaInfo>;
  /** 正在拉取额度的账号 id */
  loadingQuota: Record<string, boolean>;
  /** 刚刷新成功的账号 id，1 秒后自动清除 */
  recentlyRefreshed: Record<string, boolean>;
  autoRefreshQuota: boolean;
  /** 自动刷新额度的间隔分钟数，0 表示关闭 */
  quotaRefreshIntervalMinutes: number;
  /** 当前账号刷新间隔分钟数，限制 1-5 分钟 */
  activeQuotaRefreshIntervalMinutes: number;
  /** 用来重置定时器倒计时的 tick */
  scheduledRefreshSeq: number;
  glm52AutoSwitchEnabled: boolean;
  /** 自动切换阈值（模型 token 剩余），单位：万 */
  glm52AutoSwitchThresholdWan: number;
  /** 自动切换阈值（账号积分剩余），单位：积分；各积分桶取最小值参与判定 */
  glm52AutoSwitchPointThreshold: number;
  /** 低额度时的动作方式（见 LowQuotaAction 注释） */
  glm52LowQuotaAction: LowQuotaAction;
  /**
   * 无可切换账号时的自动暂停标记（会话内状态，不持久化）：
   * 暂停期间不做任何切换尝试（监测回落到慢速档），防止在低额度
   * 账号之间循环切换；当前账号恢复、出现满足双阈值的候选账号或
   * 重新开启开关时自动解除。
   */
  autoSwitchPaused: boolean;
  /**
   * 切账号阶段的候选余额验证进度；null = 不在验证中（会话内状态，不持久化）。
   */
  switchVerifyProgress: SwitchVerifyProgress | null;
  /**
   * 候选余额验证的并发上限与单候选超时（非用户配置、不持久化）：
   * 生产默认 3 并发 / 15 秒；回归脚本按场景收紧以便注入挂死/慢速候选。
   */
  candidateVerifyConcurrency: number;
  candidateVerifyTimeoutMs: number;
  autoRestart: boolean;
  tryNoRestartSwitch: boolean;
  theme: Theme;
  floatingWindowMode: boolean;
  /** 悬浮窗缩放比例，1 为原始大小 */
  floatingWindowScale: number;
  updateAvailable: boolean;
  customProviders: CustomProviderView[];
  accountPool: AccountPoolEntryView[];
  proxyStatus: ProxyStatus | null;
  proxyPort: number;
  proxyGatewayKey: string;
  accountViewMode: AccountViewMode;
  /** 账号列表排序方式（当前账号始终置顶） */
  accountSortMode: AccountSortMode;
  hideAccountIdentity: boolean;
  language: Language;
  loading: boolean;
  busy: boolean;
  toasts: ToastMsg[];

  refresh: (silent?: boolean, skipQuota?: boolean) => Promise<void>;
  captureCurrent: (name: string) => Promise<boolean>;
  switchTo: (id: string) => Promise<boolean>;
  renameProfile: (id: string, name: string) => Promise<boolean>;
  deleteProfile: (id: string) => Promise<boolean>;
  deleteProfiles: (ids: string[]) => Promise<{ deleted: number; failed: number }>;

  refreshQuota: (id: string) => Promise<void>;
  refreshAllQuota: (orderedIds?: string[]) => Promise<void>;
  /** 只刷新还没有额度数据的账号 */
  refreshMissingQuota: () => Promise<void>;
  /** 定时触发的批量刷新 */
  scheduledRefreshAllQuota: (orderedIds?: string[]) => Promise<void>;
  refreshActiveQuotaForAutoSwitch: () => Promise<void>;
  setAutoRefreshQuota: (v: boolean) => void;
  setQuotaRefreshIntervalMinutes: (v: number) => void;
  setActiveQuotaRefreshIntervalMinutes: (v: number) => void;
  setGlm52AutoSwitchEnabled: (v: boolean) => void;
  setGlm52AutoSwitchThresholdWan: (v: number) => void;
  setGlm52AutoSwitchPointThreshold: (v: number) => void;
  setGlm52LowQuotaAction: (v: LowQuotaAction) => void;
  setAutoSwitchPaused: (v: boolean) => void;
  setSwitchVerifyProgress: (p: SwitchVerifyProgress | null) => void;
  setAutoRestart: (v: boolean) => void;
  setTryNoRestartSwitch: (v: boolean) => void;
  setTheme: (v: Theme) => void;
  setFloatingWindowMode: (v: boolean) => void;
  setFloatingWindowScale: (v: number) => void;
  setUpdateAvailable: (v: boolean) => void;
  refreshCustomProviders: () => Promise<void>;
  refreshAccountPool: () => Promise<void>;
  addAccountToPool: (profileId: string) => Promise<boolean>;
  setAccountPoolEnabled: (profileId: string, enabled: boolean) => Promise<boolean>;
  removeAccountFromPool: (profileId: string) => Promise<boolean>;
  addCustomProvider: (
    name: string,
    baseUrl: string,
    apiKey: string,
    apiFormat: ApiFormat,
    models: string[]
  ) => Promise<boolean>;
  deleteCustomProvider: (id: string) => Promise<boolean>;
  setCustomProviderEnabled: (id: string, enabled: boolean) => Promise<boolean>;
  activateCustomProvider: (id: string) => Promise<boolean>;
  refreshProxyStatus: () => Promise<void>;
  startLocalProxy: () => Promise<boolean>;
  stopLocalProxy: () => Promise<boolean>;
  setProxyPort: (v: number) => void;
  regenerateProxyGatewayKey: () => void;
  setAccountViewMode: (v: AccountViewMode) => void;
  setAccountSortMode: (v: AccountSortMode) => void;
  setHideAccountIdentity: (v: boolean) => void;
  setLanguage: (v: Language) => void;
  restartZcode: () => Promise<boolean>;

  toast: (text: string, kind?: ToastKind) => void;
  dismissToast: (id: number) => void;
}

let toastSeq = 1;
let glm52AutoSwitching = false;
// 帐号内套餐切换的冷却记账（跨刷新周期防抖）。
let lastPlanSwitchAttemptAt = 0;
let lastPlanSwitchFailed = false;
// 暂停模式的提示只发一次；额度恢复或人工处理后再置位。
let guardPauseNotified = false;

let refreshAllInFlight = false;

function orderedProfilesForRefresh(
  state: AppState,
  orderedIds?: string[]
): ProfileView[] {
  if (orderedIds?.length) {
    return orderedIds
      .map((id) => state.profiles.find((profile) => profile.id === id))
      .filter((profile): profile is ProfileView => !!profile);
  }
  const sorted = [...state.profiles];
  sorted.sort((a, b) => {
    if (a.active !== b.active) return a.active ? -1 : 1;
    switch (state.accountSortMode) {
      case "name-asc":
        return a.name.localeCompare(b.name, undefined, { numeric: true });
      case "name-desc":
        return b.name.localeCompare(a.name, undefined, { numeric: true });
      case "quota-desc":
        return (primaryQuotaRemaining(state.quotas[b.id]) ?? -1) -
          (primaryQuotaRemaining(state.quotas[a.id]) ?? -1);
      case "quota-asc":
        return (primaryQuotaRemaining(state.quotas[a.id]) ?? Number.MAX_SAFE_INTEGER) -
          (primaryQuotaRemaining(state.quotas[b.id]) ?? Number.MAX_SAFE_INTEGER);
      case "expiry-asc":
        return (state.quotas[a.id]?.plan_ends_at ?? Number.MAX_SAFE_INTEGER) -
          (state.quotas[b.id]?.plan_ends_at ?? Number.MAX_SAFE_INTEGER);
    }
  });
  return sorted;
}

function isQuotaInfo(value: unknown): value is QuotaInfo {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  return Array.isArray((value as Partial<QuotaInfo>).balances);
}

function loadCachedQuotas(): Record<string, QuotaInfo> {
  try {
    const raw = localStorage.getItem(QUOTA_CACHE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const quotas: Record<string, QuotaInfo> = {};
    for (const [id, value] of Object.entries(parsed as Record<string, unknown>)) {
      if (id.trim() && isQuotaInfo(value)) {
        quotas[id] = value;
      }
    }
    return quotas;
  } catch {
    return {};
  }
}

function saveCachedQuotas(quotas: Record<string, QuotaInfo>) {
  try {
    localStorage.setItem(QUOTA_CACHE_KEY, JSON.stringify(quotas));
  } catch {
    /* ignore */
  }
}

/** 从 localStorage 读取设置 */
function loadAutoRefresh(): boolean {
  try {
    return localStorage.getItem("zcs:autoRefreshQuota") !== "0";
  } catch {
    return true;
  }
}
function loadAutoRestart(): boolean {
  try {
    return localStorage.getItem("zcs:autoRestart") === "1";
  } catch {
    return false;
  }
}
function loadTryNoRestartSwitch(): boolean {
  try {
    return localStorage.getItem("zcs:tryNoRestartSwitch") === "1";
  } catch {
    return false;
  }
}
function loadQuotaRefreshIntervalMinutes(): number {
  try {
    const saved = localStorage.getItem("zcs:quotaRefreshIntervalMinutes");
    if (saved === null) return DEFAULT_QUOTA_REFRESH_INTERVAL_MINUTES;
    const n = Number(saved);
    return Number.isFinite(n) && n > 0 ? Math.min(1440, Math.round(n)) : 0;
  } catch {
    return DEFAULT_QUOTA_REFRESH_INTERVAL_MINUTES;
  }
}
function loadGlm52AutoSwitchEnabled(): boolean {
  try {
    return localStorage.getItem("zcs:glm52AutoSwitchEnabled") === "1";
  } catch {
    return false;
  }
}
function clampGlm52ThresholdWan(n: number): number {
  return Math.max(GLM52_THRESHOLD_WAN_MIN, Math.min(GLM52_THRESHOLD_WAN_MAX, Math.round(n)));
}
function loadGlm52AutoSwitchThresholdWan(): number {
  try {
    const n = Number(localStorage.getItem("zcs:glm52AutoSwitchThresholdWan"));
    return Number.isFinite(n) && n > 0
      ? clampGlm52ThresholdWan(n)
      : GLM52_DEFAULT_THRESHOLD_WAN;
  } catch {
    return GLM52_DEFAULT_THRESHOLD_WAN;
  }
}
function clampGlm52PointThreshold(n: number): number {
  return Math.max(
    GLM52_POINT_THRESHOLD_MIN,
    Math.min(GLM52_POINT_THRESHOLD_MAX, Math.round(n))
  );
}
function loadGlm52AutoSwitchPointThreshold(): number {
  try {
    const n = Number(localStorage.getItem("zcs:glm52AutoSwitchPointThreshold"));
    return Number.isFinite(n) && n > 0
      ? clampGlm52PointThreshold(n)
      : GLM52_DEFAULT_POINT_THRESHOLD;
  } catch {
    return GLM52_DEFAULT_POINT_THRESHOLD;
  }
}
function loadGlm52LowQuotaAction(): LowQuotaAction {
  try {
    return localStorage.getItem("zcs:glm52LowQuotaAction") === "pause"
      ? "pause"
      : DEFAULT_LOW_QUOTA_ACTION;
  } catch {
    return DEFAULT_LOW_QUOTA_ACTION;
  }
}
function loadTheme(): Theme {
  try {
    const t = localStorage.getItem("zcs:theme");
    return t === "dark" || t === "light" ? t : DEFAULT_THEME;
  } catch {
    return DEFAULT_THEME;
  }
}
function loadFloatingWindowMode(): boolean {
  try {
    return localStorage.getItem(FLOATING_WINDOW_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}
const FLOATING_WINDOW_SCALE_KEY = "zcs:floatingWindowScale";
const FLOATING_WINDOW_SCALE_MIN = 0.7;
const FLOATING_WINDOW_SCALE_MAX = 1.6;
function clampFloatingScale(v: number): number {
  if (!Number.isFinite(v)) return 1;
  return Math.min(FLOATING_WINDOW_SCALE_MAX, Math.max(FLOATING_WINDOW_SCALE_MIN, v));
}
function loadFloatingWindowScale(): number {
  try {
    const raw = localStorage.getItem(FLOATING_WINDOW_SCALE_KEY);
    if (raw === null) return 1;
    return clampFloatingScale(Number(raw));
  } catch {
    return 1;
  }
}
function loadAccountViewMode(): AccountViewMode {
  try {
    const v = localStorage.getItem("zcs:accountViewMode");
    return v === "list" ? "list" : "card";
  } catch {
    return "card";
  }
}
const VALID_SORT_MODES: readonly AccountSortMode[] = [
  "name-asc",
  "name-desc",
  "quota-desc",
  "quota-asc",
  "expiry-asc",
];
function loadAccountSortMode(): AccountSortMode {
  try {
    const v = localStorage.getItem("zcs:accountSortMode") as AccountSortMode | null;
    return v && VALID_SORT_MODES.includes(v) ? v : DEFAULT_ACCOUNT_SORT_MODE;
  } catch {
    return DEFAULT_ACCOUNT_SORT_MODE;
  }
}
function loadHideAccountIdentity(): boolean {
  try {
    return localStorage.getItem("zcs:hideAccountIdentity") === "1";
  } catch {
    return false;
  }
}
function loadLanguage(): Language {
  try {
    const v = localStorage.getItem("zcs:language");
    return v === "en" || v === "ru" ? v : "zh";
  } catch {
    return "zh";
  }
}
function clampActiveQuotaRefreshIntervalMinutes(v: number): number {
  if (!Number.isFinite(v)) return DEFAULT_ACTIVE_QUOTA_REFRESH_INTERVAL_MINUTES;
  return Math.max(1, Math.min(5, Math.round(v)));
}
function loadActiveQuotaRefreshIntervalMinutes(): number {
  try {
    const saved = localStorage.getItem(ACTIVE_QUOTA_REFRESH_INTERVAL_MINUTES_KEY);
    if (saved === null) return DEFAULT_ACTIVE_QUOTA_REFRESH_INTERVAL_MINUTES;
    return clampActiveQuotaRefreshIntervalMinutes(Number(saved));
  } catch {
    return DEFAULT_ACTIVE_QUOTA_REFRESH_INTERVAL_MINUTES;
  }
}

function randomGatewayKey(): string {
  const alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
  const bytes = new Uint8Array(32);
  try {
    crypto.getRandomValues(bytes);
  } catch {
    for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
  }
  return Array.from(bytes, (b) => alphabet[b % alphabet.length]).join("");
}

function loadProxyPort(): number {
  try {
    const saved = Number(localStorage.getItem(PROXY_PORT_KEY));
    return Number.isFinite(saved) && saved > 0 && saved <= 65535
      ? Math.round(saved)
      : DEFAULT_PROXY_PORT;
  } catch {
    return DEFAULT_PROXY_PORT;
  }
}

function loadProxyGatewayKey(): string {
  try {
    const saved = localStorage.getItem(PROXY_GATEWAY_KEY);
    if (saved && saved.length >= 12) return saved;
    const key = randomGatewayKey();
    localStorage.setItem(PROXY_GATEWAY_KEY, key);
    return key;
  } catch {
    return randomGatewayKey();
  }
}

function hasDisplayableQuota(quota?: QuotaInfo): boolean {
  return !!quota && !quota.error && ((quota.balances?.length ?? 0) > 0 || !!quota.plan_name);
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

/** 把主题应用到 <html data-theme> 及窗口背景 */
function applyTheme(theme: Theme) {
  const bg = theme === "dark" ? "#111317" : "#f6f7f9";
  const floating = document.documentElement.dataset.windowMode === "floating";
  try {
    document.documentElement.setAttribute("data-theme", theme);
    const domBg = floating ? "transparent" : bg;
    document.documentElement.style.backgroundColor = domBg;
    document.body.style.backgroundColor = domBg;
    document.getElementById("root")?.style.setProperty("background-color", domBg);
  } catch {
    /* ignore */
  }
  getCurrentWindow()
    .setBackgroundColor(bg)
    .catch(() => {
      /* ignore */
    });
}

/**
 * 低额度自动动作（每次额度刷新后调用）。
 *
 * 判定对象是 ZCode 当前选中的套餐入口（setting.json 的 selected key）：
 * - 切换模式："当前入口"余额不足时，先切到同帐号内有余额的另一个入口
 *   （Start Plan 入口 ↔ Coding Plan 入口，即切套餐）；帐号内所有套餐都
 *   低于阈值后才切换帐号。
 * - 暂停模式：当前入口余额不足时，让本地网关拦截所有模型请求（429/SSE
 *   error），并停止一切自动切换，等待人工处理；额度恢复后自动解除。
 *
 * 切换前强制复核目标余额（防止从耗尽的帐户切到另一个耗尽的帐户）：
 * - 切套餐：执行 switchPlan 前重新拉取当前帐号的最新余额并复核目标入口
 *   仍然高于阈值，复核不通过就绝不切换；
 * - 切帐号：候选先用缓存余量排出验证顺序，然后逐一重新拉取最新余额复核
 *   （双阈值全部达标才合格），只切到第一个验证通过的帐号；全部不通过
 *   则进入自动暂停，不做任何切换。
 */
async function maybeSwitchGlm52Account(getStore: () => AppState) {
  const state = getStore();
  const t = getTexts(state.language);
  if (!state.glm52AutoSwitchEnabled || glm52AutoSwitching || state.busy) return;

  const active = state.profiles.find((p) => p.active);
  const activeQuota = active ? state.quotas[active.id] : undefined;
  if (!active || !activeQuota || activeQuota.error) return;

  const tokenWan = state.glm52AutoSwitchThresholdWan;
  const pointThreshold = state.glm52AutoSwitchPointThreshold;
  const currentLow = isCurrentEntryLow(activeQuota, tokenWan, pointThreshold);

  if (state.glm52LowQuotaAction === "pause") {
    if (!currentLow) {
      // 暂停模式要求等待人工操作：额度即使随后回升也不自动放行。
      // 人工切号、关闭自动切换或切回“自动切换套餐/账号”模式时会清除
      // QUOTA_GUARD；这样不会在低额度后静默恢复执行。
      return;
    }
    void api.setQuotaGuard(true, t.glmGuardReason).catch(() => {});
    if (!guardPauseNotified) {
      // 暂停模式是用户显式选择的另一种动作：仅通过网关拦截 + toast
      // 表达暂停，不修改 autoSwitchPaused（其语义是"所有账号 Token/
      // 积分均低于阈值"，与本模式无关，否则全局 UI 文案会不一致）。
      guardPauseNotified = true;
      state.toast(t.glmGuardPaused, "warn");
    }
    return;
  }

  // 切换模式：清掉可能残留的网关拦截。
  if (guardPauseNotified) {
    guardPauseNotified = false;
    state.toast(t.glmGuardResumed, "success");
  }
  void api.setQuotaGuard(false, "").catch(() => {});

  // 当前入口余额充足（或入口未知且整帐号判定未触发）→ 无需动作。
  if (!currentLow) {
    if (state.autoSwitchPaused) state.setAutoSwitchPaused(false);
    return;
  }

  // 第 1 步：帐号内先切套餐。入口未知（读不到 active_provider）时跳过——
  // 不知道当前在哪个入口就盲切只会把请求切到错误的量纲上。
  const currentEntry = currentPlanEntry(activeQuota.active_provider);
  if (currentEntry) {
    const target: PlanEntryTarget | null = pickPlanSwitchTarget(
      activeQuota,
      currentEntry,
      tokenWan,
      pointThreshold
    );
    if (target) {
      const cooldown = lastPlanSwitchFailed
        ? PLAN_SWITCH_RETRY_COOLDOWN_MS
        : PLAN_SWITCH_COOLDOWN_MS;
      if (Date.now() - lastPlanSwitchAttemptAt < cooldown) {
        return; // 冷却期内，等下一轮刷新复核
      }
      glm52AutoSwitching = true;
      let switched = false;
      try {
        // 执行前重新拉取当前帐号的最新余额复核：触发判定的数据可能已经
        // 过了整轮批量刷新（多帐号逐个拉取，排在前面的可能已是几分钟前），
        // 复核不通过（目标入口实际已无余额）就绝不执行切套餐。
        await state.refreshQuota(active.id);
        const freshActive = getStore().quotas[active.id];
        const freshOk = !!freshActive && !freshActive.error;
        if (freshOk && !isCurrentEntryLow(freshActive, tokenWan, pointThreshold)) {
          // 复核发现当前入口余额已恢复：无需任何切换。
          if (getStore().autoSwitchPaused) getStore().setAutoSwitchPaused(false);
          return;
        }
        if (
          freshOk &&
          pickPlanSwitchTarget(freshActive, currentEntry, tokenWan, pointThreshold) ===
            target
        ) {
          if (getStore().busy) return; // 验证期间用户开始手动操作：不抢动作
          // 冷却时间戳只在真正执行切换时记录：复核轮次（目标已耗尽/余额
          // 已恢复）不消耗冷却，否则一次"复核不动"会把切套餐和切账号一起
          // 阻塞一个冷却周期。
          lastPlanSwitchAttemptAt = Date.now();
          state.toast(
            t.glmAutoSwitchingPlan.replace(
              "{plan}",
              t[planEntryLabelKey(target)]
            ),
            "info"
          );
          const outcome = await api.switchPlan(target);
          lastPlanSwitchFailed = false;
          switched = true;
          if (outcome.applied_live) {
            state.toast(
              t.glmPlanSwitchedLive.replace(
                "{plan}",
                t[planEntryLabelKey(target)]
              ),
              "success"
            );
          } else {
            state.toast(
              t.glmPlanSwitchedNeedRestart.replace(
                "{plan}",
                t[planEntryLabelKey(target)]
              ),
              "warn"
            );
            // 实时通道不可用（ZCode 未开/无调试端口）：开了自动重启就直接重启，
            // 让写入的配置立即生效。
            const { autoRestart, tryNoRestartSwitch } = state;
            if (autoRestart && !tryNoRestartSwitch) {
              await state.restartZcode();
            }
          }
          await state.refreshQuota(active.id);
        }
      } catch (e) {
        // 典型原因：coding-plan 入口没有 API Key → 该入口实际不可用，
        // 视同耗尽，继续走帐号切换；失败后进入长冷却，避免每轮刷新都报错。
        lastPlanSwitchFailed = true;
        state.toast(
          t.glmPlanSwitchFailed.replace("{error}", String(e)),
          "error"
        );
      } finally {
        glm52AutoSwitching = false;
      }
      if (switched) return;
      // 复核不通过（目标入口实际已无余额）或切换失败：帐号内已无可用
      // 套餐入口，落到下面的切帐号验证，绝不留在耗尽的入口上。
    }
  }

  // 第 2 步：帐号内所有套餐都低于阈值 → 切换帐号。候选先用缓存余量排出
  // 优先级顺序，然后用最新余额复核（限并发、单候选限时）：其已知 token/积分
  // 量纲必须全部高于阈值（缓存可能过期，凭旧数据切换会从一个耗尽的帐户切到
  // 另一个耗尽的帐户）；只切到验证通过的候选中优先级最高的帐号。
  const orderedCandidates = state.profiles
    .filter((p) => p.id !== active.id)
    .map((profile) => ({ profile, quota: state.quotas[profile.id] }))
    .filter((item) => isSwitchableCandidate(item.quota, tokenWan, pointThreshold))
    .map((item) => ({
      ...item,
      headroom: accountHeadroom(item.quota, tokenWan, pointThreshold),
      tokenRemaining: glm52Remaining(item.quota) ?? 0,
    }))
    .sort((a, b) => b.headroom - a.headroom || b.tokenRemaining - a.tokenRemaining);

  if (orderedCandidates.length === 0) {
    // 所有账号的 token/积分都低于阈值：进入自动暂停——暂停期间不做任何
    // 切换尝试（不会在低额度账号间循环切换），监测也回落到慢速档；
    // 仅提示一次，等任一账号额度恢复到双阈值之上后自动继续。
    if (!getStore().autoSwitchPaused) {
      getStore().setAutoSwitchPaused(true);
      state.toast(t.glmAutoSwitchPaused, "warn");
    }
    return;
  }

  glm52AutoSwitching = true;
  try {
    // 验证候选：每个候选切之前都重新拉取最新余额复核，拉取失败/超时或复核
    // 不合格的候选直接跳过（绝不盲切），余额充足里优先级最高的才成为目标。
    // 并发上限与单候选超时兜底验证链路：上游挂起时最多等 timeoutMs，
    // 多候选并行验证缩短整体决策时间；进度实时回报给 UI。
    getStore().setSwitchVerifyProgress({
      total: orderedCandidates.length,
      done: 0,
      failed: 0,
      active: [],
    });
    const verified = await firstVerifiedTarget(
      orderedCandidates,
      async (item) => {
        await state.refreshQuota(item.profile.id);
        return getStore().quotas[item.profile.id];
      },
      (_item, fresh) =>
        isSwitchableCandidate(fresh ?? undefined, tokenWan, pointThreshold),
      {
        concurrency: getStore().candidateVerifyConcurrency,
        timeoutMs: getStore().candidateVerifyTimeoutMs,
        onProgress: (progress) => {
          getStore().setSwitchVerifyProgress({
            total: progress.total,
            done: progress.done,
            failed: progress.failed,
            active: progress.activeIndexes
              .map((i) => orderedCandidates[i]?.profile.name)
              .filter((name): name is string => !!name),
          });
        },
      }
    );
    getStore().setSwitchVerifyProgress(null);

    if (!verified) {
      // 逐一验证后没有任何帐号余额达标：宁可暂停也绝不切换。
      const live = getStore();
      if (!live.autoSwitchPaused) {
        live.setAutoSwitchPaused(true);
        state.toast(t.glmAutoSwitchPaused, "warn");
      }
      return;
    }

    // 出现验证通过的候选（含暂停期间额度恢复的情况）：解除暂停并切换。
    if (getStore().autoSwitchPaused) getStore().setAutoSwitchPaused(false);
    if (getStore().busy) return; // 验证期间用户开始手动操作：不抢动作
    state.toast(
      t.glmAutoSwitching.replace("{name}", verified.profile.name),
      "info"
    );
    await state.switchTo(verified.profile.id);
  } finally {
    glm52AutoSwitching = false;
    // 验证中途抛错也不能把进度条残留到下一轮。
    getStore().setSwitchVerifyProgress(null);
  }
}

export const useStore = create<AppState>((set, get) => {
  // 立即应用主题，避免首屏闪烁
  const initialTheme = loadTheme();
  const initialLanguage = loadLanguage();
  const initialTryNoRestartSwitch = loadTryNoRestartSwitch();
  const initialFloatingWindowMode =
    initialTryNoRestartSwitch && loadFloatingWindowMode();
  const storedAutoRestart = loadAutoRestart();
  const initialAutoRestart = initialTryNoRestartSwitch ? false : storedAutoRestart;
  applyTheme(initialTheme);
  if (!initialFloatingWindowMode) {
    try {
      localStorage.setItem(FLOATING_WINDOW_STORAGE_KEY, "0");
    } catch {
      /* ignore */
    }
  }
  if (initialTryNoRestartSwitch && storedAutoRestart) {
    try {
      localStorage.setItem("zcs:autoRestart", "0");
    } catch {
      /* ignore */
    }
  }
  return {
    profiles: [],
    quotas: loadCachedQuotas(),
    loadingQuota: {},
    recentlyRefreshed: {},
    autoRefreshQuota: loadAutoRefresh(),
    quotaRefreshIntervalMinutes: loadQuotaRefreshIntervalMinutes(),
    activeQuotaRefreshIntervalMinutes: loadActiveQuotaRefreshIntervalMinutes(),
    scheduledRefreshSeq: 0,
    glm52AutoSwitchEnabled: loadGlm52AutoSwitchEnabled(),
    glm52AutoSwitchThresholdWan: loadGlm52AutoSwitchThresholdWan(),
    glm52AutoSwitchPointThreshold: loadGlm52AutoSwitchPointThreshold(),
    glm52LowQuotaAction: loadGlm52LowQuotaAction(),
    autoSwitchPaused: false,
    switchVerifyProgress: null,
    candidateVerifyConcurrency: DEFAULT_VERIFY_CONCURRENCY,
    candidateVerifyTimeoutMs: DEFAULT_VERIFY_TIMEOUT_MS,
    autoRestart: initialAutoRestart,
    tryNoRestartSwitch: initialTryNoRestartSwitch,
    theme: initialTheme,
    floatingWindowMode: initialFloatingWindowMode,
    floatingWindowScale: loadFloatingWindowScale(),
    updateAvailable: false,
    customProviders: [],
    accountPool: [],
    proxyStatus: null,
    proxyPort: loadProxyPort(),
    proxyGatewayKey: loadProxyGatewayKey(),
    accountViewMode: loadAccountViewMode(),
    accountSortMode: loadAccountSortMode(),
    hideAccountIdentity: loadHideAccountIdentity(),
    language: initialLanguage,
    loading: true,
    busy: false,
    toasts: [],

  refresh: async (silent = false, skipQuota = false) => {
    set({ loading: true });
    try {
      const profiles = await api.listProfiles();
      set({ profiles, loading: false });
      // skipQuota：只重载列表，由调用方按需刷新新号
      if (!skipQuota) {
        get().refreshAllQuota();
      }
      if (!silent) {
        get().toast(getTexts(get().language).refreshSuccess, "success");
      }
    } catch (e) {
      set({ loading: false });
      get().toast(
        getTexts(get().language).loadFailed.replace("{error}", String(e)),
        "error"
      );
    }
  },

  captureCurrent: async (name) => {
    set({ busy: true });
    try {
      const p = await api.captureCurrent(name);
      get().toast(
        getTexts(get().language).saveProfileSuccess.replace("{name}", p.name),
        "success"
      );
      // 只刷新刚捕获的新号
      await get().refresh(true, true);
      get().refreshQuota(p.id);
      return true;
    } catch (e) {
      get().toast(
        getTexts(get().language).saveFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    } finally {
      set({ busy: false });
    }
  },

  switchTo: async (id) => {
    set({ busy: true });
    try {
      const { autoRestart, tryNoRestartSwitch } = get();
      // 自动重启路径：先关再写再启，避免写入期间被覆盖
      if (!tryNoRestartSwitch && autoRestart) {
        try {
          await api.killZcodeForSwitch();
        } catch {
          /* ignore */
        }
      }
      const r = await api.switchTo(id);
      // 人工/自动切号都视为已介入处理：立即解除网关拦截（暂停模式下
      // 新账号的额度会在随后的刷新里重新评估）。
      guardPauseNotified = false;
      void api.setQuotaGuard(false, "").catch(() => {});
      // 就地翻 active 标记，排序交给渲染层
      set((s) => ({
        profiles: s.profiles.map((p) => ({ ...p, active: p.id === r.id })),
      }));
      const { toast } = get();
      const t = getTexts(get().language);
      if (tryNoRestartSwitch) {
        toast(t.switchNoRestartSuccess.replace("{name}", r.name), "success");
      } else if (autoRestart) {
        toast(t.switchingRestarting.replace("{name}", r.name), "info");
        await get().restartZcode();
      } else {
        toast(t.switchedManualNotice.replace("{name}", r.name), "success");
      }
      if (!hasDisplayableQuota(get().quotas[id])) {
        set((s) => ({ loadingQuota: { ...s.loadingQuota, [id]: true } }));
        for (let attempt = 0; attempt < 10; attempt += 1) {
          if (attempt > 0) {
            set((s) => ({ loadingQuota: { ...s.loadingQuota, [id]: true } }));
            await delay(1000);
          }
          await get().refreshQuota(id);
          if (hasDisplayableQuota(get().quotas[id])) break;
        }
      }
      return true;
    } catch (e) {
      get().toast(
        getTexts(get().language).switchFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    } finally {
      set({ busy: false });
    }
  },

  renameProfile: async (id, name) => {
    try {
      const ok = await api.renameProfile(id, name);
      if (ok) {
        get().toast(getTexts(get().language).renameSuccess, "success");
        await get().refresh();
      }
      return ok;
    } catch (e) {
      get().toast(
        getTexts(get().language).renameFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    }
  },

  deleteProfile: async (id) => {
    try {
      const ok = await api.deleteProfile(id);
      if (ok) {
        get().toast(getTexts(get().language).deleteSuccess, "success");
        // 直接从本地列表移除，清掉对应额度缓存
        set((s) => {
          const { [id]: _drop, ...rest } = s.quotas;
          saveCachedQuotas(rest);
          return {
            profiles: s.profiles.filter((p) => p.id !== id),
            quotas: rest,
          };
        });
      }
      return ok;
    } catch (e) {
      get().toast(
        getTexts(get().language).deleteFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    }
  },

  deleteProfiles: async (ids) => {
    const uniqueIds = Array.from(new Set(ids.filter(Boolean)));
    if (uniqueIds.length === 0) return { deleted: 0, failed: 0 };
    set({ busy: true });
    const deletedIds: string[] = [];
    const errors: string[] = [];
    try {
      for (const id of uniqueIds) {
        try {
          const ok = await api.deleteProfile(id);
          if (ok) deletedIds.push(id);
          else errors.push(id);
        } catch (e) {
          errors.push(String(e));
        }
      }
      if (deletedIds.length > 0) {
        set((s) => {
          const quotas = { ...s.quotas };
          for (const id of deletedIds) delete quotas[id];
          saveCachedQuotas(quotas);
          return {
            profiles: s.profiles.filter((p) => !deletedIds.includes(p.id)),
            quotas,
          };
        });
      }
      const t = getTexts(get().language);
      if (errors.length === 0) {
        get().toast(
          t.batchDeleteSuccess.replace("{count}", String(deletedIds.length)),
          "success"
        );
      } else if (deletedIds.length > 0) {
        get().toast(
          t.batchDeletePartial
            .replace("{deleted}", String(deletedIds.length))
            .replace("{failed}", String(errors.length)),
          "warn"
        );
      } else {
        get().toast(
          t.batchDeleteFailed.replace("{error}", errors.slice(0, 2).join("; ")),
          "error"
        );
      }
      return { deleted: deletedIds.length, failed: errors.length };
    } finally {
      set({ busy: false });
    }
  },

  refreshQuota: async (id) => {
    // 单个账号刷新：立即执行，不进队列，保证手动点击响应及时
    set((s) => ({ loadingQuota: { ...s.loadingQuota, [id]: true } }));
    try {
      const info = await api.fetchQuota(id);
      const q: QuotaInfo = { ...info, fetched_at: Date.now() / 1000, error: null };
      set((s) => {
        const quotas = { ...s.quotas, [id]: q };
        saveCachedQuotas(quotas);
        return {
          quotas,
          recentlyRefreshed: { ...s.recentlyRefreshed, [id]: true },
        };
      });
      // 1 秒后清掉成功标记
      setTimeout(() => {
        set((s) => {
          const { [id]: _drop, ...rest } = s.recentlyRefreshed;
          return { recentlyRefreshed: rest };
        });
      }, 1000);
    } catch (e) {
      set((s) => {
        const quotas = {
          ...s.quotas,
          [id]:
            s.quotas[id]?.balances?.length > 0
              ? { ...s.quotas[id], error: String(e), fetched_at: Date.now() / 1000 }
              : {
                  plan_name: null,
                  plan_description: null,
                  plan_status: null,
                  plan_ends_at: null,
                  balances: [],
                  error: String(e),
                  fetched_at: Date.now() / 1000,
                },
        };
        saveCachedQuotas(quotas);
        // 失败时清掉可能残留的成功标记
        const { [id]: _ok, ...restOk } = s.recentlyRefreshed;
        return { quotas, recentlyRefreshed: restOk };
      });
    } finally {
      set((s) => {
        const { [id]: _drop, ...rest } = s.loadingQuota;
        return { loadingQuota: rest };
      });
    }
  },

  refreshAllQuota: async (orderedIds) => {
    // 进行中跳过，完成后 bump tick 重置定时器倒计时
    if (refreshAllInFlight) return;
    refreshAllInFlight = true;
    try {
      const state = get();
      const { refreshQuota } = state;
      const ordered = orderedProfilesForRefresh(state, orderedIds);
      for (const p of ordered) {
        await refreshQuota(p.id);
      }
      await maybeSwitchGlm52Account(get);
    } finally {
      refreshAllInFlight = false;
      set((s) => ({ scheduledRefreshSeq: s.scheduledRefreshSeq + 1 }));
    }
  },

  refreshMissingQuota: async () => {
    // 只刷新还没有额度数据的账号，逐个 await
    const { profiles, quotas, refreshQuota } = get();
    for (const p of profiles) {
      const q = quotas[p.id];
      const hasData = !!q && ((q.balances?.length ?? 0) > 0 || !!q.plan_name);
      if (!hasData) {
        await refreshQuota(p.id);
      }
    }
  },

  scheduledRefreshAllQuota: async (orderedIds) => {
    // 进行中跳过；tryNoRestartSwitch 时跳过当前活跃账号
    if (refreshAllInFlight) return;
    refreshAllInFlight = true;
    try {
      const state = get();
      const { refreshQuota, tryNoRestartSwitch } = state;
      const ordered = orderedProfilesForRefresh(state, orderedIds);
      for (const p of ordered) {
        if (tryNoRestartSwitch && p.active) continue;
        await refreshQuota(p.id);
      }
      await maybeSwitchGlm52Account(get);
    } finally {
      refreshAllInFlight = false;
    }
  },

  refreshActiveQuotaForAutoSwitch: async () => {
    const active = get().profiles.find((p) => p.active);
    if (!active) return;
    await get().refreshQuota(active.id);
    await maybeSwitchGlm52Account(get);
  },

  setAutoRefreshQuota: (v) => {
    try {
      localStorage.setItem("zcs:autoRefreshQuota", v ? "1" : "0");
    } catch {
      /* ignore */
    }
    set({ autoRefreshQuota: v });
  },

  setQuotaRefreshIntervalMinutes: (v) => {
    const minutes = Number.isFinite(v) && v > 0 ? Math.min(1440, Math.round(v)) : 0;
    try {
      localStorage.setItem("zcs:quotaRefreshIntervalMinutes", String(minutes));
    } catch {
      /* ignore */
    }
    set({ quotaRefreshIntervalMinutes: minutes });
  },

  setActiveQuotaRefreshIntervalMinutes: (v) => {
    const minutes = clampActiveQuotaRefreshIntervalMinutes(v);
    try {
      localStorage.setItem(ACTIVE_QUOTA_REFRESH_INTERVAL_MINUTES_KEY, String(minutes));
    } catch {
      /* ignore */
    }
    set({ activeQuotaRefreshIntervalMinutes: minutes });
  },

  setGlm52AutoSwitchEnabled: (v) => {
    try {
      localStorage.setItem("zcs:glm52AutoSwitchEnabled", v ? "1" : "0");
    } catch {
      /* ignore */
    }
    if (!v) {
      // 关闭自动切换时同步解除网关拦截，避免守护标志残留。
      guardPauseNotified = false;
      void api.setQuotaGuard(false, "").catch(() => {});
    }
    // 重新开启时清掉上一轮的自动暂停，让切换判定从头开始。
    set(
      v
        ? { glm52AutoSwitchEnabled: true, autoSwitchPaused: false }
        : { glm52AutoSwitchEnabled: false }
    );
  },

  setGlm52LowQuotaAction: (v) => {
    try {
      localStorage.setItem("zcs:glm52LowQuotaAction", v);
    } catch {
      /* ignore */
    }
    if (v === "switch") {
      // 从暂停模式切回切换模式：立即解除网关拦截。
      guardPauseNotified = false;
      void api.setQuotaGuard(false, "").catch(() => {});
    }
    set({ glm52LowQuotaAction: v });
  },

  setGlm52AutoSwitchThresholdWan: (v) => {
    const wan =
      Number.isFinite(v) && v > 0
        ? clampGlm52ThresholdWan(v)
        : GLM52_DEFAULT_THRESHOLD_WAN;
    try {
      localStorage.setItem("zcs:glm52AutoSwitchThresholdWan", String(wan));
    } catch {
      /* ignore */
    }
    set({ glm52AutoSwitchThresholdWan: wan });
  },

  setGlm52AutoSwitchPointThreshold: (v) => {
    const point =
      Number.isFinite(v) && v > 0
        ? clampGlm52PointThreshold(v)
        : GLM52_DEFAULT_POINT_THRESHOLD;
    try {
      localStorage.setItem("zcs:glm52AutoSwitchPointThreshold", String(point));
    } catch {
      /* ignore */
    }
    set({ glm52AutoSwitchPointThreshold: point });
  },

  setAutoSwitchPaused: (v) => {
    if (get().autoSwitchPaused === v) return;
    set({ autoSwitchPaused: v });
  },

  setSwitchVerifyProgress: (p) => {
    set({ switchVerifyProgress: p });
  },

  setAutoRestart: (v) => {
    if (v && get().tryNoRestartSwitch) {
      try {
        localStorage.setItem("zcs:autoRestart", "0");
      } catch {
        /* ignore */
      }
      set({ autoRestart: false });
      get().toast(getTexts(get().language).autoRestartBlocked, "warn");
      return;
    }
    try {
      localStorage.setItem("zcs:autoRestart", v ? "1" : "0");
    } catch {
      /* ignore */
    }
    set({ autoRestart: v });
  },

  setTryNoRestartSwitch: (v) => {
    try {
      localStorage.setItem("zcs:tryNoRestartSwitch", v ? "1" : "0");
      if (v) localStorage.setItem("zcs:autoRestart", "0");
      if (!v) localStorage.setItem(FLOATING_WINDOW_STORAGE_KEY, "0");
    } catch {
      /* ignore */
    }
    set(
      v
        ? { tryNoRestartSwitch: true, autoRestart: false }
        : { tryNoRestartSwitch: false, floatingWindowMode: false }
    );
  },

  setTheme: (v) => {
    try {
      localStorage.setItem("zcs:theme", v);
    } catch {
      /* ignore */
    }
    applyTheme(v);
    set({ theme: v });
  },

  setFloatingWindowMode: (v) => {
    if (v && !get().tryNoRestartSwitch) {
      try {
        localStorage.setItem(FLOATING_WINDOW_STORAGE_KEY, "0");
      } catch {
        /* ignore */
      }
      set({ floatingWindowMode: false });
      get().toast(getTexts(get().language).floatingWindowBlocked, "warn");
      return;
    }
    try {
      localStorage.setItem(FLOATING_WINDOW_STORAGE_KEY, v ? "1" : "0");
    } catch {
      /* ignore */
    }
    set({ floatingWindowMode: v });
  },

  setFloatingWindowScale: (v) => {
    const scale = clampFloatingScale(v);
    try {
      localStorage.setItem(FLOATING_WINDOW_SCALE_KEY, String(scale));
    } catch {
      /* ignore */
    }
    set({ floatingWindowScale: scale });
  },

  setUpdateAvailable: (v) => {
    set({ updateAvailable: v });
  },

  refreshCustomProviders: async () => {
    try {
      const customProviders = await api.listCustomProviders();
      set({ customProviders });
    } catch (e) {
      get().toast(
        getTexts(get().language).providerLoadFailed.replace("{error}", String(e)),
        "error"
      );
    }
  },

  refreshAccountPool: async () => {
    try {
      const accountPool = await api.listAccountPool();
      set({ accountPool });
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  addAccountToPool: async (profileId) => {
    try {
      const entry = await api.addAccountToPool(profileId);
      set((s) => ({
        accountPool: [
          ...s.accountPool.filter((item) => item.profile_id !== entry.profile_id),
          entry,
        ],
      }));
      return true;
    } catch (e) {
      get().toast(String(e), "error");
      return false;
    }
  },

  setAccountPoolEnabled: async (profileId, enabled) => {
    try {
      const entry = await api.setAccountPoolEnabled(profileId, enabled);
      set((s) => ({
        accountPool: s.accountPool.map((item) =>
          item.profile_id === profileId ? entry : item
        ),
      }));
      return true;
    } catch (e) {
      get().toast(String(e), "error");
      return false;
    }
  },

  removeAccountFromPool: async (profileId) => {
    try {
      const ok = await api.removeAccountFromPool(profileId);
      if (ok) {
        set((s) => ({
          accountPool: s.accountPool.filter((item) => item.profile_id !== profileId),
        }));
      }
      return ok;
    } catch (e) {
      get().toast(String(e), "error");
      return false;
    }
  },

  addCustomProvider: async (name, baseUrl, apiKey, apiFormat, models) => {
    set({ busy: true });
    try {
      const provider = await api.addCustomProvider(
        name,
        baseUrl,
        apiKey,
        apiFormat,
        models
      );
      set((s) => ({ customProviders: [...s.customProviders, provider] }));
      get().toast(getTexts(get().language).providerAdded, "success");
      return true;
    } catch (e) {
      get().toast(
        getTexts(get().language).providerAddFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    } finally {
      set({ busy: false });
    }
  },

  deleteCustomProvider: async (id) => {
    try {
      const ok = await api.deleteCustomProvider(id);
      if (ok) {
        set((s) => ({
          customProviders: s.customProviders.filter((provider) => provider.id !== id),
        }));
        get().toast(getTexts(get().language).providerDeleted, "success");
      }
      return ok;
    } catch (e) {
      get().toast(
        getTexts(get().language).providerDeleteFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    }
  },

  setCustomProviderEnabled: async (id, enabled) => {
    const provider = get().customProviders.find((item) => item.id === id);
    if (!provider) return false;
    try {
      const updated = await api.updateCustomProvider(
        provider.id,
        provider.name,
        provider.base_url,
        null,
        provider.api_format,
        provider.models,
        enabled
      );
      set((s) => ({
        customProviders: s.customProviders.map((item) =>
          item.id === id ? updated : item
        ),
      }));
      return true;
    } catch (e) {
      get().toast(
        getTexts(get().language).providerUpdateFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    }
  },

  activateCustomProvider: async (id) => {
    const providers = get().customProviders;
    const target = providers.find((item) => item.id === id);
    if (!target) return false;
    try {
      const updatedProviders = [];
      for (const provider of providers) {
        const shouldEnable = provider.id === id;
        if (provider.enabled === shouldEnable) {
          updatedProviders.push(provider);
          continue;
        }
        const updated = await api.updateCustomProvider(
          provider.id,
          provider.name,
          provider.base_url,
          null,
          provider.api_format,
          provider.models,
          shouldEnable
        );
        updatedProviders.push(updated);
      }
      set({ customProviders: updatedProviders });
      get().toast(
        getTexts(get().language).providerActivated.replace("{name}", target.name),
        "success"
      );
      return true;
    } catch (e) {
      get().toast(
        getTexts(get().language).providerActivateFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    }
  },

  refreshProxyStatus: async () => {
    try {
      const proxyStatus = await api.proxyStatus();
      set({
        proxyStatus,
        proxyPort: proxyStatus.port > 0 ? proxyStatus.port : get().proxyPort,
      });
    } catch {
      set({ proxyStatus: null });
    }
  },

  startLocalProxy: async () => {
    try {
      const proxyStatus = await api.startProxy(get().proxyPort, get().proxyGatewayKey);
      set({ proxyStatus, proxyPort: proxyStatus.port });
      try {
        localStorage.setItem(PROXY_PORT_KEY, String(proxyStatus.port));
      } catch {
        /* ignore */
      }
      get().toast(getTexts(get().language).proxyStarted, "success");
      return true;
    } catch (e) {
      get().toast(
        getTexts(get().language).proxyStartFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    }
  },

  stopLocalProxy: async () => {
    try {
      const proxyStatus = await api.stopProxy();
      set({ proxyStatus });
      get().toast(getTexts(get().language).proxyStoppedToast, "success");
      return true;
    } catch (e) {
      get().toast(
        getTexts(get().language).proxyStopFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    }
  },

  setProxyPort: (v) => {
    const port = Number.isFinite(v) && v > 0 ? Math.min(65535, Math.round(v)) : DEFAULT_PROXY_PORT;
    try {
      localStorage.setItem(PROXY_PORT_KEY, String(port));
    } catch {
      /* ignore */
    }
    set({ proxyPort: port });
  },

  regenerateProxyGatewayKey: () => {
    const proxyGatewayKey = randomGatewayKey();
    try {
      localStorage.setItem(PROXY_GATEWAY_KEY, proxyGatewayKey);
    } catch {
      /* ignore */
    }
    set({ proxyGatewayKey });
    get().toast(getTexts(get().language).proxyKeyRegenerated, "success");
  },

  setAccountViewMode: (v) => {
    try {
      localStorage.setItem("zcs:accountViewMode", v);
    } catch {
      /* ignore */
    }
    set({ accountViewMode: v });
  },

  setAccountSortMode: (v) => {
    try {
      localStorage.setItem("zcs:accountSortMode", v);
    } catch {
      /* ignore */
    }
    set({ accountSortMode: v });
  },

  setHideAccountIdentity: (v) => {
    try {
      localStorage.setItem("zcs:hideAccountIdentity", v ? "1" : "0");
    } catch {
      /* ignore */
    }
    set({ hideAccountIdentity: v });
  },

  setLanguage: (v) => {
    try {
      localStorage.setItem("zcs:language", v);
    } catch {
      /* ignore */
    }
    set({ language: v });
  },

  restartZcode: async () => {
    try {
      await api.restartZcode();
      get().toast(getTexts(get().language).restartSuccess, "success");
      return true;
    } catch (e) {
      get().toast(
        getTexts(get().language).restartFailed.replace("{error}", String(e)),
        "error"
      );
      return false;
    }
  },

  toast: (text, kind = "info") => {
    const id = toastSeq++;
    set((s) => ({ toasts: [...s.toasts, { id, text, kind }] }));
    setTimeout(() => get().dismissToast(id), 2600);
  },

  dismissToast: (id) => {
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
  },
  };
});

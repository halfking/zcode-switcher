import { useEffect, useState } from "react";
import type { BalanceItem } from "../lib/api";
import { formatResetCountdown } from "../lib/glm52";
import { formatText, getTexts, type Language } from "../i18n";

interface Props {
  item: BalanceItem;
  /** 紧凑模式：适配窄卡片，单行显示，不使用固定宽度列。 */
  compact?: boolean;
  /** 该条目所属套餐是否为 ZCode 当前选中的供应者（显示"使用中"标记）。 */
  active?: boolean;
  /** "使用中" 文案（title 提示用），由调用方按语言传入。 */
  activeLabel?: string;
  /** 界面语言（重置倒计时文案）；缺省中文。 */
  language?: Language;
}

/** 把数值格式化成易读字符串（万 / 百万 / 亿）。 */
function fmt(n: number): string {
  if (!isFinite(n)) return "—";
  const abs = Math.abs(n);
  if (abs >= 1e8) return (n / 1e8).toFixed(2) + " 亿";
  if (abs >= 1e4) return (n / 1e4).toFixed(2) + " 万";
  if (abs >= 1) return Math.round(n).toLocaleString();
  return n.toFixed(2);
}

/** 颜色类名：按剩余额度判断，足够为绿，偏低转橙/红。 */
function colorFor(remainingPct: number): string {
  if (remainingPct <= 10) return "bg-danger";
  if (remainingPct <= 30) return "bg-warn";
  return "bg-ok";
}

/** 倒计时需要随时间推进重渲染；30 秒一跳与"到分"的展示粒度匹配。 */
function useNowTick(enabled: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!enabled) return;
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, [enabled]);
  return now;
}

export function QuotaBar({
  item,
  compact = false,
  active = false,
  activeLabel,
  language = "zh",
}: Props) {
  const t = getTexts(language);
  const total = item.total_units || 0;
  const used = item.used_units || 0;
  const remaining = item.remaining_units || Math.max(0, total - used);
  const remainingPct =
    total > 0 ? Math.min(100, Math.max(0, (remaining / total) * 100)) : 0;
  const color = colorFor(remainingPct);
  // 单位标注：积分桶带"积分"；百分比 token 桶（total=100）以 % 呈现，
  // 宽视图不再重复"x% / 100%"。
  const isPercent = item.unit_type === "percentage";
  const unit = isPercent ? "%" : item.unit_type === "point" ? " 积分" : "";
  // 重置倒计时（next_reset_at 有值才展示；已过期交给到点刷新，不显示）。
  const resetAtMs = (item.next_reset_at ?? 0) * 1000;
  const now = useNowTick(resetAtMs > 0);
  const countdown = resetAtMs > 0 ? formatResetCountdown((resetAtMs - now) / 1000) : "";
  const resetLabel = countdown
    ? formatText(t.quotaResetIn, { time: countdown })
    : "";
  const nameTitle = resetLabel
    ? `${item.show_name}${active && activeLabel ? `（${activeLabel}）` : ""}（${resetLabel}）`
    : active && activeLabel
    ? `${item.show_name}（${activeLabel}）`
    : item.show_name;
  // 使用中的条目加左侧绿色边条，与普通条目视觉区分且不挤占窄卡片宽度。
  const activeRing = active ? "border-l-2 border-ok pl-1" : "";

  if (compact) {
    // 紧凑模式：单行，名称弹性截断，进度条细，剩余值在右
    return (
      <div className={`flex min-w-0 items-center gap-1.5 ${activeRing}`}>
        <span
          className="w-16 shrink-0 truncate text-[10px] font-medium text-text-muted"
          title={nameTitle}
        >
          {item.show_name}
        </span>
        <div className="relative h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-base-cardhover">
          <div
            className={`h-full rounded-full transition-all duration-500 ${color}`}
            style={{ width: `${remainingPct}%` }}
          />
        </div>
        <span className="shrink-0 font-mono text-[9px] leading-none text-text-muted">
          {fmt(remaining)}
          {unit}
        </span>
        {countdown && (
          <span
            className="shrink-0 font-mono text-[9px] leading-none text-text-muted/70"
            title={resetLabel}
          >
            {countdown}
          </span>
        )}
      </div>
    );
  }

  return (
    <div className={`flex items-center gap-3 ${activeRing}`}>
      <span className="w-28 shrink-0 truncate text-[11px] font-medium text-text-secondary" title={nameTitle}>
        {item.show_name}
      </span>
      <div className="relative h-2 flex-1 overflow-hidden rounded-full bg-base-cardhover">
        <div
          className={`h-full rounded-full transition-all duration-500 ${color}`}
          style={{ width: `${remainingPct}%` }}
        />
      </div>
      <span className="w-36 shrink-0 text-right font-mono text-[11px] text-text-muted">
        {isPercent ? (
          `${fmt(remaining)}%`
        ) : (
          <>
            {fmt(remaining)}
            {unit} / {fmt(total)}
            {unit}
          </>
        )}
        {countdown && (
          <span className="block text-[9px] font-medium leading-tight text-text-muted/70">
            {resetLabel}
          </span>
        )}
      </span>
    </div>
  );
}

export default QuotaBar;

import type { BalanceItem, QuotaInfo } from "./api";

/**
 * 额度接口按套餐优先级返回 balances，排序使用第一条有数值的额度项。
 * 不绑定具体模型名，新增模型或套餐名称变化时仍能参与排序。
 */
export function findPrimaryBalance(quota?: QuotaInfo): BalanceItem | undefined {
  return quota?.balances?.find(
    (item) =>
      Number.isFinite(item.remaining_units) ||
      (Number.isFinite(item.total_units) && Number.isFinite(item.used_units))
  );
}

export function primaryQuotaRemaining(quota?: QuotaInfo): number | null {
  const item = findPrimaryBalance(quota);
  if (!item) return null;
  if (Number.isFinite(item.remaining_units)) return item.remaining_units;
  if (Number.isFinite(item.total_units) && Number.isFinite(item.used_units)) {
    return Math.max(0, item.total_units - item.used_units);
  }
  return null;
}

import { formatMoney, formatResetIn, severityBar, severityText } from "../lib/format";
import type { QuotaWindow } from "../types/usage";

interface Props {
  quota: QuotaWindow;
  now: number;
}

export default function QuotaBar({ quota, now }: Props) {
  const pct = Math.min(100, Math.max(0, quota.usedPercent));
  const resetIn = formatResetIn(quota.resetsAt, now);
  const spend =
    quota.usedAmount && quota.limitAmount
      ? `${formatMoney(quota.usedAmount)} / ${formatMoney(quota.limitAmount)}`
      : quota.usedAmount
        ? formatMoney(quota.usedAmount)
        : null;

  return (
    // Dormant windows stay visible but recede. The dimming lives on the
    // progress bar only — never on the text — so a paused-but-critical row
    // (e.g. a maxed-out credit pool waiting to reset) keeps its numbers fully
    // legible; the "· idle" tag alone carries the dormant signal.
    <div>
      <div className="flex items-baseline justify-between gap-2 text-[12px]">
        <span className="truncate text-stone-700 dark:text-stone-200">
          {quota.label}
          {!quota.isActive && (
            <span className="ml-1 font-normal text-stone-500 dark:text-stone-300">
              · idle
            </span>
          )}
        </span>
        <span className={`tnum shrink-0 text-[13px] font-semibold ${severityText[quota.severity]}`}>
          {pct.toFixed(0)}%
        </span>
      </div>

      <div
        className={`mt-1 h-1.5 w-full overflow-hidden rounded-full bg-stone-200/80 dark:bg-stone-700/80 ${
          quota.isActive ? "" : "opacity-50"
        }`}
        role="progressbar"
        aria-valuenow={Math.round(pct)}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={quota.label}
      >
        <div
          className={`bar-fill h-full rounded-full ${severityBar[quota.severity]}`}
          style={{ width: `${pct}%` }}
        />
      </div>

      {(resetIn || spend) && (
        <div className="mt-1 flex items-baseline justify-between gap-2 text-[11px] text-stone-600 dark:text-stone-300">
          <span className="tnum truncate">{spend ?? ""}</span>
          <span className="tnum shrink-0">{resetIn ?? ""}</span>
        </div>
      )}
    </div>
  );
}

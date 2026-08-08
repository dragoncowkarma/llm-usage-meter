import type { Confidence, Money, Severity } from "../types/usage";

/** Compact token counts: 1_234_567 → "1.23M". */
export function formatTokens(n: number): string {
  if (n < 1_000) return String(n);
  if (n < 1_000_000) return `${(n / 1_000).toFixed(n < 10_000 ? 1 : 0)}K`;
  if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  return `${(n / 1_000_000_000).toFixed(2)}B`;
}

export function formatMoney(m: Money): string {
  const value = m.amountMinor / 10 ** m.exponent;
  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency: m.currency,
      // Sub-cent totals are common on cache-heavy days; showing "$0.00" would
      // read as "free" when it isn't.
      maximumFractionDigits: Math.abs(value) < 1 ? 4 : 2,
    }).format(value);
  } catch {
    return `${value.toFixed(2)} ${m.currency}`;
  }
}

/** "resets in 2h 14m" — the number a user actually acts on. */
export function formatResetIn(resetsAt: number | null, now: number): string | null {
  if (resetsAt === null) return null;
  const secs = resetsAt - now;
  if (secs <= 0) return "resetting";
  const d = Math.floor(secs / 86_400);
  const h = Math.floor((secs % 86_400) / 3_600);
  const m = Math.floor((secs % 3_600) / 60);
  if (d > 0) return `resets in ${d}d ${h}h`;
  if (h > 0) return `resets in ${h}h ${m}m`;
  return `resets in ${m}m`;
}

export function formatAgo(ts: number | null, now: number): string {
  if (ts === null) return "never";
  const secs = Math.max(0, now - ts);
  if (secs < 60) return "just now";
  if (secs < 3_600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86_400) return `${Math.floor(secs / 3_600)}h ago`;
  return `${Math.floor(secs / 86_400)}d ago`;
}

/** Vendor plan slugs are not presentable as-is. */
export function formatPlan(plan: string | null): string | null {
  if (!plan) return null;
  const known: Record<string, string> = {
    claude_pro: "Pro",
    claude_max: "Max",
    claude_team: "Team",
    claude_enterprise: "Enterprise",
    plus: "Plus",
    pro: "Pro",
    free: "Free",
    business: "Business",
  };
  return known[plan] ?? plan.replace(/[_-]/g, " ");
}

export const severityBar: Record<Severity, string> = {
  normal: "bg-emerald-500",
  warning: "bg-amber-500",
  critical: "bg-rose-500",
};

export const severityText: Record<Severity, string> = {
  normal: "text-emerald-600 dark:text-emerald-400",
  warning: "text-amber-600 dark:text-amber-400",
  critical: "text-rose-600 dark:text-rose-400",
};

/**
 * How the UI explains each confidence level.
 *
 * This is the app's central honesty mechanism: a server-reported quota and a
 * request-count proxy must never look alike.
 */
export const confidenceMeta: Record<
  Confidence,
  { label: string; tone: string; tip: string }
> = {
  authoritative: {
    label: "Reported",
    tone: "bg-emerald-500/12 text-emerald-700 dark:text-emerald-300 ring-emerald-500/25",
    tip: "Read from the vendor's own cached quota — matches what the tool reports itself.",
  },
  derived: {
    label: "Computed",
    tone: "bg-sky-500/12 text-sky-700 dark:text-sky-300 ring-sky-500/25",
    tip: "Summed from a complete local token ledger. Exact for tokens; cost depends on the price table.",
  },
  heuristic: {
    label: "Estimated",
    tone: "bg-amber-500/12 text-amber-700 dark:text-amber-300 ring-amber-500/25",
    tip: "No local token ledger exists for this tool — this counts requests, not billed usage.",
  },
};

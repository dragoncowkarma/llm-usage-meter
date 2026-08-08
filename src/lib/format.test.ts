import { describe, expect, it } from "vitest";

import {
  confidenceMeta,
  formatAgo,
  formatMoney,
  formatPlan,
  formatResetIn,
  formatTokens,
  severityBar,
  severityText,
} from "./format";
import type { Confidence, Severity } from "../types/usage";

describe("formatTokens", () => {
  it("shows small counts verbatim", () => {
    expect(formatTokens(0)).toBe("0");
    expect(formatTokens(999)).toBe("999");
  });

  it("uses one decimal place under 10K, none from 10K up", () => {
    expect(formatTokens(1_000)).toBe("1.0K");
    expect(formatTokens(9_999)).toBe("10.0K");
    expect(formatTokens(10_000)).toBe("10K");
    expect(formatTokens(999_999)).toBe("1000K");
  });

  it("switches to M at one million, B at one billion", () => {
    expect(formatTokens(1_000_000)).toBe("1.00M");
    expect(formatTokens(1_133_057_386)).toBe("1.13B");
  });
});

describe("formatMoney", () => {
  it("uses 2 decimal places at or above $1", () => {
    expect(formatMoney({ amountMinor: 70_195, currency: "USD", exponent: 2 })).toBe("$701.95");
    expect(formatMoney({ amountMinor: 100, currency: "USD", exponent: 2 })).toBe("$1.00");
  });

  it("widens to 4 decimal places under $1 — a cache-heavy day must not read as free", () => {
    // $0.0042 would round to "$0.00" at 2 decimal places, which reads as
    // "no cost" when real spend occurred. This is the case the review
    // flagged explicitly.
    expect(formatMoney({ amountMinor: 42, currency: "USD", exponent: 4 })).toBe("$0.0042");
  });

  it("handles exactly zero without the sub-$1 branch misfiring", () => {
    expect(formatMoney({ amountMinor: 0, currency: "USD", exponent: 2 })).toBe("$0.00");
  });

  it("falls back to a plain string if Intl rejects the currency code", () => {
    // Intl.NumberFormat throws RangeError on a currency it doesn't recognize.
    expect(formatMoney({ amountMinor: 500, currency: "NOTACODE", exponent: 2 })).toBe(
      "5.00 NOTACODE",
    );
  });
});

describe("formatResetIn", () => {
  const now = 1_786_000_000;

  it("returns null when there is no reset time", () => {
    expect(formatResetIn(null, now)).toBeNull();
  });

  it("reports 'resetting' at and past the exact boundary — not '0m' or a negative duration", () => {
    expect(formatResetIn(now, now)).toBe("resetting");
    expect(formatResetIn(now - 1, now)).toBe("resetting");
  });

  it("shows minutes under an hour", () => {
    expect(formatResetIn(now + 5 * 60, now)).toBe("resets in 5m");
  });

  it("shows hours+minutes from one hour up to one day", () => {
    expect(formatResetIn(now + 3600 + 90 * 60, now)).toBe("resets in 2h 30m");
  });

  it("shows days+hours from one day up", () => {
    expect(formatResetIn(now + 2 * 86_400 + 5 * 3600, now)).toBe("resets in 2d 5h");
  });

  it("stays in the hours branch one second before the day boundary", () => {
    // 86399s = 23h 59m — must not round up into the day branch early.
    expect(formatResetIn(now + 86_399, now)).toBe("resets in 23h 59m");
  });
});

describe("formatAgo", () => {
  const now = 1_786_000_000;

  it("reports 'never' for a null timestamp", () => {
    expect(formatAgo(null, now)).toBe("never");
  });

  it("clamps a future timestamp to 'just now' rather than a negative age", () => {
    expect(formatAgo(now + 10, now)).toBe("just now");
  });

  it("steps through the unit boundaries", () => {
    expect(formatAgo(now - 30, now)).toBe("just now");
    expect(formatAgo(now - 90, now)).toBe("1m ago");
    expect(formatAgo(now - 3_600, now)).toBe("1h ago");
    expect(formatAgo(now - 90_000, now)).toBe("1d ago");
  });
});

describe("formatPlan", () => {
  it("returns null for no plan", () => {
    expect(formatPlan(null)).toBeNull();
  });

  it("maps known vendor slugs to a presentable label", () => {
    expect(formatPlan("claude_pro")).toBe("Pro");
    expect(formatPlan("plus")).toBe("Plus");
  });

  it("humanises an unrecognized slug rather than showing it raw", () => {
    expect(formatPlan("some_future_tier")).toBe("some future tier");
  });
});

describe("severity and confidence maps", () => {
  // These are plain lookup tables — the real risk is silently missing an
  // entry (a blank badge/bar in the UI) after adding a new enum variant, so
  // check the map's key set against the type's variants rather than each
  // value individually.
  const severities: Severity[] = ["normal", "warning", "critical"];
  const confidences: Confidence[] = ["authoritative", "derived", "heuristic"];

  it("severityBar and severityText cover every Severity", () => {
    for (const s of severities) {
      expect(severityBar[s]).toBeTruthy();
      expect(severityText[s]).toBeTruthy();
    }
  });

  it("confidenceMeta covers every Confidence with a non-empty label and tip", () => {
    for (const c of confidences) {
      expect(confidenceMeta[c].label).toBeTruthy();
      expect(confidenceMeta[c].tone).toBeTruthy();
      expect(confidenceMeta[c].tip.length).toBeGreaterThan(10);
    }
  });
});

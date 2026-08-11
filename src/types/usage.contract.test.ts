/**
 * The TypeScript half of the Rust ↔ TypeScript shape contract.
 *
 * `usage.ts` is hand-maintained in parallel with `src-tauri/src/model.rs` /
 * `collector.rs` (see the note at the top of that file) — nothing generates
 * one from the other. This test and its Rust counterpart
 * (`src-tauri/src/lib.rs`, `mod schema_drift`) both check their own runtime
 * shape against the same checked-in file, `contract/usage-report-keys.json`,
 * so a rename/add/remove on either side that isn't mirrored in *both* the
 * contract file and the other language's type fails that language's test
 * suite immediately rather than surfacing as `undefined` in the popover.
 *
 * The object literals below are also typed against the real interfaces, so a
 * required field TypeScript added that's missing here fails to *compile*,
 * independent of the runtime key check.
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import type {
  ActivityStats,
  CostEstimate,
  Money,
  ModelUsage,
  ProviderInfo,
  QuotaWindow,
  Settings,
  SourceRef,
  UsageReport,
  UsageSnapshot,
  TokenTotals,
} from "./usage";

const here = dirname(fileURLToPath(import.meta.url));
const contractPath = join(here, "..", "..", "contract", "usage-report-keys.json");
const contract: Record<string, string[] | string> = JSON.parse(readFileSync(contractPath, "utf-8"));

function expectedKeys(typeName: string): string[] {
  const entry = contract[typeName];
  if (!Array.isArray(entry)) {
    throw new Error(`no contract entry for ${typeName} in contract/usage-report-keys.json`);
  }
  return [...entry].sort();
}

/** Direct (non-recursive) own keys — matches the Rust side's `top_level_keys`. */
function actualKeys(value: object): string[] {
  return Object.keys(value).sort();
}

function check(typeName: string, value: object) {
  it(`${typeName} matches the shared contract`, () => {
    expect(actualKeys(value)).toEqual(expectedKeys(typeName));
  });
}

describe("IPC payload shape vs contract/usage-report-keys.json", () => {
  const money: Money = { amountMinor: 0, currency: "USD", exponent: 2 };
  check("Money", money);

  const quotaWindow: QuotaWindow = {
    id: "id",
    label: "label",
    usedPercent: 0,
    resetsAt: null,
    windowMinutes: null,
    severity: "normal",
    isActive: true,
    usedAmount: null,
    limitAmount: null,
  };
  check("QuotaWindow", quotaWindow);

  const tokenTotals: TokenTotals = {
    input: 0,
    output: 0,
    cacheWrite: 0,
    cacheRead: 0,
    reasoning: 0,
  };
  check("TokenTotals", tokenTotals);

  const modelUsage: ModelUsage = {
    model: "m",
    tokens: tokenTotals,
    requests: 0,
    costUsd: null,
  };
  check("ModelUsage", modelUsage);

  const costEstimate: CostEstimate = { amount: money, partial: false, basis: "" };
  check("CostEstimate", costEstimate);

  const activityStats: ActivityStats = {
    requestsToday: 0,
    requestsTotal: 0,
    sessionsToday: 0,
    lastActivityAt: null,
    rateLimitHits: 0,
  };
  check("ActivityStats", activityStats);

  const sourceRef: SourceRef = { kind: "k", path: "p", observedAt: null };
  check("SourceRef", sourceRef);

  const usageSnapshot: UsageSnapshot = {
    providerId: "id",
    displayName: "Name",
    status: "ok",
    confidence: "authoritative",
    plan: null,
    quotas: [],
    tokens: null,
    models: [],
    cost: null,
    activity: null,
    sources: [],
    collectedAt: 0,
    collectMs: 0,
    notes: [],
    error: null,
  };
  check("UsageSnapshot", usageSnapshot);

  const usageReport: UsageReport = {
    snapshots: [],
    generatedAt: 0,
    platform: "macos",
    peakPercent: null,
    appVersion: "0.0.0",
  };
  check("UsageReport", usageReport);

  const providerInfo: ProviderInfo = { id: "id", displayName: "Name" };
  check("ProviderInfo", providerInfo);

  const settings: Settings = {
    refreshIntervalSecs: 600,
    lookbackDays: 30,
    disabledProviders: [],
  };
  check("Settings", settings);

  it("names every type this suite actually checks — no unverified contract entries", () => {
    const checked = [
      "Money",
      "QuotaWindow",
      "TokenTotals",
      "ModelUsage",
      "CostEstimate",
      "ActivityStats",
      "SourceRef",
      "UsageSnapshot",
      "UsageReport",
      "ProviderInfo",
      "Settings",
    ].sort();
    const contractNames = Object.keys(contract)
      .filter((k) => k !== "$comment")
      .sort();
    expect(checked).toEqual(contractNames);
  });
});

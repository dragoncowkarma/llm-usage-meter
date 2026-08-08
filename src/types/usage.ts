/**
 * Mirrors `src-tauri/src/model.rs`.
 *
 * The Rust types are the source of truth; these are hand-maintained to keep the
 * build free of a codegen step. If you change model.rs, change this file in the
 * same commit.
 */

export type Confidence = "authoritative" | "derived" | "heuristic";
export type ProviderStatus = "ok" | "notInstalled" | "degraded" | "error";
export type Severity = "normal" | "warning" | "critical";

export interface Money {
  amountMinor: number;
  currency: string;
  exponent: number;
}

export interface QuotaWindow {
  id: string;
  label: string;
  usedPercent: number;
  resetsAt: number | null;
  windowMinutes: number | null;
  severity: Severity;
  isActive: boolean;
  usedAmount: Money | null;
  limitAmount: Money | null;
}

export interface TokenTotals {
  input: number;
  output: number;
  cacheWrite: number;
  cacheRead: number;
  reasoning: number;
}

export interface ModelUsage {
  model: string;
  tokens: TokenTotals;
  requests: number;
  costUsd: number | null;
}

export interface CostEstimate {
  amount: Money;
  partial: boolean;
  basis: string;
}

export interface ActivityStats {
  requestsToday: number;
  requestsTotal: number;
  sessionsToday: number;
  lastActivityAt: number | null;
  rateLimitHits: number;
}

export interface SourceRef {
  kind: string;
  path: string;
  observedAt: number | null;
}

export interface UsageSnapshot {
  providerId: string;
  displayName: string;
  status: ProviderStatus;
  confidence: Confidence;
  plan: string | null;
  quotas: QuotaWindow[];
  tokens: TokenTotals | null;
  models: ModelUsage[];
  cost: CostEstimate | null;
  activity: ActivityStats | null;
  sources: SourceRef[];
  collectedAt: number;
  collectMs: number;
  notes: string[];
  error: string | null;
}

export interface UsageReport {
  snapshots: UsageSnapshot[];
  generatedAt: number;
  platform: string;
  peakPercent: number | null;
  appVersion: string;
}

export interface ProviderInfo {
  id: string;
  displayName: string;
}

export interface Settings {
  refreshIntervalSecs: number;
  lookbackDays: number;
  disabledProviders: string[];
  showPercentInMenuBar: boolean;
}

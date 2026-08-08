import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  confidenceMeta,
  formatAgo,
  formatMoney,
  formatPlan,
  formatTokens,
} from "../lib/format";
import type { UsageSnapshot } from "../types/usage";
import { AlertIcon, ChevronIcon, FolderIcon } from "./icons";
import QuotaBar from "./QuotaBar";

interface Props {
  snapshot: UsageSnapshot;
  now: number;
}

export default function ProviderCard({ snapshot: s, now }: Props) {
  const [open, setOpen] = useState(false);
  const conf = confidenceMeta[s.confidence];
  const plan = formatPlan(s.plan);

  if (s.status === "notInstalled") {
    return (
      <div className="rounded-xl border border-neutral-200/60 bg-white/40 px-3 py-2.5 dark:border-neutral-700/40 dark:bg-neutral-800/25">
        <div className="flex items-center justify-between">
          <span className="text-[14px] font-medium text-neutral-400 dark:text-neutral-500">
            {s.displayName}
          </span>
          <span className="text-[12px] text-neutral-400 dark:text-neutral-500">
            not installed
          </span>
        </div>
      </div>
    );
  }

  const hasDetail =
    s.models.length > 0 || s.notes.length > 0 || s.sources.length > 0 || s.activity !== null;

  return (
    <div className="rounded-xl border border-neutral-200/70 bg-white/70 shadow-sm dark:border-neutral-700/50 dark:bg-neutral-800/45">
      <div className="px-3 pt-2.5 pb-2">
        <div className="flex items-center gap-2">
          <span className="truncate text-[14px] font-semibold text-neutral-800 dark:text-neutral-100">
            {s.displayName}
          </span>
          {plan && (
            <span className="shrink-0 rounded-md bg-neutral-500/10 px-1.5 py-px text-[11px] font-medium text-neutral-700 dark:text-neutral-300">
              {plan}
            </span>
          )}
          <span
            title={conf.tip}
            className={`ml-auto shrink-0 rounded-md px-1.5 py-px text-[11px] font-medium ring-1 ring-inset ${conf.tone}`}
          >
            {conf.label}
          </span>
        </div>

        {s.status === "error" && (
          <div className="mt-2 flex items-start gap-1.5 rounded-lg bg-rose-500/10 px-2 py-1.5 text-[12px] leading-relaxed text-rose-700 dark:text-rose-300">
            <AlertIcon className="mt-px h-3.5 w-3.5 shrink-0" />
            <span className="break-words">{s.error ?? "Collection failed."}</span>
          </div>
        )}

        {s.quotas.length > 0 && (
          <div className="mt-2.5 space-y-2.5">
            {s.quotas.map((q) => (
              <QuotaBar key={q.id} quota={q} now={now} />
            ))}
          </div>
        )}

        {s.quotas.length === 0 && s.status !== "error" && (
          <p className="mt-2 text-[12px] text-neutral-500 dark:text-neutral-400">
            No quota reported locally.
          </p>
        )}

        {(s.tokens || s.cost) && (
          <div className="mt-2.5 flex items-baseline gap-3 border-t border-neutral-200/60 pt-2 text-[12px] dark:border-neutral-700/40">
            {s.tokens && (
              <span className="tnum text-neutral-600 dark:text-neutral-400">
                {formatTokens(
                  s.tokens.input + s.tokens.output + s.tokens.cacheWrite + s.tokens.cacheRead,
                )}{" "}
                tokens
              </span>
            )}
            {s.cost && (
              <span
                className="tnum ml-auto font-semibold text-neutral-800 dark:text-neutral-100"
                title={s.cost.basis}
              >
                {s.cost.partial ? "~" : ""}
                {formatMoney(s.cost.amount)}
              </span>
            )}
          </div>
        )}
      </div>

      {hasDetail && (
        <>
          <button
            type="button"
            onClick={() => setOpen((v) => !v)}
            className="flex w-full items-center justify-center gap-1 rounded-b-xl border-t border-neutral-200/50 py-1.5 text-[11px] font-medium text-neutral-600 transition-colors hover:bg-neutral-500/5 dark:border-neutral-700/40 dark:text-neutral-400"
            aria-expanded={open}
          >
            {open ? "Hide details" : "Details"}
            <ChevronIcon
              className={`h-3 w-3 transition-transform ${open ? "rotate-180" : ""}`}
            />
          </button>

          {open && (
            <div className="space-y-2.5 border-t border-neutral-200/50 px-3 py-2.5 text-[12px] dark:border-neutral-700/40">
              {s.activity && (
                <div className="tnum flex flex-wrap gap-x-3 gap-y-1 text-neutral-600 dark:text-neutral-300">
                  <span>{s.activity.requestsToday.toLocaleString()} today</span>
                  <span>{s.activity.requestsTotal.toLocaleString()} in window</span>
                  {s.activity.rateLimitHits > 0 && (
                    <span className="text-rose-600 dark:text-rose-400">
                      {s.activity.rateLimitHits} rate-limited
                    </span>
                  )}
                  <span className="ml-auto text-neutral-500 dark:text-neutral-400">
                    last {formatAgo(s.activity.lastActivityAt, now)}
                  </span>
                </div>
              )}

              {s.models.length > 0 && (
                <table className="w-full text-left">
                  <tbody>
                    {s.models.slice(0, 6).map((m) => (
                      <tr key={m.model} className="text-neutral-600 dark:text-neutral-300">
                        <td className="max-w-[150px] truncate py-0.5 pr-2">{m.model}</td>
                        <td className="tnum py-0.5 pr-2 text-right">
                          {formatTokens(
                            m.tokens.input +
                              m.tokens.output +
                              m.tokens.cacheWrite +
                              m.tokens.cacheRead,
                          )}
                        </td>
                        <td className="tnum py-0.5 text-right font-medium text-neutral-800 dark:text-neutral-100">
                          {m.costUsd !== null ? `$${m.costUsd.toFixed(2)}` : "—"}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}

              {s.notes.map((note, i) => (
                <p key={i} className="leading-relaxed text-neutral-600 dark:text-neutral-400">
                  {note}
                </p>
              ))}

              {s.sources.map((src, i) => (
                <button
                  key={i}
                  type="button"
                  onClick={() => void invoke("reveal_source", { path: src.path })}
                  className="flex w-full items-center gap-1.5 truncate text-left text-[11px] text-neutral-500 transition-colors hover:text-sky-600 dark:text-neutral-500 dark:hover:text-sky-400"
                  title={src.path}
                >
                  <FolderIcon className="h-3 w-3 shrink-0" />
                  <span className="truncate">
                    {src.kind}: {src.path}
                  </span>
                </button>
              ))}

              <p className="text-[11px] text-neutral-400 dark:text-neutral-500">
                Collected in {s.collectMs} ms
              </p>
            </div>
          )}
        </>
      )}
    </div>
  );
}

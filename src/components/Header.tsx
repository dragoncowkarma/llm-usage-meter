import { severityText } from "../lib/format";
import type { Severity } from "../types/usage";
import { GearIcon, RefreshIcon } from "./icons";

interface Props {
  peakPercent: number | null;
  loading: boolean;
  settingsOpen: boolean;
  onRefresh: () => void;
  onToggleSettings: () => void;
}

function severityFor(pct: number): Severity {
  if (pct >= 90) return "critical";
  if (pct >= 75) return "warning";
  return "normal";
}

export default function Header({
  peakPercent,
  loading,
  settingsOpen,
  onRefresh,
  onToggleSettings,
}: Props) {
  return (
    <header className="flex items-center gap-2 border-b border-stone-200/60 px-3 py-2.5 dark:border-stone-700/50">
      <div className="min-w-0">
        <h1 className="truncate text-[14px] font-semibold text-stone-800 dark:text-stone-100">
          LLM Usage
        </h1>
        <p className="text-[11px] text-stone-600 dark:text-stone-300">
          {peakPercent === null ? (
            "no quota reported"
          ) : (
            <>
              peak{" "}
              <span className={`tnum font-semibold ${severityText[severityFor(peakPercent)]}`}>
                {peakPercent.toFixed(0)}%
              </span>{" "}
              used
            </>
          )}
        </p>
      </div>

      <div className="ml-auto flex items-center gap-0.5">
        <button
          type="button"
          onClick={onRefresh}
          disabled={loading}
          aria-label="Refresh now"
          title="Refresh now"
          className="rounded-lg p-1.5 text-stone-500 transition-colors hover:bg-stone-500/10 hover:text-stone-800 disabled:opacity-40 dark:text-stone-300 dark:hover:text-stone-100"
        >
          <RefreshIcon className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
        </button>
        <button
          type="button"
          onClick={onToggleSettings}
          aria-label="Settings"
          aria-expanded={settingsOpen}
          title="Settings"
          className={`rounded-lg p-1.5 transition-colors hover:bg-stone-500/10 ${
            settingsOpen
              ? "bg-stone-500/10 text-stone-800 dark:text-stone-100"
              : "text-stone-500 dark:text-stone-300"
          }`}
        >
          <GearIcon />
        </button>
      </div>
    </header>
  );
}

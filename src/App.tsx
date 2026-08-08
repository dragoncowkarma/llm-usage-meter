import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import Header from "./components/Header";
import ProviderCard from "./components/ProviderCard";
import SettingsPanel from "./components/SettingsPanel";
import { PowerIcon } from "./components/icons";
import { formatAgo } from "./lib/format";
import { subscribeToBackend, useUsageStore } from "./store/useUsageStore";

export default function App() {
  const { report, settings, providers, loading, error, now, load, refresh, saveSettings } =
    useUsageStore();
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    void load();
    return subscribeToBackend();
  }, [load]);

  // Esc dismisses the popover, matching every other menu bar app.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") void invoke("hide_popup");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="flex h-full flex-col overflow-hidden rounded-xl border border-black/10 bg-neutral-50/85 backdrop-blur-2xl dark:border-white/10 dark:bg-neutral-900/85">
      <Header
        peakPercent={report?.peakPercent ?? null}
        loading={loading}
        settingsOpen={settingsOpen}
        onRefresh={() => void refresh()}
        onToggleSettings={() => setSettingsOpen((v) => !v)}
      />

      {settingsOpen && settings && (
        <SettingsPanel
          settings={settings}
          providers={providers}
          onChange={(next) => void saveSettings(next)}
        />
      )}

      <main className="scroll-area flex-1 space-y-2 px-3 py-2.5">
        {error && (
          <div className="rounded-lg bg-rose-500/10 px-2.5 py-2 text-[12px] leading-relaxed text-rose-700 dark:text-rose-300">
            {error}
          </div>
        )}

        {!report && !error && (
          <p className="py-8 text-center text-[12px] text-neutral-500 dark:text-neutral-400">
            Reading local usage…
          </p>
        )}

        {report?.snapshots.map((s) => (
          <ProviderCard key={s.providerId} snapshot={s} now={now} />
        ))}

        {report?.snapshots.length === 0 && (
          <p className="py-8 text-center text-[12px] text-neutral-500 dark:text-neutral-400">
            Every tool is switched off in settings.
          </p>
        )}
      </main>

      <footer className="flex items-center gap-2 border-t border-neutral-200/60 px-3 py-1.5 dark:border-neutral-700/50">
        <span className="tnum truncate text-[11px] text-neutral-500 dark:text-neutral-400">
          {report ? `updated ${formatAgo(report.generatedAt, now)}` : "…"}
          {report && ` · v${report.appVersion}`}
        </span>
        <button
          type="button"
          onClick={() => void invoke("quit_app")}
          aria-label="Quit"
          title="Quit"
          className="ml-auto rounded-lg p-1 text-neutral-500 transition-colors hover:bg-rose-500/10 hover:text-rose-600 dark:text-neutral-400 dark:hover:text-rose-400"
        >
          <PowerIcon className="h-3.5 w-3.5" />
        </button>
      </footer>
    </div>
  );
}

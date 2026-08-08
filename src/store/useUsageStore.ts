import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";

import type { ProviderInfo, Settings, UsageReport } from "../types/usage";

interface UsageState {
  report: UsageReport | null;
  settings: Settings | null;
  /** All registered providers, including disabled ones (settings needs those). */
  providers: ProviderInfo[];
  loading: boolean;
  error: string | null;
  /** Local clock, ticked once a second so relative times stay live. */
  now: number;

  load: () => Promise<void>;
  refresh: () => Promise<void>;
  saveSettings: (next: Settings) => Promise<void>;
  tick: () => void;
}

export const useUsageStore = create<UsageState>((set, get) => ({
  report: null,
  settings: null,
  providers: [],
  loading: false,
  error: null,
  now: Math.floor(Date.now() / 1000),

  /** First paint: show the cached report immediately, don't block on a scan. */
  async load() {
    set({ loading: true, error: null });
    try {
      const [report, settings, providers] = await Promise.all([
        invoke<UsageReport>("get_report", { force: false }),
        invoke<Settings>("get_settings"),
        invoke<ProviderInfo[]>("list_providers"),
      ]);
      set({ report, settings, providers, loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  async refresh() {
    set({ loading: true, error: null });
    try {
      set({ report: await invoke<UsageReport>("get_report", { force: true }), loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  async saveSettings(next) {
    try {
      const saved = await invoke<Settings>("save_settings", { settings: next });
      set({ settings: saved });
      // Changing the lookback window changes every number on screen, so
      // re-collect rather than leave stale totals under new settings.
      await get().refresh();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  tick() {
    set({ now: Math.floor(Date.now() / 1000) });
  },
}));

/** Subscribe to background refreshes pushed from the Rust timer. */
export function subscribeToBackend() {
  const unlisten = listen<UsageReport>("usage-updated", (event) => {
    useUsageStore.setState({ report: event.payload, loading: false });
  });
  const timer = setInterval(() => useUsageStore.getState().tick(), 1000);

  return () => {
    void unlisten.then((fn) => fn());
    clearInterval(timer);
  };
}

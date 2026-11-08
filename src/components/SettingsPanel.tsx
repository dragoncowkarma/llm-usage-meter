import type { ProviderInfo, Settings } from "../types/usage";

interface Props {
  settings: Settings;
  /** All registered providers — not just the ones in the current report, so a
   *  disabled provider keeps a control that can re-enable it. */
  providers: ProviderInfo[];
  onChange: (next: Settings) => void;
}

const REFRESH_CHOICES = [
  { label: "1 min", value: 60 },
  { label: "5 min", value: 300 },
  { label: "10 min", value: 600 },
  { label: "30 min", value: 1800 },
];

const LOOKBACK_CHOICES = [
  { label: "7d", value: 7 },
  { label: "30d", value: 30 },
  { label: "90d", value: 90 },
];

function SegmentedControl<T extends number>({
  label,
  value,
  choices,
  onSelect,
}: {
  label: string;
  value: T;
  choices: { label: string; value: T }[];
  onSelect: (v: T) => void;
}) {
  return (
    <div className="flex items-center justify-between gap-2">
      <span className="text-[12px] text-stone-700 dark:text-stone-200">{label}</span>
      <div
        role="radiogroup"
        aria-label={label}
        className="flex overflow-hidden rounded-lg border border-stone-300/70 dark:border-stone-600/60"
      >
        {choices.map((c) => (
          <button
            key={c.value}
            type="button"
            role="radio"
            aria-checked={value === c.value}
            onClick={() => onSelect(c.value)}
            className={`px-2 py-1 text-[11px] transition-colors ${
              value === c.value
                ? "bg-orange-500 text-white"
                : "text-stone-600 hover:bg-stone-500/10 dark:text-stone-200"
            }`}
          >
            {c.label}
          </button>
        ))}
      </div>
    </div>
  );
}

export default function SettingsPanel({ settings, providers, onChange }: Props) {
  const toggleProvider = (id: string, enabled: boolean) => {
    const disabled = new Set(settings.disabledProviders);
    if (enabled) disabled.delete(id);
    else disabled.add(id);
    onChange({ ...settings, disabledProviders: [...disabled] });
  };

  return (
    <div className="space-y-3 border-b border-stone-200/60 bg-stone-500/[0.04] px-3 py-3 dark:border-stone-700/50">
      <SegmentedControl
        label="Auto-refresh"
        value={settings.refreshIntervalSecs}
        choices={REFRESH_CHOICES}
        onSelect={(v) => onChange({ ...settings, refreshIntervalSecs: v })}
      />

      <SegmentedControl
        label="Usage window"
        value={settings.lookbackDays}
        choices={LOOKBACK_CHOICES}
        onSelect={(v) => onChange({ ...settings, lookbackDays: v })}
      />

      <div className="space-y-1.5 border-t border-stone-200/60 pt-2 dark:border-stone-700/40">
        <span className="text-[11px] font-medium tracking-wide text-stone-500 uppercase dark:text-stone-300">
          Tools
        </span>
        {providers.map((p) => (
          <label key={p.id} className="flex cursor-pointer items-center justify-between gap-2">
            <span className="truncate text-[12px] text-stone-700 dark:text-stone-200">
              {p.displayName}
            </span>
            <input
              type="checkbox"
              checked={!settings.disabledProviders.includes(p.id)}
              onChange={(e) => toggleProvider(p.id, e.target.checked)}
              className="h-3.5 w-3.5 accent-orange-500"
            />
          </label>
        ))}
      </div>
    </div>
  );
}

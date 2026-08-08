# Architecture

## Why the split is where it is

Everything that touches the filesystem lives in Rust. The webview has no `fs`,
`shell`, or `http` capability at all — its entire world is six IPC commands
(`capabilities/default.json`). That means every path this app reads is decided
in one auditable place, and a compromised web layer cannot go wandering through
`~/.claude` or `~/.codex`.

That guarantee is enforced server-side, not just by frontend convention. The
one command that opens something outside the app's own state —
`reveal_source`, which reveals a source file in Finder — takes a path string
from the webview, but `AppState::is_known_source_path()` rejects anything
that isn't an *exact* match to a `SourceRef.path` the app itself put in the
most recently collected report (no prefix/substring matching, so
`/known/path/../../etc/passwd` and `/known/path/extra` are both rejected).
The frontend only ever round-trips a path it was handed, so this changes
nothing for legitimate use; it just means the guarantee doesn't rely on the
frontend continuing to behave.

It is also the faster arrangement: scanning hundreds of JSONL transcripts is
I/O-bound work that would be miserable to do across an IPC boundary.

```
┌──────────────────────────── webview (React) ────────────────────────────┐
│  App → Header / SettingsPanel / ProviderCard → QuotaBar                 │
│  zustand store ── invoke() ──┐        ┌── listen("usage-updated")       │
└──────────────────────────────┼────────┼─────────────────────────────────┘
                               ▼        │
┌──────────────────────────── Rust ─────┼─────────────────────────────────┐
│  commands.rs (6 commands)             │                                 │
│       │                               │                                 │
│  collector::AppState ─── refresh() ───┘   tray.rs (icon, title, popup)  │
│       │                                                                 │
│       ├── pricing::PriceTable                                           │
│       ├── platform::HostEnv ──── PlatformPaths (mac / windows / linux)  │
│       └── providers::registry() → Vec<Box<dyn UsageProvider>>           │
│               ├── claude_code   ├── codex   ├── antigravity   └── dummy │
└─────────────────────────────────────────────────────────────────────────┘
```

## Phase 2 — the platform layer

`platform/paths.rs` defines `PlatformPaths` with intent-named methods
(`home`, `config_root`, `data_root`, `cache_root`, `dot_dir`) and one impl per
OS. No provider ever writes `~/.claude` or `%APPDATA%` directly, so the Windows
port is an impl in this module rather than an edit to every provider.

`dot_dir()` exists as its own method on purpose: the Node-based CLIs resolve
`os.homedir()` and keep the `~/.tool` convention on Windows too, which is a
non-obvious assumption worth making greppable.

`first_existing()` handles layout drift — Antigravity alone has used
`~/.antigravity`, `~/.gemini/antigravity`, and `~/.gemini/antigravity-cli`
across releases.

## Phase 3 — the plugin system

```rust
pub trait UsageProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn detect(&self, ctx: &ProviderContext) -> Detection;
    fn collect(&self, ctx: &ProviderContext) -> Result<UsageSnapshot>;
}
```

Adding a tool is: copy `providers/dummy.rs`, implement four methods, add one
line to `registry()`. The collector never knows what it is talking to, and the
frontend only knows `UsageSnapshot`.

`detect()` is separate from `collect()` so an uninstalled tool costs one `stat`
rather than a directory walk.

Providers are run through `collect_one()`, which converts both `Err` and a
panic into an error card. One malformed transcript degrades one row instead of
killing the menu bar app. (This is why the release profile does *not* set
`panic = "abort"`.)

## Confidence — the honesty mechanism

The three tools expose wildly different quality of local data, and rendering a
guess next to a server-reported fact would make the whole app untrustworthy. So
every snapshot carries a `Confidence`, and the UI badges it:

| Level | Badge | Meaning |
|---|---|---|
| `Authoritative` | **Reported** | The vendor's backend computed it; the CLI cached it locally. Matches the tool's own `/usage` or `/status`. |
| `Derived` | **Computed** | We summed a complete local per-request token ledger. Exact for tokens; cost depends on the price table. |
| `Heuristic` | **Estimated** | No local ledger exists. Request/step counts and observed rate-limit errors only. |

## Where each number comes from

| | Quota | Tokens | Cost |
|---|---|---|---|
| **Claude Code** | `~/.claude.json → cachedUsageUtilization` — 5-hour + weekly percentages, reset times, extra-usage credit spend | `~/.claude/projects/**/*.jsonl`, every assistant turn's `message.usage` | computed from tokens × price table |
| **Codex** | rollout `token_count.rate_limits` — `used_percent`, `window_minutes`, `resets_at`, `plan_type` | rollout `token_count.info.total_token_usage` | estimated (OpenAI rates unverified in this build) |
| **Antigravity** | none locally — 429 log observations only | none locally | not offered |

### Claude Code details

* **Dedup.** Resuming or forking a session copies prior turns into a new
  transcript, so the same request appears in several files. We key on
  `message.id` + `requestId` — the pair the API itself assigns — so each billed
  request is counted exactly once. A turn with neither id is skipped:
  undercounting beats double-counting.
* **Cache TTLs.** Transcripts split `cache_creation` into
  `ephemeral_5m_input_tokens` and `ephemeral_1h_input_tokens`, and Anthropic
  prices those at 1.25× and 2× input respectively. We bill them separately
  instead of averaging. Older transcripts carry only the total; that remainder
  is billed at the 5-minute rate rather than dropped.
* **Cost is notional.** On a subscription plan, the dollar figure is what the
  same traffic *would* have cost on the API — not what the user is billed. The
  `basis` string on `CostEstimate` says so, and the UI surfaces it on hover.

### Codex details

`total_token_usage` is cumulative *within a session*, so only the last
`token_count` event per rollout matters. That makes the read cheap: seek to the
end of each file (`util::read_tail`) rather than parsing megabytes, with a
second larger pass only when the first finds nothing.

Codex writes `token_count` events with every rate-limit field `null` on
local-only turns. `RateLimits::has_data()` rejects those — treating a null
payload as "0% used" would be a lie, and the newest *non-null* observation
wins across sessions.

A single session's `total_token_usage` routinely spans several models (a
planner, a coder, an auto-reviewer), and Codex reports tokens per session, not
per model — there is no honest way to split them. `uniform_price()` therefore
only computes a cost when every model seen in the window shares an identical
price on **all four** rate components (input, output, cache write, cache
read), not just input/output — two models can bill input/output the same
while pricing cache differently, and `cost_usd()` applies each model's own
cache multipliers. Otherwise cost is omitted with a note naming the models
seen, rather than guessing.

### Antigravity details

This is the honest-limits case. Its conversation store is SQLite whose payloads
are protobuf blobs carrying timestamps and session ids but **no token counts**,
and its quota manager refreshes from the server at runtime without persisting
the result. So instead of inventing a number:

* steps and conversations from `cache/conversation_metadata.json` (`NumSteps`,
  `UpdatedAt`) — a genuine request-volume proxy;
* `RESOURCE_EXHAUSTED (code 429)` lines in the CLI logs, counted once per
  rejection (`errorreport.go` only, not the several downstream lines each one
  produces).

A quota row appears only when a 429 was observed in the last 15 minutes, and it
is labelled "Quota exhausted (observed)" — a statement about an observation,
not a quota reading.

The `errorreport.go` match is a real dependency on an internal filename of a
tool this project doesn't control — if a future Antigravity release renames
it, the count would silently drop to zero. `scan_logs` guards against that
going unnoticed: if a log file has `RESOURCE_EXHAUSTED` lines but none of them
match the specific marker, it's flagged as `marker_possibly_stale`, and the
snapshot gets a note saying detection may be understated rather than quietly
reporting a clean 0.

## Pricing

`pricing.rs` holds a compiled-in table, overridable by
`~/.config/llm-usage-meter/pricing.json`, so a stale build never silently
reports a wrong bill. Cache traffic is priced as a *multiple* of the base input
rate (0.1× read, 1.25× / 2× write) because that is how the vendors document it
and it stays correct when an input price changes.

Lookup tolerates the version suffixes vendors append
(`claude-haiku-4-5-20251001`), with longest-prefix-wins so `gpt-5-codex` beats
`gpt-5`. An unknown model returns `None`, never `0.0` — a zero would silently
understate a bill, so the UI marks the total partial instead.

## Keeping the Rust and TypeScript shapes in sync

`src/types/usage.ts` mirrors `model.rs`/`collector.rs` by hand — there's no
codegen tying them together, so nothing stops a Rust field being renamed,
added, or removed without the matching edit on the TypeScript side; it would
compile fine on both sides and only fail at runtime as `undefined` in the
popover. `contract/usage-report-keys.json` closes that gap: it's a checked-in,
per-type list of exact field names, and both `src-tauri/src/lib.rs`
(`mod schema_drift`) and `src/types/usage.contract.test.ts` build a real
instance of every IPC type and assert its top-level JSON/object keys match
the file. Change a field on either side without updating the contract file
*and* the other language's type, and that language's test suite fails
immediately. It's a key-*name* check, not a structural type check — a field
changing type while keeping its name isn't caught by this, only by actual
codegen (`ts-rs`/`specta`, not adopted here).

## Menu bar behaviour

* `ActivationPolicy::Accessory` — no Dock icon, no ⌘-Tab entry.
* Template tray icon (`icons/tray-template.png`, 44×44 alpha-only) so macOS
  tints it for light/dark and inverts it on selection.
* Left click toggles the popup, anchored under the icon and clamped to the
  screen so a right-most icon doesn't open a half-offscreen panel. Right click
  opens Refresh / Quit.
* Blur and Esc dismiss it.
* The peak active quota percentage is drawn next to the icon (macOS renders
  tray titles; elsewhere it goes in the tooltip).

## Refresh loop

`periodic_refresh` ticks every 5 seconds and compares elapsed time against the
configured interval (default 10 minutes), so changing the interval in settings
takes effect within one tick rather than after the current long sleep. All
collection happens on a background thread; results reach the UI through the
`usage-updated` event.

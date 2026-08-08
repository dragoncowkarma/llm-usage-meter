# LLM Usage Meter

A macOS menu bar app that shows how much of your AI coding-agent quota you have
left — Claude Code, Codex, and Antigravity in one popover.

Built with Tauri 2 + React + TypeScript + Tailwind. It reads only the local
files those tools already write; it makes no network requests and touches no
credentials.

```
   ⌄ 34%                          ← peak quota, in the menu bar
 ┌──────────────────────────────┐
 │ LLM Usage          ↻   ⚙     │
 │ peak 34% used                │
 ├──────────────────────────────┤
 │ Claude Code   Pro   Reported │
 │ 5-hour session            0% │
 │ ▁▁▁▁▁▁▁▁▁▁▁▁▁▁  resets in 4h │
 │ Weekly (all models)      34% │
 │ ████▁▁▁▁▁▁▁▁▁▁  resets in 2d │
 │ 18.6M tokens          $12.41 │
 ├──────────────────────────────┤
 │ Codex        Plus   Reported │
 │ Weekly limit             95% │
 │ ██████████████▁  resets in 3d│
 ├──────────────────────────────┤
 │ Antigravity        Estimated │
 │ No quota reported locally.   │
 └──────────────────────────────┘
```

## Requirements

- macOS 12.0+
- Rust 1.77+ and Node 18+ (to build)
- Xcode Command Line Tools

> The 12.0 floor comes from the Rust toolchain's prebuilt standard library, not
> from anything in this app. `minimumSystemVersion` in `tauri.conf.json` is set
> to match; lower it only if your toolchain's stdlib actually targets lower, or
> the linker will warn and the claim will be untrue.

## Run it

```bash
npm install
npm run tauri dev
```

## Build a release app

```bash
npm run tauri build
```

The bundle lands in `src-tauri/target/release/bundle/`:

- `macos/LLM Usage Meter.app` — the app
- `dmg/LLM Usage Meter_0.1.0_x64.dmg` — drag-to-install image

> **First build is slow** — Tauri's dependency tree is a few hundred crates, and
> the release profile uses full LTO. If your checkout is on a USB or network
> volume, point the build at local storage:
> `CARGO_TARGET_DIR=~/.cache/llm-usage-meter-target npm run tauri build`

### If the `.dmg` step fails

Tauri's `bundle_dmg.sh` drives **Finder over AppleScript** to lay out the disk
image window. In a terminal without Automation permission for Finder — an SSH
session, a CI runner, or an automated shell — that call times out with
`AppleEvent timed out (-1712)` and the DMG step fails *after* the `.app` has
already been built successfully.

Two ways out. Grant the permission (System Settings → Privacy & Security →
Automation → your terminal → Finder) and re-run; or build just the app and
produce the image without Finder:

```bash
npm run tauri build -- --bundles app
```

```bash
STAGE=$(mktemp -d) && ditto "src-tauri/target/release/bundle/macos/LLM Usage Meter.app" "$STAGE/LLM Usage Meter.app" && ln -s /Applications "$STAGE/Applications" && hdiutil create -volname "LLM Usage Meter" -srcfolder "$STAGE" -ov -format UDZO "LLM Usage Meter.dmg"
```

Stage in a temp dir on your **internal disk** — the `/Applications` symlink
cannot be created on a FAT/exFAT volume.

### Signing

With no Developer ID certificate the bundle is unsigned. Ad-hoc sign it so
macOS will run it locally:

```bash
codesign --force --deep --sign - "src-tauri/target/release/bundle/macos/LLM Usage Meter.app"
```

An ad-hoc signature is enough for your own machine. Distributing to others
needs a Developer ID and notarization.

## Tests

```bash
cd src-tauri && cargo test   # parsers, pricing, platform layer, the shape contract
npm test                      # src/lib/format.ts, the shape contract's TS half
```

Both run in CI on every push and pull request (`.github/workflows/ci.yml`).

The Rust suite covers the parsers against the real on-disk formats:
quota-window shapes, transcript deduplication, cache-TTL splitting, price
lookup, null rate-limit payloads, and the platform path layer for both macOS
and Windows. The frontend suite covers `format.ts`'s date/money edge cases
(the sub-$1 display, the exact-zero "resetting" boundary, day/hour/minute
cutovers).

### Keeping the Rust ↔ TypeScript payload shape in sync

`src/types/usage.ts` is hand-maintained in parallel with
`src-tauri/src/model.rs` / `collector.rs` — nothing generates one from the
other. `contract/usage-report-keys.json` is the guardrail: it's a single
checked-in list of each IPC type's field names, and both
`src-tauri/src/lib.rs` (`mod schema_drift`) and
`src/types/usage.contract.test.ts` assert their own runtime shape against it.
Renaming, adding, or removing a field on either side without updating the
contract file *and* the other language's type fails that language's test
immediately, instead of showing up as `undefined` in the popover. It's a
field-name check, not a full type check — a field changing type while
keeping its name isn't caught; only real codegen (`ts-rs`/`specta`) closes
that gap.

## What it reads

| Tool | Files | What we get |
|---|---|---|
| Claude Code | `~/.claude.json`, `~/.claude/projects/**/*.jsonl` | 5-hour + weekly quota, credit spend, exact token ledger, cost |
| Codex | `~/.codex/sessions/**/rollout-*.jsonl` | quota percentage, reset time, plan, session token totals |
| Antigravity | `~/.gemini/antigravity*/cache/conversation_metadata.json`, `log/*.log` | request counts, observed rate-limit rejections |

Read-only, all of it. No API keys are read, no `auth.json` or OAuth token file
is opened, and nothing leaves the machine.

### A note on what each number means

The app labels every card with how much you can trust it:

- **Reported** — the vendor's own cached quota. Matches what `/usage` or
  `/status` prints in the tool itself.
- **Computed** — summed from a complete local token ledger. Exact for tokens.
- **Estimated** — no local ledger exists for this tool, so it counts requests,
  not billed usage.

Antigravity is always *Estimated*: it keeps no local token or quota ledger, so
token totals and cost are omitted rather than guessed. See
[ARCHITECTURE.md](ARCHITECTURE.md) for why.

Cost on a subscription plan is what the same traffic **would** have cost on the
API — not what you are billed. Hover the figure for the exact basis.

## Settings

Click ⚙ in the popover: auto-refresh interval, usage window (7/30/90 days),
menu bar percentage toggle, and per-tool on/off.

Model prices can be overridden without a rebuild — drop a
`~/.config/llm-usage-meter/pricing.json` like:

```json
{
  "gpt-5-codex": { "input": 1.25, "output": 10.0 }
}
```

Values are USD per million tokens.

## Adding another tool

Copy `src-tauri/src/providers/dummy.rs`, implement four methods, and add one
line to `registry()` in `providers/mod.rs`. Nothing else changes — see
[ARCHITECTURE.md](ARCHITECTURE.md#phase-3--the-plugin-system).

## Windows

Not shipped yet, but the OS-specific bits are already isolated in
`src-tauri/src/platform/`, with `WindowsPaths` implemented and unit-tested. The
remaining work is tray presentation, not data collection.

## License

Apache-2.0 — see [LICENSE](LICENSE).

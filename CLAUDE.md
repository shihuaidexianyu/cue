# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository status

This repo is currently **spec-only**: the sole content is `architecture.md`, the authoritative Product & Architecture Specification (v0.2, written in Chinese) for **CUE**, a lightweight Windows launcher. No code, no Cargo workspace, and no commits exist yet. Read `architecture.md` before writing any code — it is the source of truth; this file only summarizes its binding decisions (§ references point into `architecture.md`).

- **Target platform:** Windows
- **Stack:** Rust + GPUI (Zed's UI framework) + Windows API
- **Product:** `Alt+Space` opens the launcher; plain input searches apps (AppModule). **V1 ships AppModule only** — FileModule (`/` + Everything) is designed (§31–33) but deferred to V1.x pending the third-party dependency decision (§31 note). The search box forces English input (§107); Chinese apps are found via pinyin/initials, so AppModule's pinyin index is the only CJK path. CUE is a **single-instance** app (§113): a second process signals the first to show/focus, then exits.

## Commands

No workspace exists yet. Once scaffolded (Cargo workspace at repo root, crates under `crates/`, all named `cue-*`), the standard workflow is:

- Build: `cargo build`
- Run the launcher: `cargo run -p cue`
- Test everything: `cargo test`
- Test one crate: `cargo test -p cue-module-app`
- Run a single test: `cargo test -p cue-module-app <test_name>` (add `-- --nocapture` for output)
- Lint / format: `cargo clippy --all-targets` / `cargo fmt`

## Architecture

The one rule that governs every placement decision (§3):

> **Core owns how features run; Modules own how features work.**

- **Core** is a thin host runtime: session lifecycle, input routing (it parses *only* the module trigger prefix — `ext:pdf`-style query syntax belongs to modules), module registry, query lifecycle, result state/selection, settings host, usage store. Core must not know what a `.lnk`, pinyin, Everything, a fuzzy score, or app dedup is.
- **Modules** are trusted, built-in, statically linked Rust crates (no plugin system) implementing the `Module` / `LauncherModule` traits. They own all business semantics: search, ranking, icons, launch/open.
- **UI (GPUI)** and **Windows host** (RegisterHotKey, shell execute, monitor placement, single-instance mutex, tray icon §116) are separate crates; modules never touch them.
- **The `cue` binary crate is the orchestrator** (§112): host/UI events → Core state transitions → platform-neutral `CoreEffect { ShowLauncher, HideLauncher, FocusInput }` → executed by `cue-ui` / `cue-windows`. Sole synchronous exception: the injected `apply_hotkey` callback for settings try-apply (§42, §53) — one function, not a HostPlatform trait.

Binding contracts (§86 is the canonical interface):

- Modules hand Core **owned opaque** `ModuleItem` handles: `id: ItemId` + `payload: Arc<dyn Any + Send + Sync>` (§11). Core never calls `downcast_ref` — item lifetime is expressed by `Arc` ownership, so there is no `HashMap<ItemId, Entry>` lookup table, no eviction timing, no UI-thread lock. A Core result struct with `exe` / `file_path` / `url` fields is explicitly forbidden (§12).
- Modules never call back into Core (`close_launcher()` etc.). They return `ModuleOutcome { status, session: Close | KeepOpen, usage }` and Core acts on it (§21–23).
- Exactly one default module (App, no prefix); triggers must not conflict; no-prefix input always means App (§82). **AppModule is a required module in V1 — it cannot be disabled** (§65).
- Modal isolation: each module returns only its own result type. No universal mixed result list, no cross-module ranking (§83).
- Composition root: only the top-level `cue` crate names concrete modules. `cue-core` must never `use cue_module_app::...` (§70–71). Dependency direction: `cue → {cue-core, cue-ui, cue-windows, cue-module-*}`, `cue-module-* → cue-protocol`, `cue-core → cue-protocol`, `cue-ui → {cue-core, cue-protocol}`. The registry stores `Box<dyn LauncherModule>` directly (no trait-upcasting problem, §5.3).
- Async model (§91–106): Core is a single-threaded state machine on the UI thread. North star (§91): **Core never cancels async work; it judges result validity via `QueryTicket { session_id, module_id, module_epoch, generation }`** — modules bound their own resource use (Everything: dedicated IPC thread + latest-wins slot, §99). `generation` is Core bookkeeping and is *not* echoed by modules: `QueryResponse` carries only `items`; the ticket is captured by Core's spawn wrapper and completions re-enter through one event queue (§96). Input change clears results + selection immediately — stale results must never be activatable (§102). Activation outcomes: usage is always recorded, but session disposition applies only if the originating session is still current (§103). Futures are `Send + 'static`, created without blocking/IO, polled by an injected `TaskSpawner` (GPUI in production, manual pump in tests). No debounce, no loading state, **no `catch_unwind` panic boundary** (§104 — §63 discipline instead).
- Presentation (§13–17, §108–109): protocol types use `Arc<str>`, never GPUI's `SharedString` (§71). `ResultIcon` is a protocol-owned `Raster` bitmap: RGBA8, row-major, sRGB, straight alpha, `len == w*h*4`, single 96px size (UI downscales and converts at texture upload). UI caches GPU textures by `Arc` pointer — a module must reuse the same `Arc<IconImage>` per cached icon. Result Row is one fixed grid with optional slots (icon gutter always reserved) — no second layout. Modules push `ModuleEvent::PresentationInvalidated { items }` via `ModuleContext.events` (sink bound to `ModuleEpoch` at load; stale-epoch events dropped) when async resources (icons) arrive; Core re-runs `present()` on the visible rows.

Planned workspace layout (§68): `crates/{cue, cue-core, cue-protocol, cue-ui, cue-windows, cue-module-app}` (+ `cue-module-file` in V1.x). The `cue-` prefix avoids collisions with Rust's `core` and the official `windows` crate.

Settings and storage:

- Settings namespaces: `core.*` and `module.<module-id>.*`. Modules declare a schema (each `SettingSpec` carries an `apply_policy`, §38); Core renders all settings UI (§41). Settings live only in the Settings Host, never in module storage (§48).
- **Settings changes are transactional** (§42): validate → try-apply (`Module::try_apply_settings`, or the `apply_hotkey` host callback for `core.hotkey`) → commit in-memory → persist. Failure: no commit, UI restores the old value. `RestartApplication` class: validate → commit → persist → mark `restart_required`.
- Usage store is **aggregated** (§50): `UsageStat { count, last_used }` keyed by `(ModuleId, ItemKey, ActionId)` — no unbounded event log. `item_key` is a stable launch identity (§51): AUMID for packaged apps, canonical exe + normalized args for Win32. Persisted as `<storage_root>/usage.tsv` (header-versioned, whole-file rewrite on each record — bounded and crash-safe); corrupt lines are skipped, never panic.
- Module storage root: `%LOCALAPPDATA%\CUE\` with `modules/<id>/{data,state,cache}` — data (durable user data), state (recoverable), cache (freely deletable) (§43–47).

## Hard "do not build" list (§26, §69, §76)

Do not create, even as scaffolding: plugin-sdk / plugin-runtime / dynamic loading / DLL ABI / WASM / module sandbox; universal-search abstractions (`SearchDocument`, `UniversalRanker`, `CandidateProvider`, `UniversalIndex`, search/ranking strategy frameworks); a native filesystem indexer (file search goes through Everything); JSON-RPC; clipboard manager; calculator; shell runner; network search; AI features. Also absent by design in V1: query cancellation tokens, a panic boundary, loading indicators, app-catalog file watchers.

The internal Rust traits are an internal architecture boundary, **not** a third-party plugin ABI (§67).

## Working rules

- **Anti-over-abstraction is a product requirement.** Rule of Three: write it inline the first time, allow duplication the second, extract a shared util only the third — and shared code sinks *downward* into a util crate, never up into Core (§72–73).
- Unsure where logic belongs? Apply §87: feature semantics (pinyin, `.lnk`, Everything, AUMID, file paths) → module; needed by several modules → duplicate first, then shared util; controls launcher lifecycle (window, session, routing, settings, usage, query) → Core.
- Modules share Core's process: never `unwrap()` external data; IO / Windows API failures must return `ModuleError`, not panic (§63).
- App discovery in V1 = User/Common Start Menu + UWP/MSIX only; do not scan the disk for `.exe` (§29). Packaged apps via `PackageManager` → `GetAppListEntriesAsync()` → `AppListEntry` (Package ≠ App), launched via `IApplicationActivationManager::ActivateApplication(AUMID)` — never AppsFolder enumeration (§29). Dedup by launch semantics (AUMID; canonical exe + normalized args) — prefer duplicates over aggressive dedup (§30). App catalog refreshes at process start only — no watchers; newly installed apps appear after a CUE restart (§56). Per the §56 spike (cold discovery ≈ 6.7 s), catalog build runs on the module's own thread — `load()` stays cheap, query futures wait on a one-shot readiness gate, no disk cache.
- Performance budgets (§55, §77–79, measured per the contract in §114): hotkey → usable < 100 ms (ideal < 50 ms, WM_HOTKEY → first keystroke accepted); cold start < 500 ms (process entry → hotkey registered + ready); app search P50 < 5 ms / P95 < 15 ms (InputChanged → ResultState committed, excluding GPU present); resident memory < 100 MB (Private Working Set, idle 60 s). Never do app discovery, registry queries, icon extraction, or Everything init on the wake hot path (§55).
- No platform-specific code in `cue-core` / `cue-protocol` without demonstrated benefit (§110–111): no `std::os::windows`, `windows`-crate types, or Win32 calls — grep-checkable in CI; burden of proof is on whoever introduces it. This also covers **`core.*` setting values**: they must be OS-neutral data (e.g., `Hotkey { modifiers, key }`, never Win32 `MOD_*` / `VK_*` constants, §53); platform translation lives in the host. Platform code is quarantined in `cue-windows/` and module internals. Cross-platform (macOS) is architecturally unblocked but deliberately un-abstracted until a real second-platform effort starts.
- UX invariants (§115): empty query shows usage Top Apps; new non-empty results select row 0; input change clears results/selection; failed activation keeps the session open with an error; Unicode paste works even though IME composition is disabled; `core.hide_on_focus_loss` governs focus-loss hiding. The tray icon (§116) is the only persistent presence signal and V1's only quit path (left-click = show, right-click menu = Show/Quit). IME: forcing English on wake must be paired with restoring the user's previous layout before hide (§107).

## Development order (§88)

Follow the phases strictly — later phases assume earlier ones exist:

1. **Shell** — Alt+Space, GPUI window, input, Esc/Enter/↑↓, **single instance**, driven by a fake module. Includes two spikes: IME forcing approach (§107 — the Win32 calls are candidates to validate, GPUI maintains its own IME state) and `AppModule::load` cold-start timing (§56).
2. **Module Protocol** — `Module` / `LauncherModule` traits, registry, routing, presentation, action, outcome; validate with a DemoModule that proves Core has no business coupling.
3. **AppModule** — Start Menu + packaged app discovery (AppListEntry path — §29), launch, icons, fuzzy matching, pinyin (full + initials), ranking.
4. **Usage** — frequency/recency recording feeding ranking.
5. **FileModule** — `/` trigger, Everything integration, open file/folder. **Deferred out of V1** (§31); V1 = phases 1–4 + 6.
6. **Settings** — schema-driven settings UI with transactional apply (§42), only after real module settings are known. Do not build a settings framework in Phase 1.

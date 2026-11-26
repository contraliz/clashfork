# Clashfork contributor guidance

Clashfork is a Windows Tauri + React client for stock Mihomo. Keep the renderer focused on
app-owned state and leave routing, DNS, TUN, credentials, and service lifecycle in the Rust
backend.

## Shared UI primitives

Use the existing primitives in `src/components/ui.tsx` rather than adding page-local shells:

- `Panel` / `PanelHeading` for every content surface and its title/actions row.
- `Grid` / `GridItem` for page layout. The grid is 12 columns with a 16px gutter; use spans
  3/4/5/7/9/12. All spans stack to 12 columns at 1080px; the 180px sidebar becomes a 62px
  icon rail at 900px.
- `Toolbar` for search, filters, and range/action rows.
- `Button`, `IconButton`, `Toggle`, `Field`, `Progress`, `SourceBadge`, `Banner`, `Modal`,
  `EmptyState`, and `PageHeader` for the remaining controls.
- Tables use the shared dark table pattern (`.node-table`, `.connections-table`,
  `.history-table`) inside `.table-wrap` / `.node-table-wrap` so narrow layouts scroll inside
  the panel instead of overflowing the window.

## Design tokens and density

Tokens live at the top of `src/styles.css`. Do not hard-code page colors when a token exists.

- Background `--bg` `#101318`; panels `--panel` `#171c24`; borders `--border`/`--border-strong`.
- Accent, active, focus, and connection states use `--accent` cool blue `#78a9ff`.
- Upload/upload chart states use `--violet` `#b99bff`.
- Errors use `--danger`; warnings/stale states use `--warning`. **No green in visible UI**,
  including the logo, badges, progress bars, charts, selection, and service-ready states.
- Keep density compact: 32–36px controls, roughly 36px table rows, 12–16px gutters, 10–14px
  panel type, and 26px page titles. Preserve readable contrast and a visible `:focus-visible`
  outline.
- Dashboard stacks the VPN, live traffic, and run configuration sections vertically at every
  width. Within the VPN panel, keep status and toggle together above the one-row mode/destination
  controls. Keep sections full-width, form controls bounded, and the live chart faithful to its
  sampled time range. Traffic History stacks its transfer summary above a full-width chart,
  then the breakdown and table. Keep the date/actions/filters together as a compact toolbar
  panel. Rulesets may use an adaptive split when both sides retain usable width.

## Behavior and data boundaries

- Keep features, labels, and existing workflows. Do not invent actions or demo data in the
  production UI. The `?mock=1` path is development-only and lives in `src/mock.ts`.
- Diagnostics stay bounded and redacted, newest first. The backend assigns monotonic IDs and
  sorts snapshots by time then ID; the UI also sorts defensively. Do not add auto-scroll to the
  bottom of a newest-first log.
- Traffic History keeps range, breakdown, route, and app/group filters independent. `All` keeps
  authoritative totals, `Unattributed`, and recording gaps; filtered reports use attributed
  samples only, compose filters with AND, and bound per-minute estimates against recorded
  totals. App choices come from the selected range even when active filters return no rows.
  Keep download/upload subtotals. Each period chart has stable left Proxy and right Direct lanes;
  only stored attributed samples in those two route classes count, and combined lane estimates
  stay within minute totals. Blocked, unknown, and unattributed samples are never assigned to
  either lane. Keep each period's mouse/keyboard date, both route totals, and gap details
  inspectable. These semantics and log route labels are part of the product contract.
- Dashboard `Proxy traffic` uses per-flow deltas only when Mihomo's chains resolve to a configured
  Clashfork proxy node. Direct, blocked, and unknown chains are excluded; never derive proxy
  throughput as a residual from whole-core totals. Live Connections and Traffic History retain all
  route classes. Live sampling is periodic, so flows shorter than the sampling interval may not be
  represented. A failed sample clears current live rates and connections and records a history gap
  without changing the selected route or disconnecting the VPN.
- Subscription refresh downloads outside the VPN apply lock, serializes refreshes per service, and
  rebases results against current state before commit. Keep retries bounded and limited to
  transient failures. Retain last-known-good nodes and quota after failed refreshes. Reconcile exact
  source IDs first; endpoint-rotation fallback is allowed only for a unique same-service,
  display-name, and protocol match. Ambiguous matches remain distinct. A referenced node's first
  successful omission is pending confirmation; only a second successful omission marks it missing.
  Refreshing a selected node never silently changes the running core or selected route; report
  when the saved endpoint takes effect after reconnect.
- A transient refresh failure keeps cached node/quota data and may mark it stale internally, but
  should not show a red error or warning-level diagnostic when usable nodes remain. Surface only
  provider access rejection, no available nodes, confirmed active-node disappearance, and local
  persistence failures as actionable errors. Clear legacy transient error text at startup.
- Node latency checks are explicitly user-triggered from Nodes; opening, selecting, refreshing,
  or polling never starts a probe. A manual sweep tests available definitions with at most four
  concurrent requests, skips confirmed missing nodes, and can be cancelled. Tests use the exact
  loaded proxy through the authenticated local controller and a fixed HTTPS 204 probe; they do not
  alter routing or auto-select a node. Keep only milliseconds and measurement time in the
  protected store, bound it to a fingerprint of the exact loaded proxy definition, show unknown
  for failures, and strike only the numeric result at 15 minutes or later.
- Rulesets uses one compact ordered rule editor with the selected ruleset dropdown at the top and
  New ruleset in the page header. Each rule owns its named OR conditions and destination; the
  final Everything else catch-all remains fixed last. Use one destination selector with Direct,
  Block, and available local nodes for both rules and the fallback. Preserve unsaved drafts while navigating
  rulesets. Schema 2 migrates legacy match groups into independent conditions on each referencing
  rule and retains the old groups in storage. Import/export supports versioned local JSON
  documents for one rule or a whole ruleset; imported proxy destinations require an explicit
  local node mapping and remain drafts until Save.
- VPN enable validates each generated config once, reuses the successful service compatibility
  probe for the authenticated start, and checks readiness through the controller mode endpoint
  under one bounded deadline. Keep preflight cancellable and emit redacted stage durations for
  config validation, authenticated service start, mode readiness, and total startup time.
- The active-tunnel warning is a read-only best-route advisory on Windows. It must not change
  adapters, routes, DNS, proxy settings, or firewall policy, and indeterminate route lookups should
  produce no warning.
- Never expose subscription URLs, node credentials, profile data, or secrets in renderer
  state, logs, screenshots, fixtures, or diagnostics. Do not change OS proxy, DNS, firewall, or
  routes outside the service-owned TUN lifecycle.
- The service-owned Mihomo process may be primed as an inert standby at startup. Keep
  `vpnActive` false while it is idle: TUN, DNS, and traffic listeners must be disabled. On
  connect, stage/reload the selected runtime into that same owned session and report Connected
  only after the controller confirms the requested mode/TUN state and Windows best-route checks
  confirm the Clashfork adapter owns representative public routes (or the route API is
  explicitly indeterminate). On disconnect, retain the core only after controller and route
  checks confirm TUN is off; otherwise stop the owned session. Persist stop proofs before starts,
  preserve them when cleanup fails, and never discard an uncertain service session.
- Generated Mihomo config is app-owned. DNS uses the shared unproxied DoH pool for DIRECT and
  proxy bootstrap, while general DNS can carry the explicit `#GLOBAL` routing suffix. Preserve
  fake-IP/TUN settings, ruleset ordering, final `MATCH`, and user state.

## Verification

Run from the repository root:

```powershell
npm run build:web
cargo fmt --check
cargo test --locked -p clashfork --lib
cargo test --locked -p clashfork --lib -- --ignored stock_mihomo_
```

For browser QA, start `npm run dev -- --host 127.0.0.1`, then run `npm test` (or
`CLASHFORK_QA_SCREENSHOTS=1 npm test`). The script checks all five screens at 800x640,
1280x800, 1920x1000, and 2560x1375; diagnostics ordering; history filter accounting and chart
inspection; keyboard focus; overflow; and console errors. Set
`CLASHFORK_QA_BROWSER` if Chrome/Edge is not in its default location. Browser QA uses a
development-only IPC shim; it does not verify TUN, DNS, or the packaged WebView.

For changes that affect visible layout or density, also open the finished UI in a native
WebView2 window and inspect the affected screens at a normal, wide, and 800x640 size. Do not
launch a second installed Clashfork while one is running: it owns the global singleton and
privileged service. Use a side-effect-free fixture launcher with a separate WebView2 data
directory and no service setup instead, then state what was actually observed and where the
native screenshots are stored. Browser-only screenshots do not substitute for this check.

Build the installer with `npm run build` (or the documented release build). Do not commit
`target/`, `dist/`, downloaded binaries, or other local runtime artifacts.

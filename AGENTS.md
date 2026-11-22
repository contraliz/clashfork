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
  Keep download/upload subtotals and each period's mouse/keyboard date, byte counts, and gap
  details inspectable. These semantics and log route labels are part of the product contract.
- Never expose subscription URLs, node credentials, profile data, or secrets in renderer
  state, logs, screenshots, fixtures, or diagnostics. Do not change OS proxy, DNS, firewall, or
  routes outside the service-owned TUN lifecycle.
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

Build the installer with `npm run build` (or the documented release build). Do not commit
`target/`, `dist/`, downloaded binaries, or other local runtime artifacts.

# Clashfork

Clashfork is a Windows 11 desktop client for [Mihomo](https://github.com/MetaCubeX/mihomo/tree/v1.19.34), forked from [Clash Verge Rev v2.5.10](https://github.com/clash-verge-rev/clash-verge-rev/tree/v2.5.10). It uses a custom Tauri and React interface and a separately named Windows TUN service based on [Clash Verge Service IPC v2.7.7](https://github.com/clash-verge-rev/clash-verge-service-ipc/tree/v2.7.7).

## Connect

1. Open **Nodes → Add Nodes**, paste a subscription or supported node link, and review the import preview. Refresh a service to update its nodes and provider-reported allowance.
2. Use **Rulesets** to create match groups and decide where each group routes. Set **Miscellaneous** to Direct or Block.
3. On the **Dashboard**, choose a ruleset in Rule mode or a node in Global mode. Turn the VPN on. Both modes use TUN. Closing the window keeps Clashfork in the tray; use **Quit Clashfork** in the tray to stop the VPN and exit.
4. Switch rulesets on the Dashboard while connected to apply a new routing policy. The VPN stays on when the change succeeds.

**Live Connections** shows current flows. **Logs → Traffic History** keeps sampled app and route usage on this PC. Provider allowance comes from subscription metadata and updates at refresh.

## Build on Windows x64

Install Node.js, Rust via rustup, and Visual Studio Build Tools with the Desktop development with C++ workload. Rustup selects the version in [rust-toolchain.toml](rust-toolchain.toml).

```powershell
npm ci
npm run build
```

The build stages the pinned, checksum-verified Mihomo executable and builds the three Clashfork service binaries before Tauri packages the installer. The expected artifact is `target/release/bundle/nsis/Clashfork_1.0.2_x64-setup.exe`. Installing the bundle requires administrator access to register the fork-owned TUN service.

## Data and security boundaries

- App data lives in `%LOCALAPPDATA%\Clashfork`. Subscription URLs and node credentials are stored
  only in `secrets.dat`, encrypted with current-user Windows DPAPI and created with a current-user
  ACL. They are never returned to the renderer or written to diagnostics.
- The generated runtime contains only app-owned nodes, an app-owned `GLOBAL` group, fixed
  app-generated rules, DNS, and TUN settings. Remote profile rules, groups, DNS, scripts,
  controllers, ports, and providers are ignored. Only supported node definitions are imported.
- `find-process-mode: always` is set even when no process rules exist. The TUN adapter is
  `ClashforkTUN`; TUN owns normal IPv4/IPv6 Internet routing, DNS is hijacked, and its route
  exclusions are loopback, link-local, and private LAN ranges.
- No fork code changes the Windows system proxy, firewall, DNS, or routes outside the
  service-owned Mihomo TUN lifecycle.
- Traffic history is local SQLite. It stores minute totals and app/route attribution only, not
  destinations or payloads. It is retained for 30 days, can be paused, and requires confirmation
  to clear.

## Verify

Run the fast checks:

```powershell
npm run build:web
cargo test --locked -p clashfork --lib
```

Run the real pinned-core check. It starts the bundled Mihomo process on loopback with TUN disabled
and reloads generated rule policy through the core controller; it does not modify host networking:

```powershell
cargo test --locked -p clashfork --lib -- --ignored stock_mihomo_
```

For the browser workflow check, start `npm run dev -- --host 127.0.0.1` in one terminal and run
`npm test` in another. This exercises all five screens, enables the fixture VPN, switches a
ruleset while connected, imports/previews a fixture subscription, opens Connections and history,
checks diagnostics, and verifies a narrow viewport has no horizontal overflow. Set
`CLASHFORK_QA_BROWSER` for a non-default Chrome/Edge path. Set `CLASHFORK_QA_SCREENSHOTS=1` to
write screenshots to a temp directory. This browser run uses a development-only IPC shim; it is
not a substitute for the packaged WebView plus service/TUN acceptance.

## Live acceptance runbook

Use an isolated Windows 11 test machine with a disposable subscription. Install the NSIS bundle as
administrator and verify the separate Clashfork Service exists; do not touch an existing Clash Verge
installation.

1. Confirm the first launch shows VPN off. Check Windows proxy settings before and after every step.
2. Import the synthetic/test subscription. Confirm 69 regular nodes and no notice aliases, quota
   metadata, editable service names, safe refresh failures, and missing-reference warning behavior.
3. Create two matching groups with OR conditions (including process name, process path, domain
   suffix, IPv4 CIDR, and IPv6 CIDR) and two rulesets with different destinations.
4. Turn VPN on in Rule mode. Confirm `ClashforkTUN` is present, DNS works, IPv4 and IPv6 traffic
   traverse the selected route, and no direct path exists for a matched node whose route cannot
   carry UDP.
5. Switch rulesets from Dashboard while connected. Confirm the TUN stays on, the new policy is
   authoritative, existing flows close, rates continue under the new route, and deliberately
   invalid active edits show an error while preserving the prior policy.
6. Exercise Global mode with a single non-DIRECT node, save/recall a named run configuration from
   Dashboard and tray, then disconnect. Confirm the owned TUN adapter disappears.
7. Inspect Traffic History Today/7/30 views, gaps, transfer totals, app/route breakdowns, custom
   group reclassification, pause, clear, and 30-day pruning. Inspect Diagnostics redaction.
8. Reboot/disconnect recovery, restart the app, then uninstall. Confirm the fork service and
   `ClashforkTUN` are removed, system proxy settings remain unchanged, and unrelated VPN/Clash
   installations are untouched.

Clashfork and the vendored service retain their upstream GPL license notices; see `NOTICE.md`.
The installer bundles the official Mihomo core from the linked release.

# NOTICE

Clashfork is an independent Windows client fork. It is licensed under the GNU General Public
License, version 3 only (`LICENSE`).

## Upstream source

- Base application: [Clash Verge Rev](https://github.com/clash-verge-rev/clash-verge-rev) release
  `v2.5.10`, source commit
  [`c83f7ad529c7c9cb3d2d54a0955ef8c1b22a411b`](https://github.com/clash-verge-rev/clash-verge-rev/commit/c83f7ad529c7c9cb3d2d54a0955ef8c1b22a411b).
  The Git history and GPL license are preserved in this repository.
- Windows service client and helper lineage:
  [Clash Verge Service IPC v2.7.7](https://github.com/clash-verge-rev/clash-verge-service-ipc/tree/v2.7.7),
  vendored under `vendor/clashfork-service-ipc` and renamed for the Clashfork service namespace.
  Its GPL license is retained in that directory and copied to `resources/licenses`.
- VPN core: [MetaCubeX/mihomo v1.19.34](https://github.com/MetaCubeX/mihomo/tree/v1.19.34).
  The Windows amd64 v2 release asset is
  `mihomo-windows-amd64-v2-v1.19.34.zip`, official SHA-256
  `25f0d3444631c68cf8fcec7a81a65d5e3cba77cfe3554d92f9609c6e2c1285bb`.
  Only the unmodified `mihomo-windows-amd64-v2.exe` is bundled and renamed
  `clashfork-core.exe`. Its binary SHA-256 is recorded in
  `src-tauri/resources/MIHOMO_CORE_SHA256.txt` and checked by the build/install flow.

## Original portions

The five-screen React interface, local route/preset compiler, subscription sanitizer and
allowance handling, SQLite traffic history, DPAPI-backed local credential storage, diagnostics,
tray integration, and the Clashfork-specific Rust service adapter are original to this fork.

The product, identifier, AppData directory, service name, IPC pipe, TUN adapter, controller
settings, bundles and installer identity are distinct from Clash Verge Rev. No upstream
auto-updater, telemetry, WebDAV, profile scripting, remote rule providers, or upstream UI is
active in this fork.

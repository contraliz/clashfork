import { createHash } from 'node:crypto';
import { createReadStream, createWriteStream } from 'node:fs';
import { chmod, copyFile, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createGunzip } from 'node:zlib';
import { pipeline } from 'node:stream/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const resources = path.join(repo, 'src-tauri', 'resources');
const service = path.join(repo, 'vendor', 'clashfork-service-ipc');
const target = process.platform === 'darwin'
  ? (process.arch === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin')
  : 'x86_64-pc-windows-msvc';

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: repo, stdio: 'inherit', ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} failed with exit code ${result.status}`);
}

if (process.platform === 'win32') {
  run('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(repo, 'scripts', 'prepare-bundle.ps1')]);
} else if (process.platform === 'darwin') {
  await prepareMac();
} else {
  throw new Error(`Bundle preparation is not configured for ${process.platform}.`);
}

async function prepareMac() {
  const variants = {
    arm64: {
      asset: 'mihomo-darwin-arm64-v1.19.34.gz',
      sha256: '1a7a20eca63fbbaec90137717465bfde488d0db2a3a79b8fadebf9b2545b6e4c',
    },
    x64: {
      asset: 'mihomo-darwin-amd64-v1.19.34.gz',
      sha256: '089b033be801ff365b6558851115238b443b237433aa0841bc3fa41ad75a3927',
    },
  };
  const variant = variants[process.arch];
  if (!variant) throw new Error(`Unsupported macOS architecture: ${process.arch}`);
  const buildDir = path.join(repo, 'target', 'service', `mihomo-${process.arch}`);
  const archive = path.join(buildDir, variant.asset);
  const binary = path.join(resources, 'clashfork-core');
  await mkdir(buildDir, { recursive: true });
  let digest = await readFile(archive).then((bytes) => createHash('sha256').update(bytes).digest('hex')).catch(() => '');
  if (digest !== variant.sha256) {
    await rm(archive, { force: true });
    const response = await fetch(`https://github.com/MetaCubeX/mihomo/releases/download/v1.19.34/${variant.asset}`, { redirect: 'follow' });
    if (!response.ok || !response.body) throw new Error(`Pinned Mihomo download failed (${response.status}).`);
    await pipeline(response.body, createWriteStream(archive));
    digest = createHash('sha256').update(await readFile(archive)).digest('hex');
  }
  if (digest !== variant.sha256) throw new Error(`Pinned Mihomo archive SHA-256 mismatch (${digest}).`);
  const gunzip = createGunzip();
  await pipeline(createReadStream(archive), gunzip, createWriteStream(binary));
  await chmod(binary, 0o755);

  for (const name of ['clashfork-core.exe', 'clashfork-service.exe', 'clashfork-service-install.exe', 'clashfork-service-uninstall.exe']) {
    await rm(path.join(resources, name), { force: true });
  }
  const rustVersion = spawnSync('rustc', ['-vV'], { encoding: 'utf8' });
  if (rustVersion.status !== 0 || !rustVersion.stdout.includes(`host: ${target}`)) {
    throw new Error(`Build Clashfork with the ${target} Rust toolchain.`);
  }
  const targetDir = path.join(repo, 'target', 'service');
  run('cargo', [
    'build', '--locked', '--release', '--target', target, '--target-dir', targetDir,
    '--manifest-path', path.join(service, 'Cargo.toml'), '--features', 'standalone,client',
    '--bin', 'clashfork-service', '--bin', 'clashfork-service-install', '--bin', 'clashfork-service-uninstall',
  ]);
  for (const name of ['clashfork-service', 'clashfork-service-install', 'clashfork-service-uninstall']) {
    const source = path.join(targetDir, target, 'release', name);
    const destination = path.join(resources, name);
    await copyFile(source, destination);
    await chmod(destination, 0o755);
  }
  const licenses = path.join(resources, 'licenses');
  await mkdir(licenses, { recursive: true });
  await copyFile(path.join(repo, 'LICENSE'), path.join(licenses, 'Clashfork-GPL-3.0.txt'));
  await copyFile(path.join(service, 'LICENSE'), path.join(licenses, 'Clashfork-Service-GPL-3.0.txt'));
  const uninstall = path.join(resources, 'Uninstall Clashfork Service.command');
  await writeFile(uninstall, '#!/bin/bash\nset -euo pipefail\n"$(dirname "$0")/clashfork-service-uninstall" --prepare-uninstall\n');
  await chmod(uninstall, 0o755);
}

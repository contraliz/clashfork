$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$resources = Join-Path $repo 'src-tauri/resources'
$build = Join-Path $repo 'target/service'
$target = 'x86_64-pc-windows-msvc'
$service = Join-Path $repo 'vendor/clashfork-service-ipc'
$core = Join-Path $resources 'clashfork-core.exe'
$coreHash = (Get-Content -LiteralPath (Join-Path $resources 'MIHOMO_CORE_SHA256.txt') -Raw).Trim()
$compiler = & rustc -vV
if ($LASTEXITCODE -ne 0 -or "host: $target" -notin $compiler) {
    throw 'Build Clashfork with the Windows x64 MSVC Rust toolchain.'
}

function Get-Sha256([string] $path) {
    $stream = [System.IO.File]::OpenRead($path)
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    try {
        return [BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-', '')
    }
    finally {
        $algorithm.Dispose()
        $stream.Dispose()
    }
}

if (!(Test-Path -LiteralPath $core) -or (Get-Sha256 $core) -ne $coreHash) {
    $archive = Join-Path $build 'mihomo-windows-amd64-v2-v1.19.34.zip'
    $archiveHash = '25f0d3444631c68cf8fcec7a81a65d5e3cba77cfe3554d92f9609c6e2c1285bb'
    $url = 'https://github.com/MetaCubeX/mihomo/releases/download/v1.19.34/mihomo-windows-amd64-v2-v1.19.34.zip'
    New-Item -ItemType Directory -Force -Path $build | Out-Null
    if (!(Test-Path -LiteralPath $archive) -or (Get-Sha256 $archive) -ne $archiveHash) {
        & curl.exe --fail --location --silent --show-error --retry 2 --output $archive $url
        if ($LASTEXITCODE -ne 0) { throw 'Downloading the pinned Mihomo archive failed.' }
    }
    if ((Get-Sha256 $archive) -ne $archiveHash) {
        throw 'The downloaded Mihomo archive failed its SHA-256 check.'
    }
    $extracted = Join-Path $build 'mihomo-1.19.34'
    New-Item -ItemType Directory -Force -Path $extracted | Out-Null
    & tar.exe -xf $archive -C $extracted
    if ($LASTEXITCODE -ne 0) { throw 'Extracting the verified Mihomo archive failed.' }
    $binary = @(Get-ChildItem -LiteralPath $extracted -Recurse -File -Filter 'mihomo*.exe')
    if ($binary.Count -ne 1 -or (Get-Sha256 $binary[0].FullName) -ne $coreHash) {
        throw 'The extracted Mihomo executable failed its SHA-256 check.'
    }
    Copy-Item -LiteralPath $binary[0].FullName -Destination $core -Force
}

$helpers = @('clashfork-service', 'clashfork-service-install', 'clashfork-service-uninstall')
$binArgs = foreach ($helper in $helpers) { '--bin'; $helper }
& cargo build --locked --release --target $target --target-dir $build --manifest-path (Join-Path $service 'Cargo.toml') --features standalone,client @binArgs
if ($LASTEXITCODE -ne 0) { throw "Building the Clashfork service failed with exit code $LASTEXITCODE." }
foreach ($helper in $helpers) {
    Copy-Item -LiteralPath (Join-Path $build "$target/release/$helper.exe") -Destination $resources -Force
}

$licenses = Join-Path $resources 'licenses'
New-Item -ItemType Directory -Force -Path $licenses | Out-Null
Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination (Join-Path $licenses 'Clashfork-GPL-3.0.txt') -Force
Copy-Item -LiteralPath (Join-Path $service 'LICENSE') -Destination (Join-Path $licenses 'Clashfork-Service-GPL-3.0.txt') -Force

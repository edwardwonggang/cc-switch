param(
  [Parameter(Mandatory = $true)]
  [ValidateSet('Fingerprint', 'Capture', 'Restore')]
  [string]$Action
)

$ErrorActionPreference = 'Stop'
$inputFile = '.ci/windows-build-inputs.json'
$cacheDir = '.ci/windows-portable-cache'
$cacheManifest = Join-Path $cacheDir 'build-provenance.json'
$cachedExe = Join-Path $cacheDir 'cc-switch.exe'
$deliveryManifest = '.ci/windows-portable-provenance.json'

function Get-ContentHash([byte[]]$Bytes) {
  $hasher = [Security.Cryptography.SHA256]::Create()
  try {
    return [BitConverter]::ToString($hasher.ComputeHash($Bytes)).Replace('-', '').ToLowerInvariant()
  } finally {
    $hasher.Dispose()
  }
}

function Get-FileContentHash([string]$File) {
  return Get-ContentHash ([IO.File]::ReadAllBytes((Join-Path (Get-Location) $File)))
}

if ($Action -eq 'Fingerprint') {
  New-Item -ItemType Directory -Force -Path '.ci' | Out-Null
  # Hash bytes, not checkout mtimes or commit IDs. Include this recipe itself:
  # modifying the input/verification contract must invalidate its old cache.
  [string[]]$tracked = @(& git -c core.quotepath=false ls-files)
  if ($LASTEXITCODE -ne 0) { throw 'Unable to enumerate build inputs' }
  [string[]]$selected = @($tracked | Where-Object {
    $_ -match '^(src/|src-tauri/|assets/|public/|scripts/|patches/)' -or
    $_ -eq '.github/scripts/windows-portable-cache.ps1' -or
    $_ -match '^(package\.json|pnpm-lock\.yaml|pnpm-workspace\.yaml|rust-toolchain\.toml|components\.json|\.node-version|\.npmrc|\.gitattributes|\.env(?:\..*)?|tsconfig[^/]*\.json|vite\.config\.[^/]+|tailwind\.config\.[^/]+|postcss\.config\.[^/]+|index\.html)$'
  })
  [Array]::Sort($selected, [StringComparer]::Ordinal)
  $files = @($selected | ForEach-Object {
    [ordered]@{ path = $_; sha256 = Get-FileContentHash $_ }
  })
  $rust = (& rustc -vV | Out-String).Trim()
  if ($LASTEXITCODE -ne 0) { throw 'Unable to identify rustc' }
  $nodeVersion = (& node --version | Out-String).Trim()
  if ($LASTEXITCODE -ne 0) { throw 'Unable to identify node' }
  $pnpmVersion = (& pnpm --version | Out-String).Trim()
  if ($LASTEXITCODE -ne 0) { throw 'Unable to identify pnpm' }
  # Hash environment values rather than writing possible caller-provided data
  # into public provenance. Vite exposes VITE_* / TAURI_ENV_* to the frontend.
  $environment = @(
    Get-ChildItem Env: | Where-Object {
      $_.Name -match '^(VITE_|TAURI_ENV_|CARGO_PROFILE_RELEASE_|CARGO_TARGET_|CARGO_BUILD_)' -or
      $_.Name -in @('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'TAURI_CONFIG',
        'NODE_ENV', 'NODE_OPTIONS', 'BROWSERSLIST', 'SOURCE_DATE_EPOCH',
        'CC', 'CXX', 'AR', 'CL', '_CL_', 'LINK', '_LINK_', 'CFLAGS', 'CXXFLAGS', 'LDFLAGS')
    } | Sort-Object Name | ForEach-Object {
      [ordered]@{
        name = $_.Name
        sha256 = Get-ContentHash ([Text.Encoding]::UTF8.GetBytes($_.Value))
      }
    }
  )
  $inputs = [ordered]@{
    schemaVersion = 1
    builder = [ordered]@{
      rustc = $rust
      node = $nodeVersion
      pnpm = $pnpmVersion
      runnerOS = $env:RUNNER_OS
      runnerArch = $env:RUNNER_ARCH
      runnerImage = $env:ImageVersion
      profile = 'release'
      tauriArguments = ($env:TAURI_PORTABLE_ARGS_JSON | ConvertFrom-Json)
    }
    environment = $environment
    files = $files
  }
  $serialized = $inputs | ConvertTo-Json -Depth 12 -Compress
  $inputHash = Get-ContentHash ([Text.Encoding]::UTF8.GetBytes($serialized))
  [ordered]@{
    inputHash = $inputHash
    sourceSha = $env:GITHUB_SHA
    inputs = $inputs
  } | ConvertTo-Json -Depth 14 | Set-Content -Encoding UTF8 $inputFile
  "input_hash=$inputHash" >> $env:GITHUB_OUTPUT
  Write-Host "Build input hash: $inputHash ($($files.Count) tracked inputs)"
  exit 0
}

$current = Get-Content -Raw -LiteralPath $inputFile | ConvertFrom-Json
if ($Action -eq 'Capture') {
  $exe = 'src-tauri/target/release/cc-switch.exe'
  if (-not (Test-Path -LiteralPath $exe)) { throw 'Compiled executable missing' }
  New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null
  Copy-Item -LiteralPath $exe -Destination $cachedExe -Force
  $cached = [ordered]@{
    schemaVersion = 1
    inputHash = $current.inputHash
    compiledSourceSha = $env:GITHUB_SHA
    compiledRunId = $env:GITHUB_RUN_ID
    executableSha256 = Get-FileContentHash $cachedExe
    buildInputs = $current.inputs
  }
  $cached | ConvertTo-Json -Depth 14 | Set-Content -Encoding UTF8 $cacheManifest
} else {
  $cached = Get-Content -Raw -LiteralPath $cacheManifest | ConvertFrom-Json
  if ($cached.inputHash -ne $current.inputHash) { throw 'Cached executable build inputs do not match' }
  if ((Get-FileContentHash $cachedExe) -ne $cached.executableSha256) { throw 'Cached executable hash mismatch' }
  Write-Host "Reusing executable compiled from $($cached.compiledSourceSha); equivalent inputs $($current.inputHash)"
}

[ordered]@{
  packagedForSha = $env:GITHUB_SHA
  packagedRunId = $env:GITHUB_RUN_ID
  compiledSourceSha = $cached.compiledSourceSha
  compiledRunId = $cached.compiledRunId
  inputHash = $current.inputHash
  inputsEquivalent = ($cached.inputHash -eq $current.inputHash)
  executableSha256 = $cached.executableSha256
  reusedExecutable = ($Action -eq 'Restore')
  buildInputs = $current.inputs
} | ConvertTo-Json -Depth 14 | Set-Content -Encoding UTF8 $deliveryManifest

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = $PSScriptRoot
$lspDir = Join-Path $root "ic20-lsp"
$ic10LspDir = Join-Path $root "ic10-lsp"
$compilerDir = Join-Path $root "ic20c"
$extDir = Join-Path $root "ic20-vscode"
$binDir = Join-Path $extDir "bin"

Write-Host "==> Building ic20-lsp (release)..."
Push-Location $lspDir
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally {
    Pop-Location
}

Write-Host "==> Building ic10-lsp (release)..."
Push-Location $ic10LspDir
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally {
    Pop-Location
}

Write-Host "==> Building ic20c (release)..."
Push-Location $compilerDir
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally {
    Pop-Location
}

Write-Host "==> Copying binaries to extension bin/..."
New-Item -ItemType Directory -Force $binDir | Out-Null

# Determine the platform/arch-specific name matching what extension.ts resolves
$platform = if ($IsWindows) { "win32" } elseif ($IsMacOS) { "darwin" } else { "linux" }
$arch     = if ([System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -eq "Arm64") { "arm64" } else { "x64" }
$ext      = if ($IsWindows) { ".exe" } else { "" }
$srcBin   = Join-Path $lspDir "target\release\ic20-lsp$ext"
$destBin  = Join-Path $binDir "ic20-lsp-$platform-$arch$ext"

Copy-Item -Force $srcBin $destBin
Write-Host "    -> $destBin"

$srcCompiler  = Join-Path $compilerDir "target\release\ic20c$ext"
$destCompiler = Join-Path $binDir "ic20c-$platform-$arch$ext"

Copy-Item -Force $srcCompiler $destCompiler
Write-Host "    -> $destCompiler"

$srcIc10Lsp  = Join-Path $ic10LspDir "target\release\ic10-lsp$ext"
$destIc10Lsp = Join-Path $binDir "ic10-lsp-$platform-$arch$ext"

Copy-Item -Force $srcIc10Lsp $destIc10Lsp
Write-Host "    -> $destIc10Lsp"

Write-Host "==> Installing npm dependencies..."
Push-Location $extDir
try {
    npm ci
    if ($LASTEXITCODE -ne 0) { throw "npm ci failed" }

    Write-Host "==> Compiling TypeScript..."
    npx tsc -p ./
    if ($LASTEXITCODE -ne 0) { throw "tsc failed" }

    Write-Host "==> Packaging VSIX..."
    npx vsce package
    if ($LASTEXITCODE -ne 0) { throw "vsce package failed" }
} finally {
    Pop-Location
}

$vsix = Get-ChildItem $extDir -Filter "*.vsix" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
Write-Host ""
Write-Host "==> Installing extension..."
code --profile "Rust" --install-extension $vsix.FullName
if ($LASTEXITCODE -ne 0) { throw "code --install-extension failed" }

Write-Host ""
Write-Host "Done. Run 'Developer: Reload Window' in VS Code (Ctrl+Shift+P) to activate the new version."

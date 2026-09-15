[CmdletBinding()]
param([switch]$FetchSources)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$out = Join-Path $root 'target\rc003-hook'
# Reuse the pinned Detours source/lib fetched by the probe build.
$reference = Join-Path $root 'target\rc003-native-tap\reference'
$commit = 'adb07604aa56508448b95bf037c2a6d0d3b6831a'
$source = Join-Path $reference "Detours-$commit"
$expected = '42125D318F607CDED3332BB61BB7BEBAC8A58A57E46CED6DE573610F1D4CEDBA'
New-Item -ItemType Directory -Force -Path $reference | Out-Null
New-Item -ItemType Directory -Force -Path $out | Out-Null
if (-not (Test-Path -LiteralPath (Join-Path $source 'src\detours.cpp'))) {
    if (-not $FetchSources) { throw 'Pinned Detours source missing. Use -FetchSources to download source only; no software is installed.' }
    $archive = Join-Path $reference 'detours.tar.gz'
    Invoke-WebRequest "https://codeload.github.com/microsoft/Detours/tar.gz/$commit" -OutFile $archive
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expected) { throw 'Detours archive hash mismatch' }
    tar -xzf $archive -C $reference
    if ($LASTEXITCODE -ne 0) { throw 'Detours extraction failed' }
}
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) { throw 'Existing MSVC Build Tools required; nothing will be installed.' }
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Existing x64 MSVC compiler not found' }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
# vcvarsall.bat invokes `vswhere` as a bareword; on non-standard VS install roots
# (e.g. C:\BuildTools) the Installer dir is not on PATH, so make it discoverable.
$installerDir = Split-Path -Parent $vswhere
$native = Join-Path $root 'native\rc003-hook'
$batch = Join-Path $out 'build.cmd'
@"
@echo off
set "PATH=%PATH%;$installerDir"
call "$vcvars" >nul
if errorlevel 1 exit /b 1
if not exist "$source\lib.X64\detours.lib" (
  cd /d "$source\src"
  nmake /nologo
  if errorlevel 1 exit /b 1
)
cd /d "$out"
cl /nologo /W4 /WX /O2 /MT /EHsc /std:c++17 /guard:cf /I "$source\include" /I "$native" /LD "$native\hook.cpp" "$source\lib.X64\detours.lib" /link /OUT:sayall-rc003-hook.dll /DYNAMICBASE /NXCOMPAT
if errorlevel 1 exit /b 1
cl /nologo /W4 /WX /O2 /MT /EHsc /std:c++17 /guard:cf /I "$native" "$native\inject.cpp" advapi32.lib cfgmgr32.lib shell32.lib ole32.lib /link /OUT:sayall-rc003-inject.exe /DYNAMICBASE /NXCOMPAT
exit /b %errorlevel%
"@ | Set-Content -LiteralPath $batch -Encoding ascii
$log = Join-Path $out 'build.log'
& $env:ComSpec /d /c $batch *> $log
if ($LASTEXITCODE -ne 0) { Get-Content -LiteralPath $log -Tail 65; throw "Native hook build failed; full log: $log" }
Get-Item (Join-Path $out 'sayall-rc003-hook.dll'), (Join-Path $out 'sayall-rc003-inject.exe') | Select-Object Name,Length
Write-Output 'build=passed; production app unchanged'

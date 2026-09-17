<#
test-installer.ps1 -- isolated regression tests for install.ps1.

Extracts installer functions by AST (never runs the real install main,
network, live user configs, or PATH) and exercises them in temp dirs.

Run:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\test-installer.ps1
#>

$ErrorActionPreference = 'Stop'
$script:failures = 0

function Assert-True {
    param([bool]$Cond, [string]$Name)
    if ($Cond) { Write-Host "PASS: $Name" }
    else { $script:failures++; Write-Host "FAIL: $Name" -ForegroundColor Red }
}

$installer = Join-Path $PSScriptRoot '..\install.ps1'
$installer = [System.IO.Path]::GetFullPath($installer)
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$errors)
if ($errors.Count -gt 0) { throw "install.ps1 has parse errors" }

foreach ($n in @('Install-BinaryAtomic', 'ConvertFrom-JsonRaw', 'ConvertFrom-JsonXmlNode',
                 'ConvertTo-JsonRawHashtable', 'Test-EntryDeepEqual', 'Update-HashlineMcpConfig')) {
    $f = $ast.Find({ param($x) $x -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $x.Name -eq $n }, $true)
    if (-not $f) { throw "missing function: $n" }
    Invoke-Expression $f.Extent.Text
}
function Die { param($msg) throw "DIE: $msg" }
function Write-Warn { param($m) }

# --- Install-BinaryAtomic: normal replace ---
$root = Join-Path ([System.IO.Path]::GetTempPath()) ('hl-it-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path (Join-Path $root 'dest'), (Join-Path $root 'src') | Out-Null
$src = Join-Path (Join-Path $root 'src') 'new.exe'
$dst = Join-Path (Join-Path $root 'dest') 'hashline.exe'
Set-Content -LiteralPath $src 'NEW-BINARY'
Set-Content -LiteralPath $dst 'OLD-BINARY'
Install-BinaryAtomic -SourcePath $src -DestPath $dst
$leftoverOld = @(Get-ChildItem (Join-Path $root 'dest') -Filter 'hashline.exe.old.*' -ErrorAction SilentlyContinue)
$onlyExe = @((Get-ChildItem (Join-Path $root 'dest') -Force) | Where-Object { $_.Name -ne 'hashline.exe' }).Count -eq 0
$ok = ((Get-FileHash $dst).Hash -eq (Get-FileHash $src).Hash) -and
      (-not (Test-Path "$dst.old.*")) -and ($leftoverOld.Count -eq 0) -and $onlyExe
Assert-True $ok 'atomic: normal replace, hash matches, no leftovers'

# --- Install-BinaryAtomic: running-exe style lock (delete-share) triggers rename fallback ---
Set-Content -LiteralPath $dst 'OLD-BINARY'
$fs = $null
try {
    $fs = [System.IO.File]::Open($dst, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read,
                                 ([System.IO.FileShare]::Read -bor [System.IO.FileShare]::Delete))
    Install-BinaryAtomic -SourcePath $src -DestPath $dst
    Assert-True $true 'atomic: locked replace succeeds via rename fallback'
} catch { Assert-True $false ('atomic: locked replace threw: ' + $_.Exception.Message) }
finally { if ($fs) { $fs.Close() } }
Assert-True ((Get-FileHash $dst).Hash -eq (Get-FileHash $src).Hash) 'atomic: locked replace hash matches payload'

# --- Install-BinaryAtomic: hard lock fails loudly (no false success) ---
Set-Content -LiteralPath $dst 'OLD-BINARY'
$fs2 = $null
$threw = $false
try {
    $fs2 = [System.IO.File]::Open($dst, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    try { Install-BinaryAtomic -SourcePath $src -DestPath $dst } catch { $threw = $true }
} finally { if ($fs2) { $fs2.Close() } }
Assert-True $threw 'atomic: unrenamable lock throws instead of false success'
Assert-True ((Get-Content -LiteralPath $dst -Raw) -like '*OLD-BINARY*') 'atomic: failed replace keeps old binary'

# --- Update-HashlineMcpConfig fixtures ---
$cfg = Join-Path $root 'claude.json'
Set-Content -LiteralPath $cfg -Value '{"mcpServers":{"hashline":{"command":"OLD","args":["mcp"],"env":{"HASHLINE_RETURN_ANCHORS":"1"}},"other":{"command":"keep"}},"projects":{"C:/Users/ADMIN/Documents/Projects":{"x":1},"c:/Users/ADMIN/Documents/Projects":{"y":2}}}'
$r = Update-HashlineMcpConfig -Path $cfg -ServersKey 'mcpServers' -BinaryPath 'C:\new\hashline.exe'
Assert-True ($r.Status -eq 'updated') 'config: first run updates command'
$p = ConvertFrom-JsonRaw -Raw (Get-Content $cfg -Raw)
Assert-True ($p['mcpServers']['hashline']['command'] -eq 'C:\new\hashline.exe') 'config: command updated'
Assert-True ($p['mcpServers']['hashline']['env']['HASHLINE_RETURN_ANCHORS'] -eq '1') 'config: env preserved'
Assert-True ($p['mcpServers']['other']['command'] -eq 'keep') 'config: other server preserved'
Assert-True ($p['projects'].Keys.Count -eq 2) 'config: case-distinct keys preserved'
$r2 = Update-HashlineMcpConfig -Path $cfg -ServersKey 'mcpServers' -BinaryPath 'C:\new\hashline.exe'
Assert-True ($r2.Status -eq 'unchanged') 'config: second run is unchanged'
Assert-True ((Get-Content $cfg -Raw) -eq (Get-Content $cfg -Raw)) 'config: unchanged write is content-stable'

$bad = Join-Path $root 'bad.json'
Set-Content -LiteralPath $bad -Value '{not json'
$before = Get-Content $bad -Raw
$rb = Update-HashlineMcpConfig -Path $bad -ServersKey 'mcpServers' -BinaryPath 'X'
Assert-True ($null -eq $rb) 'config: invalid json skipped'
Assert-True ((Get-Content $bad -Raw) -eq $before) 'config: invalid json untouched'

$empty = Join-Path $root 'empty.json'
Set-Content -LiteralPath $empty -Value ''
$re = Update-HashlineMcpConfig -Path $empty -ServersKey 'mcpServers' -BinaryPath 'X'
Assert-True ($re.Status -eq 'installed') 'config: empty file installs'
Assert-True ((ConvertFrom-JsonRaw -Raw (Get-Content $empty -Raw))['mcpServers']['hashline']['command'] -eq 'X') 'config: empty file populated'

$amp = Join-Path $root 'amp.json'
Set-Content -LiteralPath $amp -Value '{"amp":{"mcpServers":{"other":{"command":"keep"}}}}'
Update-HashlineMcpConfig -Path $amp -ServersKey 'amp.mcpServers' -BinaryPath 'Y' | Out-Null
$pa = ConvertFrom-JsonRaw -Raw (Get-Content $amp -Raw)
Assert-True ($pa['amp']['mcpServers']['hashline']['command'] -eq 'Y') 'config: dotted key hashline'
Assert-True ($pa['amp']['mcpServers']['other']['command'] -eq 'keep') 'config: dotted key other'

Remove-Item $root -Recurse -Force
if ($script:failures -gt 0) { Write-Host "$($script:failures) FAILURES" -ForegroundColor Red; exit 1 }
Write-Host 'ALL INSTALLER TESTS PASS'

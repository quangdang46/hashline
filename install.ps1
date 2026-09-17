<#
install.ps1 -- one-shot installer for hashline on Windows.

Usage:
  irm https://raw.githubusercontent.com/quangdang46/hashline/main/install.ps1 | iex
  iwr https://raw.githubusercontent.com/quangdang46/hashline/main/install.ps1 -UseBasicParsing | iex

Pinning a version or passing flags through `irm | iex` requires a wrapper:
  & ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/quangdang46/hashline/main/install.ps1'))) -Version v0.1.10 -EasyMode

Or download once and run directly:
  irm https://raw.githubusercontent.com/quangdang46/hashline/main/install.ps1 -OutFile install.ps1
  .\install.ps1 -Version v0.1.10 -EasyMode -Verify

Flags:
  -Dest <path>          Install location. Default: $env:USERPROFILE\.local\bin
  -System               Shortcut for -Dest "$env:ProgramFiles\hashline" (admin)
  -Version <vX.Y.Z>     Pin a specific release. Default: latest
  -EasyMode             Append the install dir to the *user* PATH if missing
  -Verify               Run `hashline --version` after install
  -NoMcp                Skip the auto-install of MCP provider configs
  -Quiet                Suppress info logs
  -Uninstall            Remove the binary and any easy-mode PATH entry
  -Help                 Show this help and exit
#>

[CmdletBinding()]
param(
    [string] $Dest    = "$env:USERPROFILE\.local\bin",
    [switch] $System,
    [string] $Version = "",
    [switch] $EasyMode,
    [switch] $Verify,
    [switch] $NoMcp,
    [switch] $Quiet,
    [switch] $Uninstall,
    [switch] $Help
)

$ErrorActionPreference = 'Continue'
# Disables the slow IE-style progress bar in Invoke-WebRequest, which can
# slow large downloads from a couple of seconds to several minutes.
$ProgressPreference    = 'SilentlyContinue'

# Force TLS 1.2 (and 1.3 if available). Windows PowerShell 5.1 still defaults
# to TLS 1.0/1.1 for .NET HTTP clients, which GitHub releases / api.github.com
# now reject -- surfaces as "The request was aborted: The connection was
# closed unexpectedly." The -bor preserves any newer protocols the runtime
# already has enabled.
try {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch { }

# ============================================================================
# Configuration
# ============================================================================

$BinaryName = 'hashline'
$BinaryFile = "$BinaryName.exe"
$Owner      = 'quangdang46'
$Repo       = 'hashline'

if ($System) { $Dest = "$env:ProgramFiles\$BinaryName" }

# ============================================================================
# Logging
# ============================================================================

function Write-Info { param($msg) if (-not $Quiet) { Write-Host "==> [$BinaryName] $msg" -ForegroundColor Cyan } }
function Write-Warn { param($msg) Write-Host "!! [$BinaryName] $msg" -ForegroundColor Yellow }
function Write-Ok   { param($msg) if (-not $Quiet) { Write-Host "[OK] $msg" -ForegroundColor Green } }
function Die        { param($msg) Write-Host "ERROR: $msg" -ForegroundColor Red; exit 1 }

# ============================================================================
# Help -- print the doc-comment block at the top of this file.
# ============================================================================

if ($Help) {
    $self = $MyInvocation.MyCommand.Path
    if (-not $self) { $self = $PSCommandPath }
    if ($self -and (Test-Path $self)) {
        $content = Get-Content -Raw $self
        if ($content -match '(?s)<#(.*?)#>') { Write-Host $matches[1].Trim() }
    } else {
        Write-Host "hashline installer for Windows. Run with -Help on a downloaded copy for full text."
    }
    exit 0
}

# ============================================================================
# Platform detection -- Windows only. Anything else: bail with a hint at the
# Unix installer instead of silently producing a broken binary.
# ============================================================================

function Get-Platform {
    if ($IsLinux -or $IsMacOS) {
        Die "install.ps1 is for Windows only. On Linux / macOS use install.sh:`n  curl -fsSL https://raw.githubusercontent.com/$Owner/$Repo/main/install.sh | bash"
    }
    $arch = $env:PROCESSOR_ARCHITECTURE
    # WOW64 reports x86 even on 64-bit; PROCESSOR_ARCHITEW6432 reflects the
    # real OS bitness when the PowerShell host itself is 32-bit.
    if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
    switch -Wildcard ($arch) {
        'AMD64'  { return 'windows-x86_64' }
        'x86_64' { return 'windows-x86_64' }
        'ARM64'  { Die "Windows on ARM64 isn't published yet. Track https://github.com/$Owner/$Repo/issues for updates, or build from source: https://github.com/$Owner/$Repo#from-source" }
        default  { Die "unsupported architecture: $arch" }
    }
}

# ============================================================================
# Uninstall
# ============================================================================

function Invoke-Uninstall {
    $target = Join-Path $Dest $BinaryFile
    if (Test-Path $target) {
        Remove-Item -LiteralPath $target -Force
        Write-Ok "removed $target"
    } else {
        Write-Warn "no binary at $target"
    }

    # Strip $Dest from the user PATH if we ever appended it under -EasyMode.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -and (($userPath -split ';') -contains $Dest)) {
        $entries = $userPath -split ';' | Where-Object { $_ -and ($_ -ne $Dest) }
        $newPath = ($entries -join ';')
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Write-Ok "removed $Dest from user PATH"
    }

    Write-Ok "uninstalled"
    exit 0
}

if ($Uninstall) { Invoke-Uninstall }

# ============================================================================
# Version resolution
#
# Primary path is the GitHub releases API. If that's rate-limited or blocked,
# fall back to a HEAD against /releases/latest and parse the redirect target.
# ============================================================================

function Resolve-Version {
    if ($script:Version) {
        $v = $script:Version
        if (-not $v.StartsWith('v')) { $v = "v$v" }
        return $v
    }

    try {
        $api  = "https://api.github.com/repos/$Owner/$Repo/releases/latest"
        $resp = Invoke-RestMethod -Uri $api -Headers @{ 'Accept' = 'application/vnd.github.v3+json' } -TimeoutSec 30
        if ($resp.tag_name) {
            Write-Info "latest version: $($resp.tag_name)"
            return $resp.tag_name
        }
    } catch {
        Write-Warn "GitHub API request failed; falling back to redirect probe ($($_.Exception.Message))"
    }

    try {
        $resp = Invoke-WebRequest -Uri "https://github.com/$Owner/$Repo/releases/latest" -MaximumRedirection 0 -UseBasicParsing -ErrorAction SilentlyContinue
        $loc  = $resp.Headers.Location
        if ($loc -and $loc -match '/tag/(v[0-9][^/?#]*)') {
            Write-Info "latest version: $($matches[1])"
            return $matches[1]
        }
    } catch { }

    Die "could not resolve latest version. Pass -Version vX.Y.Z to pin."
}

# ============================================================================
# Download with retry
# ============================================================================

function Get-FileWithRetry {
    param(
        [Parameter(Mandatory)] [string] $Url,
        [Parameter(Mandatory)] [string] $OutPath,
        [int] $MaxRetries = 3,
        [int] $TimeoutSec = 120
    )
    for ($attempt = 1; $attempt -le $MaxRetries; $attempt++) {
        try {
            Invoke-WebRequest -Uri $Url -OutFile $OutPath -TimeoutSec $TimeoutSec -UseBasicParsing
            return $true
        } catch {
            if ($attempt -lt $MaxRetries) {
                Write-Warn "download attempt $attempt failed; retrying in 3s..."
                Start-Sleep -Seconds 3
            } else {
                Write-Warn "download failed: $($_.Exception.Message)"
                return $false
            }
        }
    }
    return $false
}

# ============================================================================
# PATH update (opt-in via -EasyMode)
# ============================================================================

function Update-UserPath {
    $current = $env:Path -split ';'
    if ($current -contains $Dest) { return }

    if ($EasyMode) {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $entries  = if ($userPath) { $userPath -split ';' } else { @() }
        if ($entries -notcontains $Dest) {
            $newPath = (($entries + $Dest) | Where-Object { $_ } ) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
            Write-Ok "added $Dest to user PATH"
            Write-Warn "open a new PowerShell window for the change to take effect."
        }
    } else {
        Write-Warn "$Dest is not on your PATH. Either:"
        Write-Warn "  - rerun with -EasyMode to add it permanently to your user PATH, or"
        Write-Warn "  - prepend it manually:  `$env:Path = '$Dest;' + `$env:Path"
    }
}

# ============================================================================
# Atomic install -- write to a sibling temp file in the destination dir, then
# rename. Keeps an in-use binary intact if the move fails.
# ============================================================================

function Install-BinaryAtomic {
    param([string] $SourcePath, [string] $DestPath)
    $id = [guid]::NewGuid().ToString('N')
    $tmp = "$DestPath.tmp.$id"
    $oldPath = "$DestPath.old.$id"
    $retired = $false
    $promoted = $false
    try {
        $expected = (Get-FileHash -LiteralPath $SourcePath -Algorithm SHA256 -ErrorAction Stop).Hash
        Copy-Item -LiteralPath $SourcePath -Destination $tmp -ErrorAction Stop
        if ((Get-FileHash -LiteralPath $tmp -Algorithm SHA256 -ErrorAction Stop).Hash -ne $expected) {
            throw 'staged binary checksum mismatch'
        }

        # Retain the old image until verification succeeds. Running Windows
        # executables can often be renamed, but never assume that succeeded.
        if (Test-Path -LiteralPath $DestPath) {
            Move-Item -LiteralPath $DestPath -Destination $oldPath -ErrorAction Stop
            $retired = $true
        }
        Move-Item -LiteralPath $tmp -Destination $DestPath -ErrorAction Stop
        $promoted = $true
        if ((Get-FileHash -LiteralPath $DestPath -Algorithm SHA256 -ErrorAction Stop).Hash -ne $expected) {
            throw 'installed binary checksum mismatch'
        }
    } catch {
        $failure = $_.Exception.Message
        try {
            if ($promoted) { Remove-Item -LiteralPath $DestPath -Force -ErrorAction Stop }
            if ($retired) { Move-Item -LiteralPath $oldPath -Destination $DestPath -ErrorAction Stop }
        } catch {
            throw "failed to install ${DestPath}: $failure; rollback failed: $($_.Exception.Message); backup: $oldPath"
        }
        throw "failed to install ${DestPath}: $failure"
    } finally {
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    }
    # A running process may still hold the retired image; retain it if locked.
    if ($retired) { Remove-Item -LiteralPath $oldPath -Force -ErrorAction SilentlyContinue }
}

# ============================================================================
# MCP auto-install -- mirrors install.sh: best-effort, never fatal.
#
# Detect installed MCP providers (Claude Code, Cursor, Codex, etc.) and
# upsert a `hashline` server entry into each provider's config file.
# Replicates the logic that used to live in `hashline install-mcp`.
# Failures here just print a hint; the binary install has already succeeded.
# ============================================================================

function ConvertFrom-JsonRaw {
    # PS 7: ConvertFrom-Json -AsHashtable is exact-case and lossless.
    # Windows PowerShell 5.1 lacks it; DataContractJsonSerializer's XML
    # reader round-trips case-distinct keys through a hashtable instead of
    # PSCustomObject (which would collapse them).
    param([string]$Raw)
    if ($PSVersionTable.PSVersion.Major -ge 6) {
        return ($Raw | ConvertFrom-Json -AsHashtable -ErrorAction Stop)
    }
    Add-Type -AssemblyName System.Runtime.Serialization
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Raw)
    $reader = [System.Runtime.Serialization.Json.JsonReaderWriterFactory]::CreateJsonReader(
        $bytes, [System.Xml.XmlDictionaryReaderQuotas]::Max)
    $xml = New-Object System.Xml.XmlDocument
    try {
        $xml.Load($reader)
    } finally {
        $reader.Dispose()
    }
    return ConvertFrom-JsonXmlNode -Value $xml.DocumentElement
}

function ConvertFrom-JsonXmlNode {
    param($Value)
    switch ($Value.GetAttribute('type')) {
        'object' {
            $table = New-Object System.Collections.Specialized.OrderedDictionary
            foreach ($child in $Value.ChildNodes) {
                # The JSON XML reader mangles keys with slashes/colons into a
                # namespaced <a:item item="original"> element; recover the
                # original name from the `item` attribute.
                $name = $child.LocalName
                if ($child.LocalName -eq 'item' -and $child.NamespaceURI -eq 'item') {
                    $name = $child.GetAttribute('item')
                }
                $table[$name] = ConvertFrom-JsonXmlNode -Value $child
            }
            return $table
        }
        'array' {
            $items = @()
            foreach ($child in $Value.ChildNodes) {
                $items += , (ConvertFrom-JsonXmlNode -Value $child)
            }
            return $items
        }
        'null' { return $null }
        'boolean' { return ($Value.InnerText -eq 'true') }
        'number' { return [double]$Value.InnerText }
        default {
            # Numbers encode as 1 / 0 in the XML text; honor the type hint.
            $t = $Value.GetAttribute('type')
            if ($t -eq 'number') { return [double]$Value.InnerText }
            return $Value.InnerText
        }
    }
}

function ConvertTo-JsonRawHashtable {
    param($Value)
    if ($Value -is [System.Collections.IDictionary]) {
        $pairs = foreach ($key in $Value.Keys) {
            '{0}: {1}' -f (ConvertTo-Json -InputObject ([string]$key) -Compress), (
                ConvertTo-JsonRawHashtable -Value $Value[$key]
            )
        }
        return '{' + ($pairs -join ',') + '}'
    }
    if ($Value -is [System.Collections.IEnumerable] -and -not ($Value -is [string])) {
        $items = foreach ($item in $Value) { ConvertTo-JsonRawHashtable -Value $item }
        return '[' + ($items -join ',') + ']'
    }
    if ($Value -is [bool]) { return $(if ($Value) { 'true' } else { 'false' }) }
    if ($null -eq $Value) { return 'null' }
    if ($Value -is [int] -or $Value -is [long] -or $Value -is [double] -or $Value -is [decimal] -or $Value -is [float] -or $Value -is [int16] -or $Value -is [byte]) {
        return ([string]$Value)
    }
    return (ConvertTo-Json -InputObject ([string]$Value) -Compress)
}

function Test-EntryDeepEqual {
    param($Left, $Right)
    if (($null -eq $Left) -ne ($null -eq $Right)) { return $false }
    if ($null -eq $Left) { return $true }
    if (($Left -is [System.Collections.IDictionary]) -ne ($Right -is [System.Collections.IDictionary])) { return $false }
    if ($Left -is [System.Collections.IDictionary]) {
        if ($Left.Keys.Count -ne $Right.Keys.Count) { return $false }
        foreach ($key in $Left.Keys) {
            if (-not $Right.Contains($key)) { return $false }
            if (-not (Test-EntryDeepEqual -Left $Left[$key] -Right $Right[$key])) { return $false }
        }
        return $true
    }
    if (($Left -is [System.Collections.IEnumerable]) -and -not ($Left -is [string]) -and
        ($Right -is [System.Collections.IEnumerable]) -and -not ($Right -is [string])) {
        $l = @($Left); $r = @($Right)
        if ($l.Count -ne $r.Count) { return $false }
        for ($i = 0; $i -lt $l.Count; $i++) {
            if (-not (Test-EntryDeepEqual -Left $l[$i] -Right $r[$i])) { return $false }
        }
        return $true
    }
    return ("$Left" -eq "$Right")
}

function Update-HashlineMcpConfig {
    param(
        [string]$Path,
        [string]$ServersKey,
        [string]$BinaryPath
    )

    $managed = @{
        command = $BinaryPath
        args    = @('mcp')
    }

    # Load existing config (or start empty). Hashtable keeps exact-case keys
    # so case-distinct project entries survive the round trip.
    $config = $null
    $status = 'installed'
    if (Test-Path $Path) {
        try {
            $raw = Get-Content -Path $Path -Raw -ErrorAction Stop
            if ($raw.Trim().Length -gt 0) {
                $config = ConvertFrom-JsonRaw -Raw $raw
            }
        } catch {
            Write-Warn "Skipping $Path (invalid JSON: $($_.Exception.Message))"
            return $null
        }
    }
    if ($null -eq $config) { $config = @{} }

    # Ensure servers container exists (dotted keys like amp.mcpServers).
    $parts = $ServersKey.Split('.')
    $cursor = $config
    foreach ($part in $parts) {
        if (-not $cursor.Contains($part) -or -not ($cursor[$part] -is [System.Collections.IDictionary])) {
            $cursor[$part] = @{}
        }
        $cursor = $cursor[$part]
    }
    $serversObj = $cursor

    # Merge first, then deep-compare: unchanged means the merged entry is
    # identical to what is already on disk, regardless of key order or how
    # the equality would look field-by-field.
    $existing = $serversObj['hashline']
    if ($existing -is [System.Collections.IDictionary]) {
        foreach ($key in @($existing.Keys)) { if (-not $managed.Contains($key)) { $managed[$key] = $existing[$key] } }
    }
    $serversObj['hashline'] = $managed
    if (Test-EntryDeepEqual -Left $existing -Right $managed) {
        return @{ Status = 'unchanged'; Path = $Path }
    }
    $status = if ($existing) { 'updated' } else { 'installed' }

    $dir = Split-Path -Parent $Path
    if ($dir -and -not (Test-Path $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    $json = ConvertTo-JsonRawHashtable -Value $config
    $staged = "$Path.tmp.$PID"
    Set-Content -LiteralPath $staged -Value $json -Encoding UTF8
    Move-Item -LiteralPath $staged -Destination $Path -Force

    return @{ Status = $status; Path = $Path }
}

function Update-HashlineCodexConfig {
    param([string]$Path, [string]$BinaryPath)

    $marker = '[mcp_servers.hashline]'
    $cmdEsc = $BinaryPath -replace '\\', '\\\\'
    $section = "$marker`r`ncommand = `"$cmdEsc`"`r`nargs = [`"mcp`"]"

    $dir = Split-Path -Parent $Path
    if ($dir -and -not (Test-Path $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }

    if (-not (Test-Path $Path)) {
        Set-Content -Path $Path -Value $section -Encoding UTF8
        return @{ Status = 'installed'; Path = $Path }
    }

    $existing = Get-Content -Path $Path -Raw -ErrorAction Stop
    if ($existing -notmatch [regex]::Escape($marker)) {
        $sep = if ($existing.EndsWith("`n")) { '' } else { "`r`n" }
        Add-Content -Path $Path -Value "$sep`r`n$section" -Encoding UTF8
        return @{ Status = 'installed'; Path = $Path }
    }

    # Replace the existing [mcp_servers.hashline] block.
    $pattern = '(?ms)' + [regex]::Escape($marker) + '.*?(?=^\[|\z)'
    $updated = [regex]::Replace($existing, $pattern, "$section`r`n", 1)
    if ($updated -eq $existing) {
        return @{ Status = 'unchanged'; Path = $Path }
    }
    Set-Content -Path $Path -Value $updated -Encoding UTF8
    return @{ Status = 'updated'; Path = $Path }
}

function Invoke-McpAutoInstall {
    if ($NoMcp) { return }
    Write-Info "auto-installing MCP provider configs..."

    $bin = Join-Path $Dest $BinaryFile
    try {
        $resolved = (Resolve-Path -LiteralPath $bin -ErrorAction Stop).ProviderPath
        if ($resolved) { $bin = $resolved }
    } catch { } # Fall back to the joined path if resolution fails.

    # NB: do NOT use $home -- it's a read-only automatic variable in
    # PowerShell and assignment fails with
    #   "Cannot overwrite variable HOME because it is read-only or constant."
    $userHome = $env:USERPROFILE
    $cwd      = (Get-Location).Path
    $results  = @()

    # JSON-based hosts. Each entry: name, path, servers_key, condition.
    $jsonHosts = @(
        @{ Name = 'claude-code'; Path = "$userHome\.claude.json";                       Key = 'mcpServers';     Cond = { Test-Path "$userHome\.claude.json" } },
        @{ Name = 'cursor';      Path = "$userHome\.cursor\mcp.json";                   Key = 'mcpServers';     Cond = { Test-Path "$userHome\.cursor" -PathType Container } },
        @{ Name = 'windsurf';    Path = "$userHome\.codeium\windsurf\mcp_config.json";  Key = 'mcpServers';     Cond = { Test-Path "$userHome\.codeium\windsurf" -PathType Container } },
        @{ Name = 'vscode';      Path = "$cwd\.vscode\mcp.json";                        Key = 'servers';        Cond = { Test-Path "$cwd\.vscode" -PathType Container } },
        @{ Name = 'gemini';      Path = "$userHome\.gemini\settings.json";              Key = 'mcpServers';     Cond = { Test-Path "$userHome\.gemini" -PathType Container } },
        @{ Name = 'opencode';    Path = "$userHome\.opencode.json";                     Key = 'mcpServers';     Cond = { Test-Path "$userHome\.opencode.json" } },
        @{ Name = 'amp';         Path = "$userHome\.config\amp\settings.json";          Key = 'amp.mcpServers'; Cond = { Test-Path "$userHome\.config\amp" -PathType Container } },
        @{ Name = 'droid';       Path = "$userHome\.factory\mcp.json";                  Key = 'mcpServers';     Cond = { Test-Path "$userHome\.factory" -PathType Container } }
    )

    foreach ($h in $jsonHosts) {
        if (-not (& $h.Cond)) { continue }
        try {
            $r = Update-HashlineMcpConfig -Path $h.Path -ServersKey $h.Key -BinaryPath $bin
            if ($r) { $results += "- $($h.Name): $($r.Status) ($($r.Path))" }
        } catch {
            Write-Warn "$($h.Name): $($_.Exception.Message)"
        }
    }

    # TOML host: codex.
    if (Test-Path "$userHome\.codex" -PathType Container) {
        try {
            $r = Update-HashlineCodexConfig -Path "$userHome\.codex\config.toml" -BinaryPath $bin
            if ($r) { $results += "- codex: $($r.Status) ($($r.Path))" }
        } catch {
            Write-Warn "codex: $($_.Exception.Message)"
        }
    }

    if ($results.Count -eq 0) {
        Write-Warn "No supported MCP providers detected."
        return
    }
    Write-Info "hashline MCP auto-install results:"
    foreach ($line in $results) { Write-Host $line }
}

# ============================================================================
# Main
# ============================================================================

try {
    Write-Info "temp: $env:TEMP"
    $tempDir = Join-Path $env:TEMP "hashline-install-$PID"
    New-Item -ItemType Directory -Force -Path $tempDir | Out-Null
    if (-not (Test-Path $Dest)) { New-Item -ItemType Directory -Force -Path $Dest | Out-Null }

    $platform = Get-Platform
    Write-Info "platform: $platform"
    Write-Info "destination: $Dest"

    Write-Info "resolving latest version..."
    $Version = Resolve-Version
    Write-Info "version: $Version"

    $archive     = "$BinaryName-$Version-${platform}.zip"
    $base        = "https://github.com/$Owner/$Repo/releases/download/$Version"
    $archivePath = Join-Path $tempDir $archive

    Write-Info "url: $base/$archive"
    Write-Info "downloading $archive"
    if (-not (Get-FileWithRetry -Url "$base/$archive" -OutPath $archivePath)) {
        Die @"
failed to download $archive

The version you asked for ($Version) does not include $archive. Either:
  - pin a release that does:  -Version v0.1.10 (or newer)
  - or build from source:     https://github.com/$Owner/$Repo#from-source
"@
    }

    # Verify SHA-256 against the sidecar if release.yml published one. The
    # sidecar may be either "<hash>" or "<hash>  <filename>" -- Split() picks
    # the first whitespace-delimited token either way.
    $sumPath = "${archivePath}.sha256"
    if (Get-FileWithRetry -Url "$base/${archive}.sha256" -OutPath $sumPath -MaxRetries 1 -TimeoutSec 30) {
        $expected = (Get-Content -LiteralPath $sumPath -Raw).Trim().Split()[0]
        $actual   = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLower()
        if ($expected.ToLower() -ne $actual) {
            Die "checksum mismatch for $archive`n  expected: $expected`n  actual:   $actual"
        }
        Write-Info "checksum verified"
    } else {
        Write-Warn "no checksum file at ${archive}.sha256 -- skipping verification"
    }

    # Extract. The archive root contains either hashline.exe directly or a
    # single subdir holding it; Get-ChildItem -Recurse handles both layouts.
    $extractDir = Join-Path $tempDir 'extract'
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDir -Force

    $bin = Get-ChildItem -LiteralPath $extractDir -Recurse -Filter $BinaryFile -File |
           Select-Object -First 1
    if (-not $bin) { Die "$BinaryFile not found inside $archive" }

    Install-BinaryAtomic -SourcePath $bin.FullName -DestPath (Join-Path $Dest $BinaryFile)

    # -----------------------------------------------------------------------
    # Also replace any existing `hashline` on PATH at a different location
    # so `hashline --version` always shows the latest version. Source is the
    # extracted release binary -- never the primary destination, which may
    # be stale if its own replace failed. Best-effort: report, don't die.
    # -----------------------------------------------------------------------
    $existingOnPath = Get-Command $BinaryName -ErrorAction SilentlyContinue
    if ($existingOnPath) {
        $existingDir = Split-Path $existingOnPath.Source -Parent
        $existingDirNorm = $existingDir.TrimEnd('\').ToLower()
        $destDirNorm = $Dest.TrimEnd('\').ToLower()
        if ($existingDirNorm -ne $destDirNorm) {
            Write-Info "also updating existing $BinaryName at $($existingOnPath.Source)"
            try {
                Install-BinaryAtomic -SourcePath $bin.FullName -DestPath $existingOnPath.Source
                Write-Ok "replaced $($existingOnPath.Source)"
            } catch {
                Write-Warn "could not update $($existingOnPath.Source) -- you may need to run as admin or remove it manually ($($_.Exception.Message))"
            }
        }
    }

    Update-UserPath

    if ($Verify) {
        Write-Info "running self-test: $Dest\$BinaryFile --version"
        & (Join-Path $Dest $BinaryFile) --version | Out-Host
    }

    # MCP install runs last so failures don't undo the binary install above.
    Invoke-McpAutoInstall

    # Final gate: the installed binary must report the requested version.
    # Never print [OK] on a stale destination again.
    $destBin = Join-Path $Dest $BinaryFile
    $v = $null
    try { $v = & $destBin --version 2>$null } catch { }
    if (-not $v -or ($v -notmatch [regex]::Escape($Version.TrimStart('v')))) {
        Die "installed binary reports '$v' but requested $Version at $destBin"
    }
    Write-Host ""
    Write-Host "[OK] $BinaryName installed -> $destBin" -ForegroundColor Green
    Write-Host "   version: $v"

    # Hunt down stale copies: any other `hashline` on PATH whose version
    # differs from what we just installed is an outdated shadow -- delete it
    # so the new binary always wins regardless of PATH order. Deleting a
    # running .exe fails on Windows, so rename out of the way first, then
    # delete. Best-effort only: leftover copies just warn.
    try {
        $newBin = Join-Path $Dest $BinaryFile
        $newVer = & $newBin --version 2>$null
        $allOnPath = Get-Command $BinaryName -All -ErrorAction SilentlyContinue
        foreach ($cmd in $allOnPath) {
            $cmdDirNorm = (Split-Path $cmd.Source -Parent).TrimEnd('\').ToLower()
            if ($cmdDirNorm -eq $Dest.TrimEnd('\').ToLower()) { continue }
            $otherVer = $null
            try { $otherVer = & $cmd.Source --version 2>$null } catch { }
            if ($otherVer -and ($otherVer -ne $newVer)) {
                Write-Info "removing stale $BinaryName ($otherVer) at $($cmd.Source)"
                $removed = $false
                try {
                    # Rename-then-delete: a running .exe can be renamed even
                    # though it cannot be deleted/overwritten directly.
                    $staleOld = [string]$cmd.Source + ".stale." + [string]$PID
                    Rename-Item -LiteralPath $cmd.Source -NewName (Split-Path $staleOld -Leaf) -ErrorAction Stop
                    Remove-Item -LiteralPath $staleOld -Force -ErrorAction SilentlyContinue
                    if (-not (Test-Path -LiteralPath $cmd.Source)) { $removed = $true }
                } catch { }
                if (-not $removed) {
                    try {
                        Remove-Item -LiteralPath $cmd.Source -Force -ErrorAction Stop
                        $removed = $true
                    } catch { }
                }
                if ($removed) {
                    Write-Ok "removed stale copy: $($cmd.Source)"
                } else {
                    Write-Warn "could not remove $($cmd.Source) -- delete it manually or run as admin"
                }
            }
        }
    } catch { }
    Write-Host ""
    Write-Host "   quick start:"
    Write-Host "     $BinaryName --help"
    Write-Host "     $BinaryName read <file>"
    Write-Host ""
    Write-Host "   uninstall:"
    Write-Host "     irm https://raw.githubusercontent.com/$Owner/$Repo/main/install.ps1 -OutFile `$env:TEMP\hashline-uninstall.ps1; & `$env:TEMP\hashline-uninstall.ps1 -Uninstall"
    Write-Host ""
}
finally {
    if (Test-Path $tempDir) { Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue }
}

param(
    [string]$Binary = '',
    [string]$OutputDirectory = ''
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (!$Binary) { $Binary = Join-Path $repo 'client/target/armv7-unknown-linux-musleabihf/release/kindle-dashboard' }
if (!$OutputDirectory) { $OutputDirectory = Join-Path $repo 'dist' }
$Binary = (Resolve-Path -LiteralPath $Binary).Path
$stream = [IO.File]::OpenRead($Binary)
try {
    $header = [byte[]]::new(52)
    if (($stream.Read($header, 0, $header.Length) -ne 52) -or
        ([BitConverter]::ToUInt32($header, 0) -ne 0x464c457f) -or
        $header[4] -ne 1 -or $header[5] -ne 1 -or
        [BitConverter]::ToUInt16($header, 18) -ne 40 -or
        (([BitConverter]::ToUInt32($header, 36) -band 0x400) -eq 0)) {
        throw 'Expected a 32-bit little-endian ARM ELF with hard-float ABI.'
    }
    $programHeaderOffset = [BitConverter]::ToUInt32($header, 28)
    $programHeaderSize = [BitConverter]::ToUInt16($header, 42)
    $programHeaderCount = [BitConverter]::ToUInt16($header, 44)
    $segmentType = [byte[]]::new(4)
    for ($index = 0; $index -lt $programHeaderCount; $index++) {
        $stream.Position = $programHeaderOffset + $index * $programHeaderSize
        if ($stream.Read($segmentType, 0, 4) -ne 4) { throw 'Truncated program header.' }
        if ([BitConverter]::ToUInt32($segmentType, 0) -eq 3) { throw 'Expected a statically linked binary (no PT_INTERP).' }
    }
} finally {
    $stream.Dispose()
}

$stage = Join-Path $OutputDirectory 'kpw4-dashboard'
New-Item -Force -ItemType Directory -Path (Join-Path $stage 'documents'), (Join-Path $stage 'kindle-dashboard') | Out-Null
foreach ($obsolete in 'Dashboard Recovery.sh', 'Dashboard RTC Probe.sh') {
    $obsoleteFile = Join-Path (Join-Path $stage 'documents') $obsolete
    if (Test-Path -LiteralPath $obsoleteFile -PathType Leaf) { Remove-Item -LiteralPath $obsoleteFile }
}
$deviceFiles = Join-Path $repo 'device'
Get-ChildItem -Path $deviceFiles -Recurse -Filter '*.sh' | ForEach-Object {
    $relativePath = $_.FullName.Substring($deviceFiles.Length + 1)
    $target = Join-Path $stage $relativePath
    $script = [IO.File]::ReadAllText($_.FullName).Replace("`r`n", "`n")
    [IO.File]::WriteAllText($target, $script)
}
Copy-Item -LiteralPath $Binary -Destination (Join-Path $stage 'kindle-dashboard/kindle-dashboard') -Force
Copy-Item -LiteralPath (Join-Path $repo 'client/assets/OFL.txt') -Destination (Join-Path $stage 'kindle-dashboard/FONT-LICENSE.txt') -Force
Copy-Item -LiteralPath (Join-Path $repo 'client/assets/NotoSansSC-OFL.txt') -Destination (Join-Path $stage 'kindle-dashboard/NOTO-LICENSE.txt') -Force
$archive = Join-Path $OutputDirectory 'kpw4-dashboard.zip'
Compress-Archive -Path (Join-Path $stage 'documents'), (Join-Path $stage 'kindle-dashboard') -DestinationPath $archive -Force
Get-FileHash -Algorithm SHA256 -LiteralPath $archive

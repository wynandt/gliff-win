# Installs a pinned NASM on a Windows CI runner and puts it on PATH for the
# later steps of the job. OpenH264 is built from source and needs nasm.
#
# The version and SHA-256 are pinned here, so a changed download fails the job
# instead of reaching the build. To bump: change $version and $sha256 together
# (docs/development.md, "Rules the workflows follow"). Windows PowerShell 7+.
$ErrorActionPreference = 'Stop'

$version = '3.02'
$sha256 = '161d0bfaff53c2f9e9f3e69fd0672323ebabafd1268976a5cec11be92a19aee7'

$temp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$zip = Join-Path $temp "nasm-$version-win64.zip"
$url = "https://www.nasm.us/pub/nasm/releasebuilds/$version/win64/nasm-$version-win64.zip"

Invoke-WebRequest -Uri $url -OutFile $zip -MaximumRetryCount 3 -RetryIntervalSec 5

$actual = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLowerInvariant()
if ($actual -ne $sha256) {
    throw "nasm-$version-win64.zip has SHA-256 $actual, expected $sha256"
}

Expand-Archive -Path $zip -DestinationPath $temp -Force
$bin = Join-Path $temp "nasm-$version"

if ($env:GITHUB_PATH) {
    Add-Content -Path $env:GITHUB_PATH -Value $bin -Encoding utf8
}
& (Join-Path $bin 'nasm.exe') -v

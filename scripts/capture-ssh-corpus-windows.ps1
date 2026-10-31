# Captures real stderr of Windows' OpenSSH client for the connect-diagnostics
# corpus (crates/gliff-windows/tests/ssh_corpus/windows-openssh-<version>/).
# The Linux half is scripts/capture-ssh-corpus.
#
#   powershell -File scripts\capture-ssh-corpus-windows.ps1 <outdir>
#
# Read-only on the machine it runs on: it starts no sshd, makes no key, writes
# no known_hosts and no config, and touches no registry value. Every ssh call
# uses `-F NUL` and a null known_hosts file. This is a capture tool for test
# data: it passes options the client itself must never pass. The only things it
# opens are connections to loopback and to documentation addresses that answer
# nobody, and, for one case, a loopback listener that closes every connection,
# for a few seconds. It reads one file, the Windows win.ini, as a config file
# that ssh cannot parse.
#
# Cases that need a server (an unknown or changed host key, a refused key, a key
# file with loose rights, a missing server program) need a real sshd, and are
# only in the Linux corpus. The stderr is saved byte for byte: ssh ends its own
# messages with CRLF, and the tests read the files that way.
param([Parameter(Mandatory = $true)][string]$OutDir)
$ErrorActionPreference = 'Stop'
$ssh = Join-Path $env:SystemRoot 'System32\OpenSSH\ssh.exe'
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path

function Capture([string]$Name, [string[]]$SshArgs) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $ssh
  $psi.Arguments = $SshArgs -join ' '
  $psi.UseShellExecute = $false
  $psi.CreateNoWindow = $true
  $psi.RedirectStandardInput = $true
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $p = [System.Diagnostics.Process]::Start($psi)
  $p.StandardInput.Close()
  $bytes = New-Object System.IO.MemoryStream
  $copy = $p.StandardError.BaseStream.CopyToAsync($bytes)
  $null = $p.StandardOutput.ReadToEndAsync()
  if (-not $p.WaitForExit(40000)) { $p.Kill(); Write-Warning "$Name killed after 40 s" }
  $null = $copy.Wait(5000)
  [System.IO.File]::WriteAllBytes((Join-Path $OutDir "$Name.stderr"), $bytes.ToArray())
  "${Name}: exit $($p.ExitCode)"
}

# What the client passes, plus the probe-only options.
$probe = @('-F', 'NUL', '-T', '-o', 'BatchMode=yes', '-o', 'IdentitiesOnly=yes',
  '-o', 'UserKnownHostsFile=NUL')
$client = $probe + @('-o', 'ConnectTimeout=10')

# A port nothing listens on. With ConnectTimeout, Windows ssh prints a
# different line than without it.
Capture 'refused' ($client + @('-p', '2298', '127.0.0.1', 'true'))
Capture 'refused_without_connect_timeout' ($probe + @('-p', '2298', '127.0.0.1', 'true'))
Capture 'name_not_found_unknown' ($client + @('a..b', 'true'))
Capture 'name_not_found_no_address' ($client + @('-o', 'AddressFamily=inet6', 'nohost.invalid', 'true'))
Capture 'missing_key_file_then_unresolved' ($client + @('-i', 'C:\nonexistent\key', 'a..b', 'true'))
Capture 'timeout' ($client + @('192.0.2.1', 'true'))
Capture 'unreachable' ($client + @('2001:db8::1', 'true'))
Capture 'bad_config' @('-F', (Join-Path $env:SystemRoot 'win.ini'), '-T', '-o', 'BatchMode=yes', 'x', 'true')

# A listener that accepts a connection and closes it at once.
$closer = Start-Job {
  $l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 2296)
  $l.Start()
  $end = (Get-Date).AddSeconds(25)
  while ((Get-Date) -lt $end) {
    if ($l.Pending()) { $l.AcceptTcpClient().Close() } else { Start-Sleep -Milliseconds 50 }
  }
  $l.Stop()
}
try {
  Start-Sleep -Seconds 2
  Capture 'reset' ($client + @('-p', '2296', '127.0.0.1', 'true'))
} finally {
  Stop-Job $closer
  Remove-Job $closer -Force
}

# The version, as a CRLF text file like the rest.
$ErrorActionPreference = 'Continue' # ssh -V writes to stderr
$version = (& $ssh -V 2>&1 | ForEach-Object { "$_" }) -join "`r`n"
[System.IO.File]::WriteAllText((Join-Path $OutDir 'version.txt'), "$version`r`n")

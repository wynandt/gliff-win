# Set up ssh keys

Gliff signs in with your own `ssh` and your own keys. It does not create keys, copy keys to a machine, store keys, or ask for a password. If `ssh user@host` works in a terminal without asking anything, a Gliff tab for `user@host` works too.

This page has the few commands that get you there. Run them yourself, in PowerShell on the Windows machine unless a step says otherwise.

## Before you start

You need the OpenSSH client that comes with Windows.

1. Run `ssh -V`. It prints a version when the client is present.
2. If PowerShell says the command is not found, turn the client on: Settings, System, Optional features, add "OpenSSH Client".

## 1. Trust the machine once

The first time you connect to a machine, ssh asks whether to trust it. Gliff cannot answer that question, so do it once by hand:

```powershell
ssh user@host
```

ssh prints a fingerprint. Compare it with the one the machine reports (run `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the Linux machine, or ask whoever runs it). If they match, type `yes`. ssh saves the machine in your `known_hosts` file, and Gliff accepts it from then on.

## 2. Create a key

```powershell
ssh-keygen -t ed25519 -C "my-windows-pc"
```

Press Enter to accept the default file (`%USERPROFILE%\.ssh\id_ed25519`). ssh-keygen asks for a passphrase.

| Choice | What it means |
|---|---|
| No passphrase | Reconnects need nothing from you. Anyone who can run programs as you can use the key, so keep the file private. |
| A passphrase | The key is safer at rest, but a Gliff tab cannot type it. Load the key into the ssh-agent service (step 3) first. |

The file without `.pub` is your private key. Never send it to anyone. The `.pub` file is safe to share.

## 3. Use the Windows ssh-agent (only for a key with a passphrase)

The ssh-agent service is off by default, and turning it on needs an administrator. In an elevated PowerShell:

```powershell
Get-Service ssh-agent | Set-Service -StartupType Automatic
Start-Service ssh-agent
```

Then, in a normal PowerShell, add the key and check that the agent holds it:

```powershell
ssh-add $env:USERPROFILE\.ssh\id_ed25519
ssh-add -l
```

## 4. Put the public key on the machine

Show the public key:

```powershell
Get-Content $env:USERPROFILE\.ssh\id_ed25519.pub
```

It is one line that starts with `ssh-ed25519`. Add that line to `~/.ssh/authorized_keys` of your account on the Linux machine, by any means you already have: a terminal there, a console, or a tool your administrator gives you. For example, in a terminal on the Linux machine:

```sh
mkdir -p ~/.ssh
chmod 700 ~/.ssh
echo 'ssh-ed25519 AAAA...your-public-key... my-windows-pc' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

Replace the text inside the quotes with your own line.

## 5. Test it the way Gliff runs it

```powershell
ssh -o BatchMode=yes user@host true
```

`BatchMode=yes` stops ssh from asking questions, which is how Gliff starts a session. If this command prints nothing and returns, Gliff can connect. If it prints an error, the table below says what to do.

## When Gliff shows an error

Gliff reads ssh's own message and says the problem in plain words, in the tab's status and in the client log. ssh's own text stays in the session log (`ssh-stderr-<n>.log` in the log folder), and the client log names that file. In these rows, "the machine" is the name you typed after the `@`.

| Gliff says | ssh said | What to do |
|---|---|---|
| "The machine did not accept a key from this computer." | `Permission denied (publickey…)` | Repeat [steps 2](#2-create-a-key) and [4](#4-put-the-public-key-on-the-machine). If the key has a passphrase, check [step 3](#3-use-the-windows-ssh-agent-only-for-a-key-with-a-passphrase). If the machine allows passwords only, ask whoever runs it to allow keys. |
| "The machine is not known to this computer yet." | `Host key verification failed`, with no warning above it | Do [step 1](#1-trust-the-machine-once). |
| "The machine has a different identity than before." | `REMOTE HOST IDENTIFICATION HAS CHANGED` | Do not continue until you know why. See [If the machine's identity changed](#if-the-machines-identity-changed). |
| "Other users can read your key file, so Windows' ssh will not use it." | `UNPROTECTED PRIVATE KEY FILE` | See [If ssh refuses your key file](#if-ssh-refuses-your-key-file). |
| "ssh-agent offered more keys than the machine allows." | `Too many authentication failures` | See [If ssh says too many authentication failures](#if-ssh-says-too-many-authentication-failures). |
| "Nothing is listening for ssh on the machine." | `Connection refused` | Start the ssh service on the Linux machine. |
| "The machine did not answer in 10 seconds." | `Connection timed out`, `No route to host`, `Network is unreachable` | Check that it is on, awake, and reachable (same network, VPN, firewall). |
| "Can't find the machine." | `Could not resolve hostname` | Check the spelling, the network and the VPN. |
| "Windows' OpenSSH client is missing." | `'ssh' is not recognized…`, or Gliff cannot start the program | Turn on "OpenSSH Client", as in [Before you start](#before-you-start). |
| "gliff is not installed on the machine." | The sign-in worked, then the shell said `command not found` (exit 127) | Install gliff-server on the Linux machine, or give its path with `--server-path` or `server_path` in `config.toml`. |
| "Something went wrong connecting to the machine." | Anything else; the last line is shown, with file names hidden | Open the session log. The full text is there. |

### If ssh refuses your key file

ssh will not use a private key that other users can read. In PowerShell, remove the extra rights and keep your own:

```powershell
icacls $env:USERPROFILE\.ssh\id_ed25519 /inheritance:r
icacls $env:USERPROFILE\.ssh\id_ed25519 /grant:r "$($env:USERNAME):(R)"
```

Gliff never changes the file's rights itself. Then try again.

### If ssh says too many authentication failures

ssh tried more keys than the machine allows before it reached the right one. In `%USERPROFILE%\.ssh\config`, name the key under the machine's entry:

```
Host my-machine
    HostName host
    User user
    IdentityFile ~/.ssh/id_ed25519
    IdentitiesOnly yes
```

### If the machine's identity changed

ssh remembers each machine's identity in your `known_hosts` file, and the machine now shows a different one. A reinstall causes this. So can someone sitting between you and the machine, so find out which it is first: ask whoever runs the machine, and compare the fingerprint as in [step 1](#1-trust-the-machine-once). If you know it was reinstalled, remove the old entry and trust the machine again:

```powershell
ssh-keygen -R host
```

Then do [step 1](#1-trust-the-machine-once) again. Gliff never removes the entry or accepts the new identity for you.

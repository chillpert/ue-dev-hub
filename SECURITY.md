# Security

## Reporting a vulnerability

Please report it privately through GitHub: on this repository's **Security** tab, choose
**Report a vulnerability**. Don't open a public issue for it.

Say what's affected, how to reproduce it and which hub version you used. You should hear back
as soon as possible. Fixes go into the next release, and the advisory is published once it's out.

Only the latest release gets security fixes.

## In scope

- **Updates**: anything that gets the hub to install or run an update that wasn't signed with its
  updater key, or to install an older release than the one it has. This covers the installed updater,
  the portable `ue-dev-hub.exe` self-update and the "Install UE Dev Hub" flow.
- **Addon signing**: anything that gets an official addon installed or updated without a valid
  signature from the addons key, or lets one addon pass for another.
- **Addon isolation**: an addon page reaching hub commands or files it wasn't given.
- **Project settings**: a `ue-dev-hub.toml` that makes the hub do more than the settings say,
  like writing outside the project, running programs it shouldn't, or leaking logins or the webhook.
- Logins (Nextcloud) leaking out of the system keyring, or into logs and reports.

## Out of scope

- What a project's own code does when the hub builds or opens it. Opening a project runs its build
  scripts, editor modules and, if it sets `git_hooks_path`, its Git hooks. That's by design.
- What an addon does once you installed it from a zip or folder. Those are unsigned and fully trusted.
- SmartScreen warnings: the Windows files aren't Authenticode-signed.

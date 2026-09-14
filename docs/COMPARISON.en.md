# Feature and solution comparison

This is a product-positioning comparison, not a benchmark, price comparison, or security
audit. Third-party capabilities vary by version, platform, plugins, and commercial plan.
“—” means the capability is not a core built-in workflow; “external” means it normally
requires a CLI, plugin, or additional configuration. Third-party labels reflect public product
positioning and should be verified against the target platform and exact version before adoption.

## Capability matrix

| Capability | DBX SSH & SFTP | OpenSSH + sftp | Tabby | Termius | FinalShell | electerm | iSHell Pro |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Graphical connection management | Built in | — | Built in | Built in | Built in | Built in | Built in/mobile |
| Interactive PTY terminal | Built in | Built-in terminal | Built in | Built in | Built in | Built in | Built in |
| SFTP file workspace | Built in | External `sftp`/client | Plugin/version dependent | Built in/plan dependent | Built in | Built in | Built in/version dependent |
| SSH Agent / key / password auth | Built in | Built in | Built in/config dependent | Built in/plan dependent | Built in | Built in | Built in |
| Jump hosts / ProxyJump | Up to three hops | Built in | Config/plugin dependent | Supported/plan dependent | Supported/version dependent | Supported/config dependent | Supported/version dependent |
| Known Hosts and changed-key rejection | Built in | Built in | Configuration dependent | Supported/config dependent | Supported/config dependent | Supported/config dependent | Supported/version dependent |
| Port forwarding / SOCKS | Built in/config dependent | Built in | Plugin/config dependent | Supported/plan dependent | Supported/version dependent | Supported/config dependent | Supported/version dependent |
| Read-only and command safety gates | Built in | Shell/system policy | Plugin/manual policy | Configuration/plan dependent | Manual policy | Manual policy | Config/manual policy |
| Quick Sudo / TOTP interaction | Built in | External scripts/terminal flow | Plugin/script | Supported/plan dependent | Supported/version dependent | Terminal flow/version dependent | Terminal flow/version dependent |
| File preview, drag/drop, resumable transfer | Built in | External tool | Plugin/version dependent | Built in/plan dependent | Built in | Built in/version dependent | Built in/version dependent |
| Connection sync / multi-device workflow | DBX host workbench | — | Config/sync dependent | Core selling point/plan dependent | Version dependent | Cross-platform desktop | Mobile-first |
| MCP automation tools | Built in | — | — | — | — | — | — |
| DBX host secret binding | Native | — | — | — | — | — | — |
| RDP | Improving | External tool | Plugin/version dependent | Supported/plan dependent | Supported/version dependent | Version dependent | Version dependent |
| Telnet | Improving | External `telnet` | Plugin/version dependent | Supported/version dependent | Supported/version dependent | Built in/version dependent | Version dependent |
| Serial | Planned or version dependent | External tool | Plugin/version dependent | Version dependent | Version dependent | Version dependent | Mobile/version dependent |
| Seven-language plugin UI | Built in | — | Partial/version dependent | Partial/plan dependent | Partial/version dependent | Version dependent | Version dependent |

> **Protocol roadmap**: DBX SSH & SFTP currently focuses on SSH, SFTP, ProxyJump, PTY, and
> governed server operations. RDP, Telnet, and additional remote protocols are still being
> strengthened. “Improving” does not mean a complete production-ready replacement is already
> guaranteed; verify the exact capability in the release notes for your target version.

## Position in the DBX plugin family

| Plugin | Primary object | Best for |
| --- | --- | --- |
| DBX SSH Terminal | SSH hosts, terminals, SFTP, remote operations | Log in, run commands, browse and transfer files |
| DBX Files | File systems and object storage | Browse, upload/download, archive, and organize storage |
| DBX LDAP | LDAP directories | Search, aggregate, and edit directory entries |
| DBX Kafka | Kafka clusters | Topics, messages, consumer groups, and schemas |

SSH Terminal does not duplicate the dedicated Files, LDAP, or Kafka protocols. It focuses on
terminal and SFTP operations after reaching a host over SSH, while reusing DBX connection,
secret, and workbench boundaries.

## Choosing a solution

- For script-only SSH, choose OpenSSH; it is lightweight and leaves automation boundaries to
  shell and system policy.
- For terminal tabs and a customizable terminal, Tabby is a general terminal option; SFTP and
  security controls depend on plugins and configuration.
- For cross-device connection synchronization, Termius is more cloud/commercial oriented;
  exact capabilities depend on its version and plan.
- For a desktop tool combining SSH, SFTP, and server status, FinalShell is one direct option.
- If you already use DBX host connections, MCP, and plugin workbenches, DBX SSH & SFTP keeps
  those workflows inside the same connection and permission boundary.
- If you need RDP, Telnet, or additional protocols, follow the DBX roadmap; those capabilities
  are actively being strengthened and should be checked against the release notes for the
  version you plan to deploy.

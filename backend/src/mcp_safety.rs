//! Production misoperation guards for the MCP exec tools.
//!
//! Every `ssh_exec` / `ssh_exec_sudo` command is classified before it is
//! executed:
//!
//! - [`CommandRisk::ReadOnly`] — provably read-only (inspection verbs such as
//!   `ls`, `df`, `systemctl status`). The only class allowed through the
//!   read-only connection gate.
//! - [`CommandRisk::Destructive`] — matches a known catastrophic pattern
//!   (disk formatting, recursive deletes of system roots, shutdown, raw
//!   device writes, ...). Refused outright on read-only connections and
//!   requires `confirmDestructive: true` everywhere else.
//! - [`CommandRisk::Unknown`] — everything else. Allowed on normal
//!   connections, refused on read-only ones (whitelist, not blacklist:
//!   unrecognized means unproven).
//!
//! The classifier is deliberately conservative: quoted strings are not
//! parsed (a `;` inside quotes still splits segments, which can only
//! downgrade `ReadOnly` to `Unknown`, never upgrade), command substitution
//! and output redirection force `Unknown`, and privileged/wrapper prefixes
//! are unwrapped before the inner command is assessed.

/// Classification of one shell command under the safety gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandRisk {
    /// Provably read-only; allowed on read-only connections.
    ReadOnly,
    /// Matches a catastrophic pattern; carries a human-readable reason.
    Destructive(&'static str),
    /// Not provably read-only and not a known pattern; refused on
    /// read-only connections, allowed (unconfirmed) elsewhere.
    Unknown,
}

/// Classifies a full command line (which may chain several commands).
pub fn assess_command(command: &str) -> CommandRisk {
    let neutralized = neutralize_fd_dups(command);
    let mut overall = CommandRisk::ReadOnly;
    for segment in split_segments(&neutralized) {
        match assess_segment(segment) {
            CommandRisk::Destructive(reason) => return CommandRisk::Destructive(reason),
            CommandRisk::Unknown => overall = CommandRisk::Unknown,
            CommandRisk::ReadOnly => {}
        }
    }
    overall
}

/// True when any top-level command segment runs under `sudo` (after the same
/// env-prefix / wrapper unwrapping the classifier applies). Agent terminal
/// mode treats these as privilege escalation regardless of the inner verb,
/// so teaching-mode approval covers inline `sudo …`, not just `ssh_exec_sudo`.
pub fn runs_under_sudo(command: &str) -> bool {
    let neutralized = neutralize_fd_dups(command);
    split_segments(&neutralized)
        .iter()
        .any(|segment| effective_tokens(segment).first().map(String::as_str) == Some("sudo"))
}

/// Replaces fd duplications (`2>&1`, `1>&2`, ...) with a neutral token so
/// the `&` splitter does not shred them; they duplicate fds, not files.
fn neutralize_fd_dups(command: &str) -> String {
    command
        .replace("2>&1", " FDDUP ")
        .replace("1>&2", " FDDUP ")
        .replace("2>&2", " FDDUP ")
        .replace("1>&1", " FDDUP ")
        .replace(">&1", " FDDUP ")
        .replace(">&2", " FDDUP ")
}

/// Splits a command line into pipeline/chain segments. Naive on purpose:
/// quote contents are not parsed, and a split inside quotes can only make
/// the assessment more conservative.
fn split_segments(command: &str) -> Vec<&str> {
    command
        .split(|c: char| c == ';' || c == '|' || c == '\n' || c == '&')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// Verbs that are read-only in every argument shape.
const READ_ONLY_VERBS: &[&str] = &[
    "ls", "cat", "head", "tail", "grep", "egrep", "fgrep", "rg", "df", "du", "ps", "free",
    "uptime", "whoami", "id", "uname", "hostname", "w", "who", "last", "lastlog", "stat", "wc",
    "file", "cksum", "md5sum", "sha1sum", "sha256sum", "sha512sum", "echo", "printf", "date",
    "printenv", "which", "whereis", "type", "lsof", "ss", "netstat", "ping", "ping6",
    "traceroute", "tracepath", "nslookup", "dig", "host", "vmstat", "iostat", "sar", "mpstat",
    "dmesg", "lsblk", "lsmod", "lspci", "lsusb", "lsns", "lscpu", "nproc", "getent", "groups",
    "true", "false", "test", "[", "sleep", "history", "arch", "cut", "sort", "uniq", "tr",
    "column", "nl", "tac", "rev", "seq", "dirname", "basename", "readlink", "realpath", "pwd",
];

/// Verbs whose read-only-ness depends on the first subcommand.
const SUBCOMMAND_VERBS: &[(&str, &[&str])] = &[
    (
        "systemctl",
        &[
            "status", "list-units", "list-unit-files", "list-timers", "list-sockets",
            "list-dependencies", "list-jobs", "is-active", "is-enabled", "is-failed", "show",
            "cat", "help", "get-default",
        ],
    ),
    ("docker", &["ps", "images", "stats", "version", "info", "logs", "inspect", "top", "port", "events", "search"]),
    (
        "git",
        &["status", "log", "diff", "show", "branch", "blame", "describe", "rev-parse", "remote", "tag", "reflog"],
    ),
    ("kubectl", &["get", "describe", "top", "logs", "version", "explain", "api-resources", "api-versions"]),
    ("ip", &["addr", "a", "address", "l", "link", "route", "r", "rule", "neigh", "n"]),
];

/// Command prefixes that merely wrap an inner command; unwrapped before the
/// real verb is assessed.
const WRAPPER_VERBS: &[&str] = &["nohup", "timeout", "watch", "time", "nice", "ionice", "stdbuf", "env"];

/// Wrapper flags that consume the following token as their value
/// (`nice -n 10`, `ionice -c 2`, `watch -n 1`).
const WRAPPER_VALUE_FLAGS: &[&str] = &["-n", "-c", "-p"];

/// Verbs that format or wipe raw disks / power the machine off.
const DESTRUCTIVE_VERBS: &[&str] = &[
    "mkfs", "mkfs.ext2", "mkfs.ext3", "mkfs.ext4", "mkfs.xfs", "mkfs.btrfs", "mkfs.vfat",
    "mkfs.fat", "mkswap", "fdisk", "sfdisk", "cfdisk", "gdisk", "sgdisk", "parted", "partprobe",
    "wipefs", "blkdiscard", "shutdown", "reboot", "halt", "poweroff",
];

/// SQL interpreters whose arguments may carry a DROP statement.
const SQL_VERBS: &[&str] = &["mysql", "mariadb", "psql", "sqlite3"];

/// System files whose overwrite or deletion is always catastrophic.
const CRITICAL_FILES: &[&str] = &[
    "/etc/passwd", "/etc/shadow", "/etc/sudoers", "/etc/fstab", "/boot/",
];

/// Assesses one chain segment (no `;`/`&&`/`|` left inside).
fn assess_segment(segment: &str) -> CommandRisk {
    if segment.contains(":(){") {
        return CommandRisk::Destructive("fork bomb");
    }
    if segment.contains("$(") || segment.contains('`') {
        return CommandRisk::Unknown;
    }
    let tokens = effective_tokens(segment);
    let Some((verb, args)) = tokens.split_first() else {
        return CommandRisk::Unknown;
    };
    let verb = verb.as_str();

    if let Some(reason) = destructive_pattern(verb, args) {
        return CommandRisk::Destructive(reason);
    }
    if has_redirect(args) {
        // Redirection writes a file; only raw-device / critical targets are
        // destructive (checked inside destructive_pattern), everything else
        // is merely a write.
        return CommandRisk::Unknown;
    }
    if verb == "sudo" {
        // Privileged commands never count as whitelisted reads; only their
        // destructive shape matters (checked inside destructive_pattern).
        return CommandRisk::Unknown;
    }
    if READ_ONLY_VERBS.contains(&verb) {
        return CommandRisk::ReadOnly;
    }
    if verb == "find" {
        if args.iter().any(|arg| arg.starts_with("-exec") || arg == "-ok") {
            return CommandRisk::Unknown;
        }
        return CommandRisk::ReadOnly;
    }
    if verb == "journalctl" {
        if args.iter().any(|arg| arg.starts_with("--vacuum") || arg.starts_with("--rotate")) {
            return CommandRisk::Unknown;
        }
        return CommandRisk::ReadOnly;
    }
    if verb == "crontab" {
        // `crontab -r` removes the crontab and `-e` opens an editor; only
        // the listing forms are read-only.
        let listing = !args.is_empty()
            && args
                .iter()
                .all(|arg| arg == "-l" || arg == "--list");
        return if listing { CommandRisk::ReadOnly } else { CommandRisk::Unknown };
    }
    if verb == "service" {
        return if args.iter().any(|arg| arg == "status") {
            CommandRisk::ReadOnly
        } else {
            CommandRisk::Unknown
        };
    }
    if let Some((_, allowed)) = SUBCOMMAND_VERBS.iter().find(|(owner, _)| *owner == verb) {
        let subcommand = args.iter().find(|arg| !arg.starts_with('-'));
        return match subcommand {
            Some(sub) if allowed.contains(&sub.as_str()) => CommandRisk::ReadOnly,
            _ => CommandRisk::Unknown,
        };
    }
    CommandRisk::Unknown
}

/// Tokenizes a segment, strips leading `FOO=bar` assignments, and unwraps
/// prefix wrappers (`timeout 10 df`, `nohup ./job`, `env X=1 cmd`, ...) so
/// the inner command is what gets assessed.
fn effective_tokens(segment: &str) -> Vec<String> {
    let mut tokens: Vec<String> = segment
        .split_whitespace()
        .map(|token| token.trim_matches(|c: char| c == '"' || c == '\'').to_string())
        .collect();
    // Assignments and wrappers are unwrapped iteratively with a depth cap so
    // pathological input cannot loop forever.
    for _ in 0..16 {
        strip_leading_assignments(&mut tokens);
        let Some(verb) = tokens.first().cloned() else {
            break;
        };
        if !WRAPPER_VERBS.contains(&verb.as_str()) {
            break;
        }
        tokens.remove(0);
        strip_wrapper_flags(&mut tokens);
        if verb == "timeout" {
            // `timeout` takes a duration argument before the command.
            if tokens.first().map(|token| !token.starts_with('-')).unwrap_or(false) {
                tokens.remove(0);
            }
        }
    }
    strip_leading_assignments(&mut tokens);
    tokens
}

fn strip_leading_assignments(tokens: &mut Vec<String>) {
    while tokens
        .first()
        .map(|token| {
            let Some(eq) = token.find('=') else { return false };
            eq > 0
                && token[..eq]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && token[..eq].chars().next().is_some_and(|c| !c.is_ascii_digit())
        })
        .unwrap_or(false)
    {
        tokens.remove(0);
    }
}

/// Drops leading wrapper flags; flags in [`WRAPPER_VALUE_FLAGS`] also drop
/// the token after them (their value).
fn strip_wrapper_flags(tokens: &mut Vec<String>) {
    while let Some(first) = tokens.first() {
        if !first.starts_with('-') {
            return;
        }
        let takes_value = WRAPPER_VALUE_FLAGS.contains(&first.as_str());
        tokens.remove(0);
        if takes_value && tokens.first().map(|token| !token.starts_with('-')).unwrap_or(false) {
            tokens.remove(0);
        }
    }
}

/// Recognized catastrophic patterns for one (verb, args) pair. Raw-device
/// and critical-file redirection is checked here (not in `has_redirect`)
/// because it must stay `Destructive` rather than merely `Unknown`.
fn destructive_pattern(verb: &str, args: &[String]) -> Option<&'static str> {
    if DESTRUCTIVE_VERBS.contains(&verb) {
        return Some("formats or wipes a disk / powers the machine off");
    }
    if matches!(verb, "init" | "telinit") && args.iter().any(|arg| arg == "0" || arg == "6") {
        return Some("init 0/6 shuts down or reboots the machine");
    }
    if verb == "dd" {
        if let Some(output) = args.iter().find(|arg| arg.starts_with("of=")) {
            if is_raw_device(&output["of=".len()..]) {
                return Some("dd writes to a raw device");
            }
        }
    }
    if verb == "rm" {
        let recursive = args.iter().any(|arg| {
            (arg.starts_with('-') && !arg.starts_with("--")
                && arg.len() > 1
                && arg[1..].chars().any(|c| c == 'r' || c == 'R'))
                || arg == "--recursive"
        });
        for target in args.iter().filter(|arg| !arg.starts_with('-')) {
            if is_critical_file(target) {
                return Some("rm targets a critical system file");
            }
            if recursive && destructive_delete_target(target) {
                return Some("recursive rm targets a system root");
            }
        }
    }
    if matches!(verb, "chmod" | "chown") {
        let recursive = args.iter().any(|arg| arg.starts_with("-R") || arg == "--recursive");
        for target in args.iter().filter(|arg| !arg.starts_with('-') && !arg.contains('=')) {
            if is_critical_file(target) {
                return Some("chmod/chown targets a critical system file");
            }
            if recursive && destructive_delete_target(target) {
                return Some("recursive chmod/chown targets a system root");
            }
        }
    }
    if verb == "truncate"
        && args.iter().any(|arg| !arg.starts_with('-') && is_critical_file(arg))
    {
        return Some("truncate targets a critical system file");
    }
    if verb == "find" && args.iter().any(|arg| arg == "-delete") {
        return Some("find -delete removes files");
    }
    if verb == "docker" && args.iter().any(|arg| arg == "prune") {
        return Some("docker prune deletes images/containers/volumes");
    }
    if verb == "kill" {
        // `kill -9 -1` / `kill -1` signal every process the user owns.
        if args.iter().any(|arg| arg == "-1" || arg == "-9") {
            let pid_like = args.iter().filter(|arg| !arg.starts_with('-')).count();
            if pid_like == 0 {
                return Some("kill signals every process (pid -1)");
            }
        }
    }
    if SQL_VERBS.contains(&verb) {
        let joined = args.join(" ").to_ascii_lowercase();
        if joined.contains("drop database") || joined.contains("drop table") {
            return Some("SQL DROP statement");
        }
    }
    for (index, token) in args.iter().enumerate() {
        if is_redirect_token(token) {
            if let Some(target) = args.get(index + 1) {
                if is_raw_device(target) {
                    return Some("redirection writes to a raw device");
                }
                if is_critical_file(target) {
                    return Some("redirection overwrites a critical system file");
                }
            }
        }
    }
    None
}

/// True when the token opens an output redirection.
fn is_redirect_token(token: &str) -> bool {
    token.starts_with('>')
        || token.starts_with("&>")
        || token.starts_with("2>")
        || token.starts_with("1>")
}

/// True when the token list contains an output redirection.
fn has_redirect(args: &[String]) -> bool {
    args.iter().any(|token| is_redirect_token(token))
}

/// `/dev/sda`, `/dev/nvme0n1`, ... — block devices a shell must never touch
/// directly. `/dev/null` and `/dev/zero` deliberately do not match.
fn is_raw_device(path: &str) -> bool {
    const PREFIXES: &[&str] = &["/dev/sd", "/dev/nvme", "/dev/vd", "/dev/mmcblk", "/dev/disk/"];
    PREFIXES.iter().any(|prefix| path.starts_with(prefix))
}

fn is_critical_file(path: &str) -> bool {
    CRITICAL_FILES.iter().any(|critical| {
        path == *critical || critical.ends_with('/') && path.starts_with(*critical)
    })
}

/// Recursive-delete target rule: the filesystem root and everything up to
/// two components deep, except under `/tmp` / `/var/tmp` where cleanup is a
/// routine operation.
fn destructive_delete_target(target: &str) -> bool {
    let target = target.trim_end_matches('/');
    if target.is_empty() {
        return true; // "/" (or "//") itself
    }
    if matches!(target, "*" | "." | ".." | "./*" | "~" | "$HOME" | "${HOME}" | "~/*") {
        return true;
    }
    let Some(path) = target.strip_prefix('/') else {
        return false;
    };
    let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    match components.first() {
        None => true, // "/" itself
        Some(&"tmp") | Some(&"var") => {
            // `/tmp`, `/var`, `/var/tmp` are routine cleanup targets.
            components.len() <= 1 || components.len() == 2 && components[1] == "tmp"
        }
        // Wildcards only escalate at shallow depth: `/*` and `/etc/*` wipe a
        // root, but `/root/.cache/*` is routine cleanup.
        _ => components.len() <= 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CommandRisk::{Destructive, ReadOnly, Unknown};

    fn read_only(command: &str) {
        assert_eq!(assess_command(command), ReadOnly, "expected ReadOnly: {command}");
    }

    fn unknown(command: &str) {
        assert_eq!(assess_command(command), Unknown, "expected Unknown: {command}");
    }

    fn destructive(command: &str) {
        assert!(
            matches!(assess_command(command), Destructive(_)),
            "expected Destructive: {command}"
        );
    }

    #[test]
    fn inspection_commands_are_read_only() {
        read_only("df -h");
        read_only("ls -la /var/log");
        read_only("cat /etc/hostname");
        read_only("systemctl status nginx");
        read_only("systemctl is-active sshd");
        read_only("journalctl -u sshd -n 50 --no-pager");
        read_only("docker ps -a");
        read_only("docker logs --tail 100 c1");
        read_only("git log --oneline -5");
        read_only("ps aux | grep mysqld | grep -v grep");
        read_only("ps aux 2>&1 | grep sshd");
        read_only("du -xh --max-depth=1 /www | sort -rh | head -10");
        read_only("echo hello");
        read_only("free -m && uptime");
        read_only("ip a");
        read_only("ip addr show eth0");
        read_only("crontab -l");
        read_only("service nginx status");
    }

    #[test]
    fn runs_under_sudo_detects_inline_privilege_escalation() {
        assert!(super::runs_under_sudo("sudo whoami"));
        assert!(super::runs_under_sudo("sudo -u postgres psql -l"));
        assert!(super::runs_under_sudo("echo hi && sudo reboot"));
        assert!(super::runs_under_sudo("FOO=1 sudo id"));
        assert!(!super::runs_under_sudo("echo sudo is a word here"));
        assert!(!super::runs_under_sudo("whoami"));
        assert!(!super::runs_under_sudo("ls && cat /etc/hostname"));
    }

    #[test]
    fn mutating_or_unclear_commands_are_unknown() {
        unknown("rm -rf /home/vagrant/acct-main");
        unknown("systemctl restart nginx");
        unknown("systemctl stop mysqld");
        unknown("echo x > /tmp/out.txt");
        unknown("cat /etc/passwd >> /tmp/copy");
        unknown("docker system df -v"); // `system` is not a whitelisted subcommand
        unknown("sed -i s/a/b/ file");
        unknown("awk '{print $1}' file");
        unknown("mysql -e 'UPDATE t SET x=1'");
        unknown("sudo cat /var/log/auth.log");
        unknown("env FOO=1 rm -rf /home/vagrant/junk");
        unknown("echo $(whoami)");
        unknown("crontab -e");
        unknown("crontab -r");
        unknown("journalctl --vacuum-size=100M");
        unknown("find /var -name '*.log' -exec rm {} +");
    }

    #[test]
    fn catastrophic_patterns_are_destructive() {
        destructive("rm -rf /");
        destructive("rm -rf /*");
        destructive("rm -fr /etc");
        destructive("rm -rf /etc/nginx");
        destructive("rm --recursive /usr/local");
        destructive("rm -rf ~");
        destructive("rm -rf $HOME");
        destructive("rm -rf .");
        destructive("rm -rf *");
        destructive("rm -f /etc/passwd");
        destructive("mkfs.ext4 /dev/sda1");
        destructive("wipefs -a /dev/sdb");
        destructive("dd if=/dev/zero of=/dev/sda");
        destructive("dd if=img.raw of=/dev/nvme0n1 bs=4M");
        destructive("echo x > /dev/sda");
        destructive("shutdown -h now");
        destructive("reboot");
        destructive("init 0");
        destructive("telinit 6");
        destructive(":(){ :|:& };:");
        destructive("chmod -R 777 /");
        destructive("chmod -R 755 /etc");
        destructive("chown -R nobody /usr");
        destructive("echo hack >> /etc/sudoers");
        destructive("truncate -s 0 /etc/shadow");
        destructive("mysql -e 'DROP DATABASE prod'");
        destructive("psql -c 'drop table users'");
        destructive("docker system prune -af");
        destructive("find / -delete");
        destructive("kill -9 -1");
        destructive("rm -rf / ; echo done");
        destructive("uptime && mkfs.xfs /dev/vdb");
    }

    #[test]
    fn routine_cleanup_stays_out_of_the_destructive_class() {
        // Real-world cleanup commands must not demand confirmation.
        unknown("rm -rf /root/.cache/*");
        unknown("rm -rf /lib/modules/5.4.0-117-generic");
        unknown("truncate -s 0 /www/server/data/vagrant.err");
        unknown("apt-get clean");
        unknown("journalctl --rotate && sleep 3");
        unknown("rm -rf /tmp/dbx-mcp-smoke-dir");
        unknown("dd if=/dev/zero of=/dev/null bs=1M count=10");
    }

    #[test]
    fn wrappers_and_assignments_are_unwrapped_conservatively() {
        read_only("nohup df -h");
        read_only("timeout 30 du -sh /var");
        read_only("nice -n 10 ls /");
        read_only("watch -n 1 uptime");
        read_only("FOO=bar BAR=baz df -h");
        destructive("timeout 10 rm -rf /data");
        destructive("FOO=bar rm -rf /data");
        unknown("timeout 10 systemctl restart nginx");
    }

    #[test]
    fn quoting_is_not_parsed_and_stays_conservative() {
        // A `;` inside quotes splits segments; both halves must be read-only
        // for the whole command to pass. Here the second half (`b' x ...`)
        // is unrecognized, so the fd-dup neutralization cannot save it —
        // exactly the fail-closed behavior we want.
        unknown("grep -F 'a;b' x 2>&1 || true");
        read_only("grep -F a.b x 2>&1 || true");
        unknown("grep -F 'a;b' /etc/hosts"); // second half: `b' /etc/hosts`
        read_only("echo \"line1\"; df -h");
        unknown("echo \"x\"; systemctl restart nginx");
    }

    #[test]
    fn destructive_classification_names_the_pattern() {
        assert!(matches!(assess_command("reboot"), Destructive(_)));
        assert_eq!(assess_command("df -h"), ReadOnly);
        assert_eq!(
            assess_command("rm -rf /etc"),
            Destructive("recursive rm targets a system root")
        );
    }
}

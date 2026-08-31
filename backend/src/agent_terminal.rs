//! AI terminal synchronous execution (agent terminal mode): pure routing,
//! sanitizing, and output-capture logic for commands an AI/MCP client runs
//! inside the user's interactive workbench terminal. No SSH I/O lives here —
//! everything is unit-testable without a connection (see the M1/M2 sections
//! of `docs/IMPL_PLAN_AGENT_TERMINAL.zh-CN.md`).

use std::time::{Duration, Instant};

use crate::exec;

/// Connection-level agent terminal mode (`agentTerminalMode`). Kept
/// in-memory per connection like the Quick Sudo field overrides: a sidecar
/// restart falls back to [`AgentTerminalMode::Off`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentTerminalMode {
    #[default]
    Off,
    Auto,
    Strict,
}

impl AgentTerminalMode {
    /// Lenient parse for stored/forwarded values: anything unknown degrades
    /// to `Off` instead of failing the connection.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "strict" => Self::Strict,
            _ => Self::Off,
        }
    }

    /// Strict parse for `ssh/settings/set`: an unknown value must be refused
    /// so a typo cannot silently disable the approval machinery. Built on
    /// [`Self::parse`] with a canonical-name round-trip check.
    pub fn parse_exact(value: &str) -> Result<Self, String> {
        let parsed = Self::parse(value);
        if parsed.name() == value.trim().to_ascii_lowercase() {
            Ok(parsed)
        } else {
            Err(format!(
                "agentTerminalMode must be \"off\", \"auto\", or \"strict\"; got '{value}'"
            ))
        }
    }

    /// Canonical lowercase name used in settings and event payloads.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Strict => "strict",
        }
    }
}

/// Risk of one MCP command as seen by the terminal routing. Sudo commands
/// and catastrophic `mcp_safety` hits are elevated; everything else is low.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRisk {
    Low,
    Elevated,
}

impl CommandRisk {
    /// Canonical name carried in `ssh/agent/prompt` / `ssh/agent/notice`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Elevated => "elevated",
        }
    }
}

/// Verdict of the §1 policy matrix for a terminal-routed command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingDecision {
    /// Inject into the terminal directly (a notice event is still emitted).
    Run,
    /// Raise an approval challenge and wait for `ssh/agent/resolve`.
    Prompt,
    /// Refuse the terminal path outright (the AI keeps the hidden channel).
    Deny(&'static str),
}

/// The §1 matrix: `auto` approves low-risk commands and prompts for
/// elevated ones; `strict` prompts for everything.
///
/// `Off` only reaches here when `runInTerminal: true` forced the terminal
/// path (otherwise the routing never leaves the hidden channel): low-risk
/// commands may still run because they are plain visibility, but elevated
/// ones are denied — the user never opted into agent approvals, and the
/// audited hidden channel (with its `confirmDestructive` gate) stays the
/// path for privileged work.
pub fn decide(mode: AgentTerminalMode, risk: CommandRisk) -> RoutingDecision {
    match (mode, risk) {
        (AgentTerminalMode::Off, CommandRisk::Low) => RoutingDecision::Run,
        (
            AgentTerminalMode::Off,
            CommandRisk::Elevated,
        ) => RoutingDecision::Deny(
            "Agent terminal mode is off; enable agentTerminalMode (auto or strict) to run \
             elevated commands in the terminal",
        ),
        (AgentTerminalMode::Auto, CommandRisk::Low) => RoutingDecision::Run,
        (AgentTerminalMode::Auto, CommandRisk::Elevated) => RoutingDecision::Prompt,
        (AgentTerminalMode::Strict, CommandRisk::Low) => RoutingDecision::Prompt,
        (AgentTerminalMode::Strict, CommandRisk::Elevated) => RoutingDecision::Prompt,
    }
}

/// Decision delivered through an approval challenge's oneshot channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentDecision {
    /// Approved, optionally with the user-edited command text (the approval
    /// dialog shows the command editable, so what was approved is exactly
    /// what gets typed into the terminal).
    Approve { command: Option<String> },
    Deny,
}

/// Strips C0 control characters (notably `\x03` interrupt and `\x1b`
/// escape, which could otherwise drive the terminal or the auto-sudo state
/// machine) while keeping `\n` and `\t`. Trims surrounding whitespace and
/// refuses the empty remainder.
pub fn sanitize_command(command: &str) -> Result<String, String> {
    let cleaned: String = command
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return Err("Command is empty after removing control characters".to_string());
    }
    Ok(trimmed.to_string())
}

/// Terminal text injected for an `ssh_exec_sudo` tool call. The tool *means*
/// "run this with sudo", but the terminal path types raw text into the user's
/// shell, so the `sudo` prefix must be part of the injected command or the
/// privilege escalation is silently lost. Idempotent: a command already
/// carrying the prefix (inline from the AI, or edited in by the approval
/// dialog) is returned unchanged so `sudo sudo` can never happen. The prefix
/// lands on the first line only — multi-line commands execute line by line,
/// which is a known limitation of the injection path. An empty command
/// degrades to a bare `sudo ` here; emptiness is rejected upstream by
/// [`sanitize_command`], so this function never has to police it.
pub fn sudo_command_text(command: &str) -> String {
    let first_line = command.lines().next().unwrap_or_default().trim_start();
    if first_line.starts_with("sudo ") || first_line == "sudo" {
        command.to_string()
    } else {
        format!("sudo {command}")
    }
}

/// Removes ANSI escape sequences: CSI, OSC/DCS-style string sequences
/// (BEL or `ESC \` terminated), and other two-byte ESC sequences.
/// Incomplete sequences at the end of the input are swallowed.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                // CSI: parameter bytes, intermediate bytes, then one final
                // byte in 0x40..=0x7e terminates the sequence.
                while let Some(next) = chars.next() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            // String sequences (OSC, DCS, SOS, PM, APC).
            Some(']' | 'P' | 'X' | '^' | '_') => {
                let mut pending_escape = false;
                for next in chars.by_ref() {
                    if pending_escape {
                        if next == '\\' {
                            break;
                        }
                        // A stray ESC inside the string just continues it.
                        pending_escape = false;
                        continue;
                    }
                    match next {
                        '\x07' => break,
                        '\x1b' => pending_escape = true,
                        _ => {}
                    }
                }
            }
            // Two-byte escapes (ESC 7, ESC (, ...) were consumed above.
            Some(_) => {}
            None => {}
        }
    }
    out
}

/// Capture states of a terminal recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderState {
    Idle,
    Capturing,
    Settled,
}

/// Silence after the last output chunk before a seen prompt counts as
/// "command finished".
pub const SILENCE_DEBOUNCE: Duration = Duration::from_millis(300);

/// Fallback silence for shells without local echo: after this much quiet
/// following a prompt, the capture settles even without a seen echo.
pub const ECHO_FALLBACK_SILENCE: Duration = Duration::from_secs(2);

/// Cap of the raw capture buffer (newest output wins).
const RECORDER_BUFFER_CAP: usize = 1024 * 1024;
/// Cap of the rolling tail used for prompt detection, kept small so the
/// scan never walks the full buffer on every chunk.
const RECORDER_TAIL_CAP: usize = 8 * 1024;

/// Bounded output recorder installed on a session while an agent command
/// runs in the user's terminal. The PTY read loop feeds every stdout chunk
/// into [`TerminalRecorder::observe`]; the capture ends when a fresh shell
/// prompt has been seen and the terminal went silent for
/// [`SILENCE_DEBOUNCE`], or when the caller forces
/// [`TerminalRecorder::finish`] (timeout / denial paths).
///
/// Known limits (accepted by design): an ANSI sequence split across chunk
/// boundaries can leave residue in the buffer, multi-line commands execute
/// line by line, and full-screen TUIs never show a prompt (they run into
/// the timeout path and return partial output).
pub struct TerminalRecorder {
    state: RecorderState,
    buffer: String,
    tail: String,
    prompt_seen: bool,
    echo_seen: bool,
    last_chunk_at: Option<Instant>,
    silence_debounce: Duration,
    /// Fragment of the injected command whose echo proves the shell actually
    /// started executing it (guards against settling on the login banner
    /// prompt before the typed command was processed).
    echo_fragment: String,
}

impl Default for TerminalRecorder {
    fn default() -> Self {
        Self {
            state: RecorderState::Idle,
            buffer: String::new(),
            tail: String::new(),
            prompt_seen: false,
            echo_seen: false,
            last_chunk_at: None,
            silence_debounce: SILENCE_DEBOUNCE,
            echo_fragment: String::new(),
        }
    }
}

impl TerminalRecorder {
    /// Starts a fresh capture, discarding any previous content. The
    /// `echo_fragment` (a slice of the injected command) gates settling: a
    /// prompt-silence without the command's echo means the shell has not
    /// processed the injection yet.
    pub fn arm(&mut self, echo_fragment: &str) {
        self.state = RecorderState::Capturing;
        self.buffer.clear();
        self.tail.clear();
        self.prompt_seen = false;
        self.echo_seen = false;
        self.last_chunk_at = None;
        self.echo_fragment = echo_fragment.chars().take(32).collect();
    }

    /// Feeds one chunk of PTY output. Returns the recorder state; a chunk
    /// arriving after the capture already settled reports `Settled` before
    /// absorbing the straggler.
    pub fn observe(&mut self, chunk: &str) -> RecorderState {
        if self.state != RecorderState::Capturing {
            return self.state;
        }
        if self.is_settled() {
            self.state = RecorderState::Settled;
            return self.state;
        }
        push_bounded(&mut self.buffer, chunk, RECORDER_BUFFER_CAP);
        push_bounded(&mut self.tail, chunk, RECORDER_TAIL_CAP);
        // Prompt detection reuses exec.rs's shell-prompt heuristic on the
        // ANSI-stripped tail, so colored prompts still match.
        if exec::has_shell_prompt(&strip_ansi(&self.tail)) {
            self.prompt_seen = true;
        }
        if !self.echo_seen && !self.echo_fragment.is_empty() && self.tail.contains(&self.echo_fragment)
        {
            self.echo_seen = true;
        }
        self.last_chunk_at = Some(Instant::now());
        self.state
    }

    /// True when the command's echo was seen, a prompt followed and the
    /// terminal went silent for [`SILENCE_DEBOUNCE`]. As a fallback for
    /// shells without local echo, a long [`ECHO_FALLBACK_SILENCE`] silence
    /// after a prompt settles too; [`TerminalRecorder::finish`] forces the
    /// capture closed regardless.
    pub fn is_settled(&self) -> bool {
        match self.state {
            RecorderState::Settled => true,
            RecorderState::Capturing => {
                let silence = self
                    .last_chunk_at
                    .is_some_and(|at| at.elapsed() >= self.silence_debounce);
                let long_silence = self
                    .last_chunk_at
                    .is_some_and(|at| at.elapsed() >= ECHO_FALLBACK_SILENCE);
                self.prompt_seen
                    && ((self.echo_seen && silence) || long_silence)
            }
            RecorderState::Idle => false,
        }
    }

    /// Forces the capture closed (timeout / denial path).
    pub fn finish(&mut self) {
        self.state = RecorderState::Settled;
    }

    /// Returns the captured output: ANSI-stripped text with, best-effort,
    /// the echoed command line and the trailing prompt line removed.
    pub fn take_output(&mut self, command: &str) -> String {
        self.state = RecorderState::Settled;
        let text = strip_ansi(&self.buffer);
        let mut lines: Vec<&str> = text.lines().collect();
        // The echoed command line contains a fragment of the command itself
        // (usually behind the prompt); drop exactly one such leading line.
        let fragment: String = command
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .chars()
            .take(24)
            .collect();
        if !fragment.is_empty() && lines.first().is_some_and(|line| line.contains(&fragment)) {
            lines.remove(0);
        }
        trim_trailing_blank_lines(&mut lines);
        if looks_like_prompt_line(lines.last().copied()) {
            lines.pop();
            trim_trailing_blank_lines(&mut lines);
        }
        lines.join("\n")
    }
}

/// Single-line prompt check mirroring the tail of `exec.rs::has_shell_prompt`
/// (line trimmed of trailing spaces ending in `$` or `#`); the exec helper
/// operates on whole texts, so the one-line shape is duplicated here.
fn looks_like_prompt_line(line: Option<&str>) -> bool {
    line.is_some_and(|line| {
        let line = line.trim_end();
        line.ends_with('$') || line.ends_with('#')
    })
}

fn trim_trailing_blank_lines(lines: &mut Vec<&str>) {
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
}

/// Appends `chunk` keeping at most the newest `cap` bytes, cutting at a
/// char boundary so the buffer stays valid UTF-8.
fn push_bounded(buffer: &mut String, chunk: &str, cap: usize) {
    buffer.push_str(chunk);
    if buffer.len() > cap {
        let mut cut = buffer.len() - cap;
        while !buffer.is_char_boundary(cut) {
            cut += 1;
        }
        buffer.drain(..cut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_covers_the_full_policy_matrix() {
        // off: only forced low-risk runs; elevated is refused outright.
        assert_eq!(
            decide(AgentTerminalMode::Off, CommandRisk::Low),
            RoutingDecision::Run
        );
        assert!(matches!(
            decide(AgentTerminalMode::Off, CommandRisk::Elevated),
            RoutingDecision::Deny(_)
        ));
        // auto: low runs, elevated needs approval.
        assert_eq!(
            decide(AgentTerminalMode::Auto, CommandRisk::Low),
            RoutingDecision::Run
        );
        assert_eq!(
            decide(AgentTerminalMode::Auto, CommandRisk::Elevated),
            RoutingDecision::Prompt
        );
        // strict: everything needs approval.
        assert_eq!(
            decide(AgentTerminalMode::Strict, CommandRisk::Low),
            RoutingDecision::Prompt
        );
        assert_eq!(
            decide(AgentTerminalMode::Strict, CommandRisk::Elevated),
            RoutingDecision::Prompt
        );
    }

    #[test]
    fn mode_parse_degrades_and_parse_exact_refuses_unknown() {
        assert_eq!(AgentTerminalMode::parse("auto"), AgentTerminalMode::Auto);
        assert_eq!(AgentTerminalMode::parse(" STRICT "), AgentTerminalMode::Strict);
        assert_eq!(AgentTerminalMode::parse("bogus"), AgentTerminalMode::Off);
        assert_eq!(AgentTerminalMode::parse(""), AgentTerminalMode::Off);

        assert_eq!(AgentTerminalMode::parse_exact("off"), Ok(AgentTerminalMode::Off));
        assert!(AgentTerminalMode::parse_exact("Auto").is_ok());
        assert!(AgentTerminalMode::parse_exact("bogus").is_err());
        assert!(AgentTerminalMode::parse_exact("").is_err());
        let error = AgentTerminalMode::parse_exact("on").unwrap_err();
        assert!(error.contains("off"), "unexpected error: {error}");
    }

    #[test]
    fn sanitize_strips_control_characters_and_keeps_newlines() {
        // \x03 (interrupt) and \x1b (escape) must never reach the terminal.
        assert_eq!(sanitize_command("echo\x03 hi").unwrap(), "echo hi");
        assert_eq!(sanitize_command("echo\x1b[31m hi").unwrap(), "echo[31m hi");
        // Whitespace controls that carry meaning survive.
        assert_eq!(sanitize_command("echo \n\thi").unwrap(), "echo \n\thi");
        // Surrounding whitespace is trimmed.
        assert_eq!(sanitize_command("  echo hi \n").unwrap(), "echo hi");
        // Empty (or control-only) commands are refused.
        assert!(sanitize_command("").is_err());
        assert!(sanitize_command("\x03\x1b \n\t ").is_err());
        assert!(sanitize_command("   ").is_err());
    }

    #[test]
    fn strip_ansi_removes_csi_osc_and_plain_escapes() {
        assert_eq!(strip_ansi("\x1b[1mbold\x1b[0m text"), "bold text");
        assert_eq!(strip_ansi("\x1b[2J\x1b[Hcleared"), "cleared");
        assert_eq!(
            strip_ansi("\x1b]0;window title\x07body"),
            "body",
            "OSC terminated by BEL"
        );
        assert_eq!(
            strip_ansi("\x1b]8;;http://x\x1b\\link"),
            "link",
            "OSC terminated by ST"
        );
        assert_eq!(strip_ansi("\x1b7saved"), "saved", "two-byte escape");
        assert_eq!(strip_ansi("plain text"), "plain text");
        assert_eq!(strip_ansi("trailing \x1b"), "trailing ", "dangling ESC");
        assert_eq!(strip_ansi("trailing \x1b[3"), "trailing ", "dangling CSI");
    }

    #[test]
    fn recorder_settles_after_prompt_and_silence() {
        let mut recorder = TerminalRecorder::default();
        assert_eq!(recorder.observe("ignored"), RecorderState::Idle);

        recorder.arm("echo dbx-agent-marker");
        assert_eq!(recorder.observe("echo dbx-agent-marker\r\n"), RecorderState::Capturing);
        assert!(!recorder.is_settled(), "no prompt seen yet");
        assert_eq!(recorder.observe("out-a\r\nout-b\r\n"), RecorderState::Capturing);
        // The prompt may arrive in its own chunk (and split across chunks).
        recorder.observe("user@host");
        recorder.observe(":~$ ");
        assert!(!recorder.is_settled(), "silence debounce has not elapsed");
        std::thread::sleep(SILENCE_DEBOUNCE + Duration::from_millis(50));
        assert!(recorder.is_settled());

        let output = recorder.take_output("echo dbx-agent-marker");
        assert_eq!(output, "out-a\nout-b", "echo and trailing prompt are stripped");
    }

    #[test]
    fn recorder_finish_forces_the_capture_closed() {
        let mut recorder = TerminalRecorder::default();
        recorder.arm("never echoed");
        recorder.observe("partial output without any prompt");
        assert!(!recorder.is_settled());
        recorder.finish();
        assert_eq!(recorder.observe("straggler"), RecorderState::Settled);
        assert!(recorder.is_settled());
        assert_eq!(recorder.take_output("never echoed"), "partial output without any prompt");
    }

    #[test]
    fn recorder_buffer_stays_bounded_and_keeps_the_newest_output() {
        let mut recorder = TerminalRecorder::default();
        recorder.arm("echo not-present");
        // 1.5 MiB total: the head must be evicted, the tail must survive.
        let chunk = "x".repeat(4096);
        for _ in 0..384 {
            recorder.observe(&chunk);
        }
        let marker = "dbx-agent-tail-marker\n$ ";
        recorder.observe(marker);
        recorder.finish();
        let output = recorder.take_output("echo not-present");
        assert!(
            output.contains("dbx-agent-tail-marker"),
            "tail lost: {} bytes",
            output.len()
        );
        assert!(
            output.len() < 1024 * 1024 + 256,
            "buffer cap breached: {} bytes",
            output.len()
        );
    }

    #[test]
    fn take_output_handles_multiline_echo_and_blank_padding() {
        let mut recorder = TerminalRecorder::default();
        recorder.arm("echo first");
        recorder.observe("root@host:/# echo first\r\nline1\r\n\r\nroot@host:/# ");
        assert_eq!(recorder.take_output("echo first"), "line1");
    }

    #[test]
    fn sudo_command_text_never_duplicates_an_existing_prefix() {
        // Already prefixed: returned untouched (no `sudo sudo`).
        assert_eq!(sudo_command_text("sudo whoami"), "sudo whoami");
        assert_eq!(sudo_command_text("sudo -E ls -la"), "sudo -E ls -la");
        // Leading whitespace on the first line still counts as prefixed; the
        // original text passes through and `sanitize_command` trims it later.
        assert_eq!(sudo_command_text("  sudo id"), "  sudo id");
        // Bare `sudo` is a prefix too, not a command to wrap again.
        assert_eq!(sudo_command_text("sudo"), "sudo");
    }

    #[test]
    fn sudo_command_text_prefixes_plain_commands_first_line_only() {
        assert_eq!(sudo_command_text("whoami"), "sudo whoami");
        // The prefix lands on the first line only; the remainder is left as
        // typed because multi-line input executes line by line (known
        // limitation of the injection path).
        assert_eq!(sudo_command_text("whoami\nid"), "sudo whoami\nid");
        // A `sudo` on a later line does not count as prefixed: the first
        // line decides, otherwise the intended command would be lost.
        assert_eq!(
            sudo_command_text("echo hi\nsudo id"),
            "sudo echo hi\nsudo id"
        );
        // Empty input is rejected upstream by `sanitize_command`; here it
        // degrades to a bare prefix instead of panicking.
        assert_eq!(sudo_command_text(""), "sudo ");
    }
}

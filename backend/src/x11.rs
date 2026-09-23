//! X11 forwarding protocol pieces (OpenSSH `-X` parity, MVP).
//!
//! Everything in this module is pure and unit-tested: DISPLAY parsing, the
//! one-shot fake cookie handed to the server, the first X-setup-packet
//! inspection (byte-order aware, OpenSSH-compatible), `.Xauthority` parsing
//! for the local display's real cookie, and the per-connection admission
//! gate. The async bridge relaying an accepted channel into the local X
//! server is wired from `ssh.rs` (see `server_channel_open_x11` there):
//! russh's default handler accepts every incoming x11 channel, so the
//! explicit fail-closed gate below is the security boundary
//! (docs/SPIKE_X11_FORWARDING.zh-CN.md §3.4).
//!
//! No DISPLAY is probed anywhere in this module, so the tests never depend
//! on an X server being present (CI runners have none).

// The bridge consumer lands in the next commit; everything below is
// exercised from unit tests meanwhile.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// The only authorization protocol this implementation produces and accepts.
pub(crate) const X11_AUTH_PROTOCOL: &str = "MIT-MAGIC-COOKIE-1";

/// Fake-cookie size in bytes; OpenSSH generates the same 16-byte value.
const COOKIE_LEN: usize = 16;

/// Upper bound for a display number: the TCP form maps to port `6000 + n`,
/// which must stay a valid `u16`.
const MAX_DISPLAY_NUMBER: u32 = u16::MAX as u32 - 6000;

/// Local X server endpoint resolved from a DISPLAY string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayTarget {
    /// Display number (`:10.0` -> 10). The screen suffix is ignored: the
    /// bridged connection carries the client's own screen selection.
    pub(crate) number: u32,
    /// Where the local X server listens.
    pub(crate) server: DisplayServer,
}

/// Where the local X server listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DisplayServer {
    /// Preferred transport: AF_UNIX socket (the X(7) `/tmp/.X11-unix/X<n>`
    /// convention, or an explicit path — macOS launchd hands out per-boot
    /// socket paths, Windows has no unix sockets).
    UnixSocket(PathBuf),
    /// `host:n` form: pinned to loopback `6000 + n`. The DISPLAY hostname is
    /// deliberately ignored so a remote-influenced value cannot steer the
    /// local dial away from 127.0.0.1 (spike §3.5).
    TcpLoopback(u16),
}

/// Parses a DISPLAY value into the local endpoint to bridge to (X(7)):
/// `:0[.s]`, `unix:0`, `unix:/path[:0]` and `/path:0` (macOS launchd) are
/// unix sockets; `host:0[.s]` is TCP loopback `6000 + n`. `None` on
/// anything unparsable — callers surface that as "no local X server"
/// (macOS: XQuartz, Windows: VcXsrv).
pub(crate) fn parse_display(display: &str) -> Option<DisplayTarget> {
    let text = display.trim();
    // `unix:` prefix (X(7)): a bare number, or an explicit socket path with
    // an optional display-number suffix.
    if let Some(rest) = text.strip_prefix("unix:") {
        if rest.is_empty() {
            return Some(unix_display(0));
        }
        if let Some(path) = rest.strip_prefix('/') {
            let (path, number) = match path.rsplit_once(':') {
                Some((head, number)) => (head, number),
                None => (path, ""),
            };
            let number = if number.is_empty() {
                0
            } else {
                parse_display_number(number)?
            };
            return Some(DisplayTarget {
                number,
                server: DisplayServer::UnixSocket(PathBuf::from(format!("/{path}"))),
            });
        }
        let number = parse_display_number(rest)?;
        return Some(unix_display(number));
    }
    // General form `[host]:number[.screen]`. Splitting on the LAST colon
    // keeps explicit socket paths (`/tmp/launchd…/org.xquartz:0`) intact.
    let (host, number) = text.rsplit_once(':')?;
    let number = parse_display_number(number)?;
    if host.is_empty() {
        return Some(unix_display(number));
    }
    // Explicit unix socket path in host position (macOS launchd DISPLAY).
    if host.starts_with('/') {
        return Some(DisplayTarget {
            number,
            server: DisplayServer::UnixSocket(PathBuf::from(host)),
        });
    }
    let port = u16::try_from(6000u32.checked_add(number)?).ok()?;
    Some(DisplayTarget {
        number,
        server: DisplayServer::TcpLoopback(port),
    })
}

fn unix_display(number: u32) -> DisplayTarget {
    DisplayTarget {
        number,
        server: DisplayServer::UnixSocket(PathBuf::from(format!("/tmp/.X11-unix/X{number}"))),
    }
}

/// `10` or `10.0` -> 10. Digits only, bounded so `6000 + n` stays a u16.
fn parse_display_number(text: &str) -> Option<u32> {
    let (number, screen) = match text.split_once('.') {
        Some((number, screen)) => (number, screen),
        None => (text, ""),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(number) || !(screen.is_empty() || digits(screen)) {
        return None;
    }
    let number: u32 = number.parse().ok()?;
    (number <= MAX_DISPLAY_NUMBER).then_some(number)
}

/// One-shot fake cookie generated per armed session: it is sent to the SSH
/// server inside `x11-req` (and configured via xauth on the remote side),
/// and every incoming x11 channel's first packet must carry it before
/// anything is relayed to the local display. The real `.Xauthority` cookie
/// never leaves the machine (spike §3.3).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FakeCookie {
    bytes: [u8; COOKIE_LEN],
    hex: String,
}

impl std::fmt::Debug for FakeCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The cookie is a bearer secret: never print its value.
        f.debug_struct("FakeCookie").finish_non_exhaustive()
    }
}

impl FakeCookie {
    /// Draws a fresh cookie from the OS CSPRNG.
    pub(crate) fn generate() -> Result<Self, String> {
        let mut bytes = [0u8; COOKIE_LEN];
        getrandom::fill(&mut bytes).map_err(|error| format!("OS CSPRNG failure: {error}"))?;
        let mut hex = String::with_capacity(COOKIE_LEN * 2);
        for byte in bytes {
            hex.push_str(&format!("{byte:02x}"));
        }
        Ok(Self { bytes, hex })
    }

    /// The value to send in `x11-req` (lowercase hex, as xauth prints it).
    pub(crate) fn hex(&self) -> &str {
        &self.hex
    }

    fn bytes(&self) -> &[u8; COOKIE_LEN] {
        &self.bytes
    }
}

/// Verdict for the first bytes a remote X application writes into an x11
/// channel (the X connection setup block). Mirrors OpenSSH's client-side
/// check: a 12-byte fixed header, then the authorization protocol name and
/// data, each padded to a 4-byte boundary; byte order comes from the first
/// byte (`0x6c` LSB / `0x42` MSB).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupInspection {
    /// Not enough bytes yet; buffer and read again.
    Incomplete,
    /// Authorized: rewrite `COOKIE_LEN` bytes at `cookie_offset` with the
    /// local display's real cookie (OpenSSH substitutes fake -> real here)
    /// and relay the whole buffer as-is.
    Accept { cookie_offset: usize },
    /// Malformed or carrying the wrong authorization data: the channel must
    /// be dropped, never relayed.
    Reject(&'static str),
}

/// Inspects the first setup bytes against the cookie issued for the session.
pub(crate) fn inspect_setup(buffer: &[u8], expected: &FakeCookie) -> SetupInspection {
    if buffer.len() < 12 {
        return SetupInspection::Incomplete;
    }
    let least_significant = buffer[0] == 0x6c;
    if !least_significant && buffer[0] != 0x42 {
        return SetupInspection::Reject("X setup has a bad byte-order byte");
    }
    let u16_at = |offset: usize| -> u16 {
        if least_significant {
            u16::from_le_bytes([buffer[offset], buffer[offset + 1]])
        } else {
            u16::from_be_bytes([buffer[offset], buffer[offset + 1]])
        }
    };
    let protocol_len = u16_at(6) as usize;
    let data_len = u16_at(8) as usize;
    let cookie_offset = 12 + pad4(protocol_len);
    let total_len = cookie_offset + pad4(data_len);
    if buffer.len() < total_len {
        return SetupInspection::Incomplete;
    }
    if protocol_len != X11_AUTH_PROTOCOL.len()
        || &buffer[12..12 + protocol_len] != X11_AUTH_PROTOCOL.as_bytes()
    {
        return SetupInspection::Reject("X setup uses a different authorization protocol");
    }
    if data_len != COOKIE_LEN {
        return SetupInspection::Reject("X setup carries an unexpected cookie length");
    }
    if buffer[cookie_offset..cookie_offset + COOKIE_LEN] != *expected.bytes() {
        return SetupInspection::Reject("X setup cookie does not match the issued fake cookie");
    }
    SetupInspection::Accept { cookie_offset }
}

/// X protocol fields are padded to 4-byte boundaries.
fn pad4(len: usize) -> usize {
    (len + 3) & !3
}

/// Family codes from Xau(3).
const FAMILY_LOCAL: u16 = 256;
const FAMILY_WILD: u16 = 65535;

/// One decoded `.Xauthority` record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct XauthRecord {
    family: u16,
    number: String,
    name: Vec<u8>,
    data: Vec<u8>,
}

/// Decodes the binary xauth store (big-endian `u16` length-prefixed fields:
/// family, address, display number, name, data). A truncated trailing record
/// is dropped instead of failing the whole file — a torn store must degrade
/// to "no real cookie", never to a parse error.
fn parse_xauthority(bytes: &[u8]) -> Vec<XauthRecord> {
    let mut reader = XauthReader {
        bytes,
        cursor: 0usize,
    };
    let mut records = Vec::new();
    while let Some(family) = reader.u16() {
        // Each read borrows the reader, so own the bytes immediately to let
        // the next `bytes()` call take `&mut` again.
        let Some(_address) = reader.bytes().map(<[u8]>::to_vec) else {
            break;
        };
        let Some(number) = reader.bytes().map(<[u8]>::to_vec) else {
            break;
        };
        let Some(name) = reader.bytes().map(<[u8]>::to_vec) else {
            break;
        };
        let Some(data) = reader.bytes().map(<[u8]>::to_vec) else {
            break;
        };
        records.push(XauthRecord {
            family,
            number: String::from_utf8_lossy(&number).into_owned(),
            name: name.to_vec(),
            data: data.to_vec(),
        });
    }
    records
}

/// Length-prefixed field reader over the xauth binary format.
struct XauthReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl XauthReader<'_> {
    fn u16(&mut self) -> Option<u16> {
        let slice = self.bytes.get(self.cursor..self.cursor + 2)?;
        self.cursor += 2;
        Some(u16::from_be_bytes([slice[0], slice[1]]))
    }

    fn bytes(&mut self) -> Option<&[u8]> {
        let length = self.u16()? as usize;
        let slice = self.bytes.get(self.cursor..self.cursor + length)?;
        self.cursor += length;
        Some(slice)
    }
}

/// Picks the real `MIT-MAGIC-COOKIE-1` for the display: family LOCAL or WILD
/// with the same display number. The address field is ignored on purpose —
/// local records differ across platforms and display managers (empty vs.
/// hostname), and xauth's own CLI lookup keys on family + number too.
fn find_real_cookie(records: &[XauthRecord], display_number: u32) -> Option<Vec<u8>> {
    let number = display_number.to_string();
    records
        .iter()
        .filter(|record| {
            matches!(record.family, FAMILY_LOCAL | FAMILY_WILD) && record.number == number
        })
        .find(|record| record.name == X11_AUTH_PROTOCOL.as_bytes())
        .map(|record| record.data.clone())
}

/// `$XAUTHORITY` or `$HOME/.Xauthority`; `None` when neither resolves.
pub(crate) fn xauthority_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("XAUTHORITY") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    let home = std::env::var("HOME").ok()?;
    let home = home.trim();
    if home.is_empty() {
        return None;
    }
    Some(Path::new(home).join(".Xauthority"))
}

/// Looks up the real cookie for the display; `None` means "not found" and
/// the bridge falls back to relaying the fake cookie (which only works when
/// the local X server runs with access control disabled — the same fallback
/// OpenSSH takes on its "no xauth data" path).
pub(crate) fn load_real_cookie(display: &DisplayTarget) -> Option<Vec<u8>> {
    let bytes = std::fs::read(xauthority_path()?).ok()?;
    find_real_cookie(&parse_xauthority(&bytes), display.number)
}

/// Concurrent x11 channels bridged per armed session. Eight is generous for
/// interactive use and bounds the local X connection fan-out of one remote
/// host (spike §3.6).
pub(crate) const MAX_BRIDGES_PER_SESSION: u32 = 8;

/// Per-SSH-connection admission gate shared between the session opener (arms
/// it with the fake cookie when the preference and read-only checks pass)
/// and the `server_channel_open_x11` handler. A gate that was never armed
/// rejects every channel — russh's default handler would accept, so this is
/// the fail-closed boundary (spike §3.4).
#[derive(Default)]
pub(crate) struct X11Gate {
    armed: AtomicBool,
    cookie: Mutex<Option<FakeCookie>>,
    bridges: AtomicU32,
}

/// Slot for one bridged x11 channel; dropping it releases the cap slot.
pub(crate) struct BridgePermit {
    gate: Arc<X11Gate>,
}

impl Drop for BridgePermit {
    fn drop(&mut self) {
        self.gate.bridges.fetch_sub(1, Ordering::AcqRel);
    }
}

impl X11Gate {
    /// Draws a fresh cookie and arms the gate; returns the gate plus the hex
    /// value to send in `x11-req`.
    pub(crate) fn armed() -> Result<(Arc<Self>, String), String> {
        let cookie = FakeCookie::generate()?;
        let hex = cookie.hex().to_string();
        let gate = Arc::new(Self {
            armed: AtomicBool::new(true),
            cookie: Mutex::new(Some(cookie)),
            bridges: AtomicU32::new(0),
        });
        Ok((gate, hex))
    }

    /// Clears the cookie on session teardown: channels that race the close
    /// fail closed at the next admission or first-packet check.
    pub(crate) fn disarm(&self) {
        self.armed.store(false, Ordering::Release);
        if let Ok(mut slot) = self.cookie.lock() {
            *slot = None;
        }
    }

    /// Handler-side admission. `None` when X11 was never armed on this
    /// session, the session already closed, or the per-session bridge cap is
    /// exhausted — the caller must reject the channel.
    pub(crate) fn try_admit(self: &Arc<Self>) -> Option<(FakeCookie, BridgePermit)> {
        if !self.armed.load(Ordering::Acquire) {
            return None;
        }
        // Bump first and roll back on every failure path so concurrent
        // admissions keep the cap exact.
        if self.bridges.fetch_add(1, Ordering::AcqRel) >= MAX_BRIDGES_PER_SESSION {
            self.bridges.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        let cookie = match self.cookie.lock() {
            Ok(slot) => match slot.as_ref() {
                Some(cookie) => cookie.clone(),
                None => {
                    self.bridges.fetch_sub(1, Ordering::AcqRel);
                    return None;
                }
            },
            Err(_) => {
                self.bridges.fetch_sub(1, Ordering::AcqRel);
                return None;
            }
        };
        Some((cookie, BridgePermit { gate: self.clone() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_unix(display: &str, number: u32, path: &str) {
        let parsed = parse_display(display).expect("unix display must parse");
        assert_eq!(parsed.number, number, "{display}");
        assert_eq!(
            parsed.server,
            DisplayServer::UnixSocket(PathBuf::from(path)),
            "{display}"
        );
    }

    fn assert_tcp(display: &str, number: u32, port: u16) {
        let parsed = parse_display(display).expect("tcp display must parse");
        assert_eq!(parsed.number, number, "{display}");
        assert_eq!(parsed.server, DisplayServer::TcpLoopback(port), "{display}");
    }

    #[test]
    fn parse_display_matches_x7_forms() {
        assert_unix(":0", 0, "/tmp/.X11-unix/X0");
        assert_unix(":10.0", 10, "/tmp/.X11-unix/X10");
        assert_unix("unix:3", 3, "/tmp/.X11-unix/X3");
        assert_unix("unix:", 0, "/tmp/.X11-unix/X0");
        assert_unix(
            "unix:/private/tmp/com.apple.launchd.abc/org.xquartz",
            0,
            "/private/tmp/com.apple.launchd.abc/org.xquartz",
        );
        assert_unix("unix:/tmp/x:1", 1, "/tmp/x");
        // macOS launchd hands out DISPLAY values like
        // /private/tmp/com.apple.launchd.*/org.xquartz:0
        assert_unix(
            "/private/tmp/com.apple.launchd.abc/org.xquartz:0",
            0,
            "/private/tmp/com.apple.launchd.abc/org.xquartz",
        );
        assert_tcp("localhost:0", 0, 6000);
        assert_tcp("remote.example:2.1", 2, 6002);
        // Surrounding whitespace is trimmed before parsing: a trimmed `:7`
        // has no host part, so it is the unix form (exercises the trim path).
        assert_unix("  :7  ", 7, "/tmp/.X11-unix/X7");
    }

    #[test]
    fn parse_display_rejects_unparsable_values() {
        assert_eq!(parse_display(""), None);
        assert_eq!(parse_display("   "), None);
        assert_eq!(parse_display(":"), None);
        assert_eq!(parse_display("host:"), None);
        assert_eq!(parse_display(":abc"), None);
        assert_eq!(parse_display(":1.2.3"), None);
        assert_eq!(parse_display(":-1"), None);
        assert_eq!(parse_display(":0x10"), None);
        assert_eq!(parse_display("unix:abc"), None);
        assert_eq!(parse_display("unix:/tmp/x:abc"), None);
        // 6000 + n would not fit a u16 anymore.
        assert_eq!(parse_display(":59536"), None);
        assert_eq!(parse_display("host:99999999999"), None);
    }

    #[test]
    fn cookie_generation_is_unique_lowercase_hex() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let cookie = FakeCookie::generate().expect("CSPRNG");
            let hex = cookie.hex();
            assert_eq!(hex.len(), 32);
            assert!(hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
            assert!(seen.insert(hex.to_string()), "cookie repeated");
        }
    }

    /// Builds an X connection setup block the way Xlib writes it.
    fn setup_packet(byte_order: u8, lsb: bool, protocol: &[u8], cookie: &[u8]) -> Vec<u8> {
        let put = |value: u16| -> Vec<u8> {
            if lsb {
                value.to_le_bytes().to_vec()
            } else {
                value.to_be_bytes().to_vec()
            }
        };
        let mut out = vec![byte_order, 0, 11, 0, 0, 0];
        out.extend_from_slice(&put(protocol.len() as u16));
        out.extend_from_slice(&put(cookie.len() as u16));
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(protocol);
        out.resize(out.len() + pad4(protocol.len()) - protocol.len(), 0);
        out.extend_from_slice(cookie);
        out.resize(out.len() + pad4(cookie.len()) - cookie.len(), 0);
        out
    }

    #[test]
    fn inspect_setup_accepts_matching_cookie_on_both_byte_orders() {
        let cookie = FakeCookie::generate().expect("CSPRNG");
        let expected_offset = 12 + pad4(X11_AUTH_PROTOCOL.len());
        for (byte_order, lsb) in [(0x6c_u8, true), (0x42, false)] {
            let packet = setup_packet(
                byte_order,
                lsb,
                X11_AUTH_PROTOCOL.as_bytes(),
                cookie.bytes(),
            );
            assert_eq!(
                inspect_setup(&packet, &cookie),
                SetupInspection::Accept {
                    cookie_offset: expected_offset
                },
                "byte order {byte_order:#x}"
            );
            // Trailing bytes (already-relayed X traffic) are fine: only the
            // setup block itself is inspected.
            let mut extended = packet.clone();
            extended.extend_from_slice(&[1, 2, 3, 4]);
            assert_eq!(
                inspect_setup(&extended, &cookie),
                SetupInspection::Accept {
                    cookie_offset: expected_offset
                },
                "byte order {byte_order:#x}"
            );
        }
    }

    #[test]
    fn inspect_setup_needs_the_whole_setup_block() {
        let cookie = FakeCookie::generate().expect("CSPRNG");
        assert_eq!(inspect_setup(&[], &cookie), SetupInspection::Incomplete);
        let packet = setup_packet(0x6c, true, X11_AUTH_PROTOCOL.as_bytes(), cookie.bytes());
        assert_eq!(
            inspect_setup(&packet[..7], &cookie),
            SetupInspection::Incomplete
        );
        assert_eq!(
            inspect_setup(&packet[..packet.len() - 1], &cookie),
            SetupInspection::Incomplete
        );
    }

    #[test]
    fn inspect_setup_rejects_malformed_or_foreign_setups() {
        let cookie = FakeCookie::generate().expect("CSPRNG");
        assert_eq!(
            inspect_setup(&[0x00, 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0], &cookie),
            SetupInspection::Reject("X setup has a bad byte-order byte")
        );
        let packet = setup_packet(0x6c, true, b"OTHER-AUTH", cookie.bytes());
        assert_eq!(
            inspect_setup(&packet, &cookie),
            SetupInspection::Reject("X setup uses a different authorization protocol")
        );
        let wrong_cookie =
            setup_packet(0x6c, true, X11_AUTH_PROTOCOL.as_bytes(), &[0u8; COOKIE_LEN]);
        assert_eq!(
            inspect_setup(&wrong_cookie, &cookie),
            SetupInspection::Reject("X setup cookie does not match the issued fake cookie")
        );
        let short_cookie = setup_packet(0x42, false, X11_AUTH_PROTOCOL.as_bytes(), &[1u8; 8]);
        assert_eq!(
            inspect_setup(&short_cookie, &cookie),
            SetupInspection::Reject("X setup carries an unexpected cookie length")
        );
    }

    fn xauth_record(
        family: u16,
        address: &[u8],
        number: &str,
        name: &[u8],
        data: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&family.to_be_bytes());
        for field in [address, number.as_bytes(), name, data] {
            out.extend_from_slice(&(field.len() as u16).to_be_bytes());
            out.extend_from_slice(field);
        }
        out
    }

    #[test]
    fn xauthority_lookup_matches_family_and_number() {
        let data = [7u8; COOKIE_LEN];
        let mut store = Vec::new();
        // Local display record (family 256, hostname-style address).
        store.extend_from_slice(&xauth_record(
            FAMILY_LOCAL,
            b"somehost",
            "0",
            X11_AUTH_PROTOCOL.as_bytes(),
            &data,
        ));
        // A foreign display number must not leak its cookie.
        store.extend_from_slice(&xauth_record(
            FAMILY_LOCAL,
            b"somehost",
            "1",
            X11_AUTH_PROTOCOL.as_bytes(),
            &[9u8; COOKIE_LEN],
        ));
        // Wildcard family matches the number too.
        store.extend_from_slice(&xauth_record(
            FAMILY_WILD,
            &[],
            "2",
            X11_AUTH_PROTOCOL.as_bytes(),
            &[5u8; COOKIE_LEN],
        ));
        let records = parse_xauthority(&store);
        assert_eq!(records.len(), 3);
        assert_eq!(find_real_cookie(&records, 0), Some(data.to_vec()));
        assert_eq!(find_real_cookie(&records, 2), Some(vec![5u8; COOKIE_LEN]));
        // A different authorization protocol name never matches.
        let wrong_name = parse_xauthority(&xauth_record(
            FAMILY_LOCAL,
            &[],
            "0",
            b"XDM-AUTHORIZATION-1",
            &data,
        ));
        assert_eq!(find_real_cookie(&wrong_name, 0), None);
        // No record at all.
        assert_eq!(find_real_cookie(&parse_xauthority(&[]), 0), None);
    }

    #[test]
    fn xauthority_parser_survives_truncated_records() {
        let mut store = xauth_record(
            FAMILY_LOCAL,
            &[],
            "0",
            X11_AUTH_PROTOCOL.as_bytes(),
            &[3u8; COOKIE_LEN],
        );
        // Append a record whose data field is cut short.
        let mut torn = xauth_record(
            FAMILY_LOCAL,
            &[],
            "1",
            X11_AUTH_PROTOCOL.as_bytes(),
            &[4u8; 4],
        );
        torn.truncate(torn.len() - 2);
        store.extend_from_slice(&torn);
        let records = parse_xauthority(&store);
        assert_eq!(records.len(), 1);
        assert_eq!(find_real_cookie(&records, 0), Some(vec![3u8; COOKIE_LEN]));
    }

    #[test]
    fn gate_fails_closed_for_unarmed_sessions() {
        // Fresh gate: no session ever asked for X11 -> every channel out.
        let gate = Arc::new(X11Gate::default());
        assert!(gate.try_admit().is_none());
        // Disarming a previously armed gate closes it again (session close).
        let (gate, hex) = X11Gate::armed().expect("arm");
        assert_eq!(hex.len(), 32);
        assert!(gate.try_admit().is_some());
        gate.disarm();
        assert!(gate.try_admit().is_none());
    }

    #[test]
    fn gate_hands_the_issued_cookie_to_the_bridge() {
        let (gate, hex) = X11Gate::armed().expect("arm");
        let (cookie, _permit) = gate.try_admit().expect("admit while armed");
        assert_eq!(cookie.hex(), hex);
        // Dropping the permit releases the slot; the gate is reusable.
        drop(_permit);
        assert!(gate.try_admit().is_some());
    }

    #[test]
    fn gate_caps_concurrent_bridges_per_session() {
        let (gate, _hex) = X11Gate::armed().expect("arm");
        let mut permits = Vec::new();
        for _ in 0..MAX_BRIDGES_PER_SESSION {
            permits.push(gate.try_admit().expect("slot within the cap"));
        }
        assert!(gate.try_admit().is_none(), "cap must hold");
        // Releasing one slot admits the next waiting channel.
        drop(permits.pop());
        assert!(gate.try_admit().is_some());
    }

    #[test]
    fn pad4_matches_x_protocol_alignment() {
        assert_eq!(pad4(0), 0);
        assert_eq!(pad4(1), 4);
        assert_eq!(pad4(4), 4);
        assert_eq!(pad4(18), 20);
    }
}

// —— 会话接线（ssh.rs 消费）———————————————————————————————

/// The gate of the most recent X11-enabled session (`armed()` output).
/// `None` until a session turns X11 on; replaced on each re-arm.
static ACTIVE_GATE: std::sync::OnceLock<Arc<X11Gate>> = std::sync::OnceLock::new();

/// Arms a fresh gate for a session that turned X11 on; returns the fake
/// cookie hex to send in `x11-req`.
pub(crate) fn arm_session() -> Result<String, String> {
    let (gate, hex) = X11Gate::armed()?;
    let _ = ACTIVE_GATE.set(gate);
    set_enabled(true);
    Ok(hex)
}

/// Admission for `server_channel_open_x11`: only succeeds while a session
/// has X11 armed and the per-connection bridge cap is not exhausted.
pub(crate) fn try_admit_active() -> Option<(FakeCookie, BridgePermit)> {
    ACTIVE_GATE.get().and_then(|gate| gate.try_admit())
}

/// Fast-path preference flag (mirrors `x11_forwarding` in preferences.json).
/// The Handler consults this before touching the registry; the on-disk file
/// stays the source of truth via [`enabled_from`].
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Reads `x11_forwarding` from the allowlisted preferences file.
/// The display target for the current session: the `DISPLAY` environment
/// variable parsed, falling back to the default unix socket (`:0`).
pub(crate) fn current_display_target() -> DisplayTarget {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    parse_display(&display).unwrap_or_else(|| unix_display(0))
}

pub(crate) fn enabled_from(data_dir: &Path) -> bool {
    let text =
        std::fs::read_to_string(crate::preferences::store_path(data_dir)).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("x11_forwarding")
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(false)
}

/// Bridges an accepted x11 channel into the local X server endpoint
/// (unix socket first, TCP loopback fallback). Byte-for-byte relay: the X
/// client's real-cookie setup packet travels untouched to the local server.
pub(crate) async fn bridge_channel(
    channel: russh::Channel<russh::client::Msg>,
    target: &DisplayTarget,
) -> Result<(), String> {
    let connect_error = |error| format!("X11: cannot reach local display {target:?}: {error}");
    match &target.server {
        DisplayServer::UnixSocket(path) => {
            let local = tokio::net::UnixStream::connect(path)
                .await
                .map_err(|error| connect_error(error))?;
            let mut stream = channel.into_stream();
            let (up, down) = tokio::io::copy_bidirectional(&mut stream, &mut { local })
                .await
                .map_err(|error| format!("X11: bridge error: {error}"))?;
            tracing_bridge_stats(up, down);
            Ok(())
        }
        DisplayServer::TcpLoopback(port) => {
            let mut local = tokio::net::TcpStream::connect(("127.0.0.1", *port))
                .await
                .map_err(|error| connect_error(error))?;
            let mut stream = channel.into_stream();
            let (up, down) = tokio::io::copy_bidirectional(&mut stream, &mut local)
                .await
                .map_err(|error| format!("X11: bridge error: {error}"))?;
            tracing_bridge_stats(up, down);
            Ok(())
        }
    }
}

fn tracing_bridge_stats(up: u64, down: u64) {
    if up + down > 0 {
        eprintln!("[x11] bridged {up}↑/{down}↓ bytes");
    }
}

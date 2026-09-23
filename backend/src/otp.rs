//! OTP 算法中心库（P2-1）：RFC 4226 HOTP / RFC 6238 TOTP 与
//! `otpauth://` URI 的解析/生成。零新增 crate 依赖——HMAC 走既有
//! `hmac`/`sha1`/`sha2`，base32 走既有 `data-encoding`。
//!
//! 取码路径的唯一权威实现：`otp/generate` 协议、连接绑定的 OTP 库条目
//! 自动应答回落（`ssh.rs::sudo_auth_for`）都经由本模块，保证与
//! `triggers.rs` 内联的 tssh 兼容 TOTP（SHA1/6 位/30 s 默认）产出一致。
//! QR 图片解码（`rqrr`/`image` 新依赖）刻意不放在这里——见 `otp_store.rs`。

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Sha256, Sha512};

/// TOTP/HOTP 摘要算法。URI/存储里的大小写不敏感名称见 [`OtpAlgorithm::parse`]，
/// 未知或缺省一律回落 SHA1（RFC 6238 / Google Authenticator 默认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtpAlgorithm {
    Sha1,
    Sha256,
    Sha512,
}

impl OtpAlgorithm {
    /// 解析算法名（"SHA1"/"SHA256"/"SHA512"，大小写不敏感；缺省/未知 → SHA1）。
    pub fn parse(value: &str) -> OtpAlgorithm {
        match value.trim().to_ascii_uppercase().as_str() {
            "SHA256" => OtpAlgorithm::Sha256,
            "SHA512" => OtpAlgorithm::Sha512,
            _ => OtpAlgorithm::Sha1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OtpAlgorithm::Sha1 => "SHA1",
            OtpAlgorithm::Sha256 => "SHA256",
            OtpAlgorithm::Sha512 => "SHA512",
        }
    }
}

/// RFC 4226 动态截断：取摘要末字节低 4 位为偏移，读 4 字节大端并屏蔽符号位，
/// 再对 10^digits 取模。`digits` 由调用方约束在 1..=9（协议解析限 6..=8；
/// 9 以上会溢出 u32，这里防御性夹到 9）。
pub fn hotp(algorithm: OtpAlgorithm, secret: &[u8], counter: u64, digits: u8) -> u32 {
    let digits = digits.clamp(1, 9);
    let message = counter.to_be_bytes();
    let digest: Vec<u8> = match algorithm {
        OtpAlgorithm::Sha1 => {
            let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(secret)
                .expect("HMAC accepts keys of any length");
            mac.update(&message);
            mac.finalize().into_bytes().to_vec()
        }
        OtpAlgorithm::Sha256 => {
            let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret)
                .expect("HMAC accepts keys of any length");
            mac.update(&message);
            mac.finalize().into_bytes().to_vec()
        }
        OtpAlgorithm::Sha512 => {
            let mut mac = <Hmac<Sha512> as KeyInit>::new_from_slice(secret)
                .expect("HMAC accepts keys of any length");
            mac.update(&message);
            mac.finalize().into_bytes().to_vec()
        }
    };
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let binary = ((digest[offset] as u32 & 0x7f) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | digest[offset + 3] as u32;
    binary % 10_u32.pow(u32::from(digits))
}

/// 指定墙钟时刻的 TOTP：返回 `(码, 剩余秒数)`。码左补零到 `digits` 位；
/// 剩余秒数 = `period - unix_secs % period`（RFC 6238 步长语义，
/// counter = unix_secs / period）。`period` 为 0 时按 1 处理（防御；解析层
/// 已拒绝非正周期）。
pub fn totp_at(
    algorithm: OtpAlgorithm,
    secret: &[u8],
    period: u64,
    unix_secs: u64,
    digits: u8,
) -> (String, u64) {
    let period = period.max(1);
    let counter = unix_secs / period;
    let code = hotp(algorithm, secret, counter, digits);
    (
        format!("{code:0width$}", width = digits as usize),
        period - unix_secs % period,
    )
}

/// `otpauth://` URI 解析产物（协议回传给前端预填，不入库）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtpUriParams {
    /// "totp" | "hotp"
    pub otp_type: String,
    /// 发行方（`issuer` 参数优先，其次 label 的 `Issuer:account` 前缀）。
    pub issuer: String,
    /// 解码后的 label（URI 路径段，通常是 `Issuer:account` 或账号）。
    pub label: String,
    /// 规范化（去空格/连字符、大写、补齐填充后重编码）的 base32 密钥。
    pub secret_base32: String,
    pub algorithm: OtpAlgorithm,
    pub digits: u8,
    pub period: u64,
    /// 仅 hotp 有值；totp 忽略该参数。
    pub counter: Option<u64>,
}

/// 解析 `otpauth://totp/…` / `otpauth://hotp/…` URI。要求精确前缀；
/// `secret` 必填且必须是合法 base32（容忍空格/连字符与大小写）；
/// `algorithm` 未知值回落 SHA1；`digits`（默认 6）限 6..=8；
/// `period`（默认 30）限 1..=3600；hotp 缺 `counter` 报错。
pub fn parse_otpauth_uri(uri: &str) -> Result<OtpUriParams, String> {
    let uri = uri.trim();
    let (otp_type, rest) = if let Some(rest) = uri.strip_prefix("otpauth://totp/") {
        ("totp", rest)
    } else if let Some(rest) = uri.strip_prefix("otpauth://hotp/") {
        ("hotp", rest)
    } else {
        return Err("expected an otpauth://totp/ or otpauth://hotp/ URI".to_string());
    };
    let (raw_label, query) = rest.split_once('?').unwrap_or((rest, ""));

    let mut secret_base32 = String::new();
    let mut issuer = String::new();
    let mut digits: u8 = 6;
    let mut period: u64 = 30;
    let mut counter: Option<u64> = None;
    let mut algorithm = OtpAlgorithm::Sha1;
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(value);
        match key {
            "secret" => secret_base32 = normalize_base32(&value)?,
            "issuer" => issuer = value,
            // 未知算法值与既有 exec::parse_otpauth 一致回落 SHA1。
            "algorithm" => algorithm = OtpAlgorithm::parse(&value),
            "digits" => {
                digits = value
                    .parse::<u8>()
                    .ok()
                    .filter(|digits| (6..=8).contains(digits))
                    .ok_or_else(|| format!("otpauth digits must be 6-8, got '{value}'"))?;
            }
            "period" => {
                period = value
                    .parse::<u64>()
                    .ok()
                    .filter(|period| (1..=3600).contains(period))
                    .ok_or_else(|| format!("otpauth period must be 1-3600, got '{value}'"))?;
            }
            "counter" => {
                if !value.is_empty() && value.parse::<u64>().is_err() {
                    return Err(format!("otpauth counter must be an integer, got '{value}'"));
                }
                // totp 忽略 counter；hotp 缺失在下方统一报错。
                counter = (otp_type == "hotp").then(|| value.parse().ok()).flatten();
            }
            _ => {}
        }
    }

    if secret_base32.is_empty() {
        return Err("otpauth URI is missing the secret parameter".to_string());
    }
    let label = percent_decode(raw_label);
    let issuer = if issuer.is_empty() {
        // Google Authenticator 约定：label = "Issuer:account"。
        label
            .split(':')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        issuer
    };
    if otp_type == "hotp" && counter.is_none() {
        return Err("otpauth hotp URI requires a counter parameter".to_string());
    }
    Ok(OtpUriParams {
        otp_type: otp_type.to_string(),
        issuer,
        label,
        secret_base32,
        algorithm,
        digits,
        period,
        counter,
    })
}

/// 由解析产物重建 `otpauth://` URI（`otp/import-qr` 预填回显与解析/生成
/// 往返测试共用）。label 做百分号编码，参数按 Google Authenticator 约定排列。
pub fn to_otpauth_uri(params: &OtpUriParams) -> String {
    let mut uri = format!(
        "otpauth://{}/{}?secret={}&issuer={}&algorithm={}&digits={}&period={}",
        params.otp_type,
        percent_encode(&params.label),
        params.secret_base32,
        percent_encode(&params.issuer),
        params.algorithm.as_str(),
        params.digits,
        params.period,
    );
    if params.otp_type == "hotp" {
        uri.push_str(&format!(
            "&counter={}",
            params
                .counter
                .map(|value| value.to_string())
                .unwrap_or_default()
        ));
    }
    uri
}

/// 容错 base32 解码（对齐 authenticator 与 triggers.rs 接受的形态）：
/// 丢弃空格/连字符、大小写折叠、补齐缺失的 `=` 填充。返回原始 HMAC 密钥字节。
pub fn decode_base32(text: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = text
        .chars()
        .filter(|character| !matches!(character, ' ' | '-'))
        .map(|character| character.to_ascii_uppercase())
        .collect();
    let unpadded = cleaned.trim_end_matches('=');
    if unpadded.is_empty() {
        return Err("base32 secret is empty".to_string());
    }
    let mut padded = unpadded.to_string();
    if !unpadded.len().is_multiple_of(8) {
        padded.push_str(&"=".repeat(8 - (unpadded.len() % 8)));
    }
    data_encoding::BASE32
        .decode(padded.as_bytes())
        .map_err(|_| format!("invalid base32 secret (expected A-Z2-7, got '{text}')"))
}

/// 规范化 base32 密钥文本：合法性与 [`decode_base32`] 一致，返回大写无填充
/// 规范形态（存储与回显统一用这个）。
fn normalize_base32(text: &str) -> Result<String, String> {
    let decoded = decode_base32(text)?;
    Ok(data_encoding::BASE32_NOPAD.encode(&decoded))
}

/// RFC 3986 保留集之外原样保留；等价于 `encodeURIComponent` 的常用子集
/// （otpauth label/issuer 只需要这一档，避免为编码再引 crate）。
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// `%XX` 百分号解码；非法序列原样保留（宽松，避免给粘贴数据报无谓错误）。
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if let Some(hex) = bytes.get(index + 1..index + 3) {
                if let Ok(value) = u8::from_str_radix(std::str::from_utf8(hex).unwrap_or(""), 16) {
                    decoded.push(value);
                    index += 3;
                    continue;
                }
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4226 附录 D：SHA1、ASCII 密钥 "12345678901234567890"、6 位。
    const RFC4226_SECRET: &[u8] = b"12345678901234567890";
    const RFC4226_CODES_6: [u32; 10] = [
        755224, 287082, 359152, 969429, 338314, 254676, 287922, 162583, 399871, 520489,
    ];
    /// RFC 6238 附录 B：8 位、period 30，`(时刻, SHA1, SHA256, SHA512)`。
    const RFC6238_VECTORS: [(u64, &str, &str, &str); 6] = [
        (59, "94287082", "46119246", "90693936"),
        (1111111109, "07081804", "68084774", "25091201"),
        (1111111111, "14050471", "67062674", "99943326"),
        (1234567890, "89005924", "91819424", "93441116"),
        (2000000000, "69279037", "90698825", "38618901"),
        (20000000000, "65353130", "77737706", "47863826"),
    ];

    // —— HOTP（RFC 4226 附录 D 官方向量）———————————————

    #[test]
    fn hotp_matches_rfc4226_appendix_d() {
        for (counter, expected) in RFC4226_CODES_6.iter().enumerate() {
            assert_eq!(
                hotp(OtpAlgorithm::Sha1, RFC4226_SECRET, counter as u64, 6),
                *expected,
                "counter {counter}"
            );
        }
    }

    #[test]
    fn hotp_dynamic_truncation_is_counter_sensitive_and_digit_scaled() {
        // 相邻 counter 不碰撞（附录 D 十个向量互不相同已覆盖）；这里补
        // digits 位数语义：8 位 = 前 8 位十进制（补零语义在 totp_at 验证）。
        assert_eq!(hotp(OtpAlgorithm::Sha1, RFC4226_SECRET, 0, 8), 84_755_224);
    }

    // —— TOTP（RFC 6238 SHA1/SHA256/SHA512 官方向量）———————

    #[test]
    fn totp_at_matches_rfc6238_vectors_for_all_algorithms() {
        let secrets = [
            (OtpAlgorithm::Sha1, &b"12345678901234567890"[..]),
            (
                OtpAlgorithm::Sha256,
                &b"12345678901234567890123456789012"[..],
            ),
            (
                OtpAlgorithm::Sha512,
                &b"1234567890123456789012345678901234567890123456789012345678901234"[..],
            ),
        ];
        for (algorithm, secret) in secrets {
            for (unix_secs, sha1, sha256, sha512) in RFC6238_VECTORS {
                let expected = match algorithm {
                    OtpAlgorithm::Sha1 => sha1,
                    OtpAlgorithm::Sha256 => sha256,
                    OtpAlgorithm::Sha512 => sha512,
                };
                let (code, remaining) = totp_at(algorithm, secret, 30, unix_secs, 8);
                assert_eq!(code, expected, "{algorithm:?} at {unix_secs}");
                assert_eq!(remaining, 30 - unix_secs % 30);
            }
        }
    }

    #[test]
    fn totp_at_pads_left_and_reports_remaining_seconds() {
        // 剩余秒数 = period - unix_secs % period；码左补零到 digits 位。
        let (code, remaining) = totp_at(OtpAlgorithm::Sha1, RFC4226_SECRET, 30, 60, 6);
        assert_eq!(remaining, 30);
        assert_eq!(code.len(), 6);
        // 人为夹一个会前导补零的组合（counter 0、5 位裁切不必真实——只验
        // 形态）：digits=8 时长度恒为 8。
        let (code, _) = totp_at(OtpAlgorithm::Sha1, RFC4226_SECRET, 30, 59, 8);
        assert_eq!(code, "94287082");
    }

    #[test]
    fn totp_at_zero_period_degrades_to_one_second_step() {
        // 解析层拒绝 period 0；这里验证不 panic（防御性 max(1)：步长 1s，
        // 任何时刻剩余都是 1）。
        let (code, remaining) = totp_at(OtpAlgorithm::Sha1, RFC4226_SECRET, 0, 7, 6);
        assert_eq!(remaining, 1);
        assert_eq!(code.len(), 6);
    }

    // —— otpauth URI 解析 / 生成往返 ————————————————————

    #[test]
    fn parse_totp_uri_with_all_parameters() {
        let params = parse_otpauth_uri(
            "otpauth://totp/ACME%3Aalice%40acme.com?secret=gezd%20gnbvgy3tqojq&issuer=ACME&algorithm=SHA256&digits=8&period=60",
        )
        .unwrap();
        assert_eq!(params.otp_type, "totp");
        assert_eq!(params.issuer, "ACME");
        assert_eq!(params.label, "ACME:alice@acme.com");
        assert_eq!(params.secret_base32, "GEZDGNBVGY3TQOJQ");
        assert_eq!(params.algorithm, OtpAlgorithm::Sha256);
        assert_eq!(params.digits, 8);
        assert_eq!(params.period, 60);
        assert_eq!(params.counter, None);
    }

    #[test]
    fn parse_issuer_falls_back_to_label_prefix() {
        let params =
            parse_otpauth_uri("otpauth://totp/Example:alice@example.com?secret=JBSWY3DPEHPK3PXP")
                .unwrap();
        assert_eq!(params.issuer, "Example");
        assert_eq!(params.algorithm, OtpAlgorithm::Sha1, "缺省 SHA1");
        assert_eq!(params.digits, 6);
        assert_eq!(params.period, 30);
    }

    #[test]
    fn parse_hotp_uri_requires_counter() {
        let error = parse_otpauth_uri("otpauth://hotp/counter?secret=JBSWY3DPEHPK3PXP")
            .expect_err("hotp without counter must be rejected");
        assert!(error.contains("counter"), "{error}");
        let params =
            parse_otpauth_uri("otpauth://hotp/counter?secret=JBSWY3DPEHPK3PXP&counter=42").unwrap();
        assert_eq!(params.counter, Some(42));
        assert_eq!(params.otp_type, "hotp");
    }

    #[test]
    fn parse_uri_rejects_wrong_prefix_or_missing_secret() {
        for bad in [
            "https://example.com",
            "otpauth://move/label?secret=JBSWY3DPEHPK3PXP",
            "otpauth://totp/",
            "otpauth://totp/label",
            "otpauth://totp/label?digits=5",
            "otpauth://totp/label?period=0",
            "otpauth://totp/label?counter=x&",
        ] {
            // digits/period/counter 非法都要报错；前缀/secret 缺失也要报错。
            assert!(parse_otpauth_uri(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn parse_uri_tolerates_base32_spaces_and_case() {
        let params = parse_otpauth_uri("otpauth://totp/x?secret=jbsw y3dp ehpk 3pxp").unwrap();
        assert_eq!(params.secret_base32, "JBSWY3DPEHPK3PXP");
    }

    #[test]
    fn uri_round_trip_preserves_parameters() {
        let params = OtpUriParams {
            otp_type: "totp".to_string(),
            issuer: "ACME Corp".to_string(),
            label: "ACME Corp:alice@example.com".to_string(),
            secret_base32: "JBSWY3DPEHPK3PXP".to_string(),
            algorithm: OtpAlgorithm::Sha512,
            digits: 8,
            period: 45,
            counter: None,
        };
        let rebuilt = parse_otpauth_uri(&to_otpauth_uri(&params)).unwrap();
        assert_eq!(rebuilt, params);

        let hotp = OtpUriParams {
            otp_type: "hotp".to_string(),
            counter: Some(7),
            ..params
        };
        let rebuilt = parse_otpauth_uri(&to_otpauth_uri(&hotp)).unwrap();
        assert_eq!(rebuilt, hotp);
    }

    #[test]
    fn algorithm_parse_defaults_to_sha1() {
        assert_eq!(OtpAlgorithm::parse("sha1"), OtpAlgorithm::Sha1);
        assert_eq!(OtpAlgorithm::parse("Sha256 "), OtpAlgorithm::Sha256);
        assert_eq!(OtpAlgorithm::parse("SHA512"), OtpAlgorithm::Sha512);
        assert_eq!(OtpAlgorithm::parse(""), OtpAlgorithm::Sha1);
        assert_eq!(OtpAlgorithm::parse("md5"), OtpAlgorithm::Sha1);
        assert_eq!(OtpAlgorithm::Sha1.as_str(), "SHA1");
        assert_eq!(OtpAlgorithm::Sha256.as_str(), "SHA256");
        assert_eq!(OtpAlgorithm::Sha512.as_str(), "SHA512");
    }

    #[test]
    fn base32_decode_is_tolerant_and_validating() {
        // "GEZDGNBVGY3TQOJQ" 是 ASCII "1234567890" 的 base32（RFC 6238 测试密钥
        // 前 10 字节），大小写与空格/连字符形态都应解出同一结果。
        assert_eq!(decode_base32("GEZDGNBVGY3TQOJQ").unwrap(), b"1234567890");
        assert_eq!(decode_base32("gezd gnbv-gy3t qojq").unwrap(), b"1234567890");
        assert!(decode_base32("").is_err());
        assert!(decode_base32("!!").is_err());
    }

    #[test]
    fn percent_codec_round_trips_ordinary_text() {
        let original = "ACME Corp:alice@example.com/path+x";
        assert_eq!(percent_decode(&percent_encode(original)), original);
        // 宽松解码：非法序列原样保留。
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%2Fb"), "a/b");
    }
}

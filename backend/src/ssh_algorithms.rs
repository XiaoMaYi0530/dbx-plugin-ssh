use std::borrow::Cow;

use russh::{cipher, kex, mac, Preferred};

/// Controls how much legacy SSH compatibility is enabled for a connection.
///
/// `Compatible` is the default so existing connections can reach servers
/// that only provide the SHA-1 MACs removed from russh's defaults.  It keeps
/// russh's secure KEX/cipher defaults unchanged and only appends the two
/// SHA-1 MAC variants.  `Legacy` is an explicit opt-in for older servers that
/// also require SHA-1 Diffie-Hellman or CBC ciphers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SshAlgorithmPolicy {
    Secure,
    #[default]
    Compatible,
    Legacy,
}

impl SshAlgorithmPolicy {
    /// Parses the connection-form value.  Missing/empty values preserve the
    /// default compatibility behavior; an unknown explicit value fails closed
    /// to the secure profile instead of silently enabling legacy algorithms.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            None | Some("") | Some("compatible") => Self::Compatible,
            Some("secure") => Self::Secure,
            Some("legacy") => Self::Legacy,
            Some(_) => Self::Secure,
        }
    }
}

/// Returns russh's default preferences, optionally extended with algorithms
/// needed by older SSH servers.  Secure algorithms always remain first, so a
/// server supporting both modern and legacy algorithms does not cause a
/// needless downgrade merely because compatibility is enabled.
pub fn preferred(policy: SshAlgorithmPolicy) -> Preferred {
    let mut preferred = Preferred::default();
    if policy == SshAlgorithmPolicy::Secure {
        return preferred;
    }

    preferred.mac = append_unique(
        preferred.mac.as_ref(),
        &[mac::HMAC_SHA1_ETM, mac::HMAC_SHA1],
    );
    if policy == SshAlgorithmPolicy::Legacy {
        preferred.kex = append_unique(
            preferred.kex.as_ref(),
            &[kex::DH_G14_SHA1, kex::DH_GEX_SHA1, kex::DH_G1_SHA1],
        );
        preferred.cipher = append_unique(
            preferred.cipher.as_ref(),
            &[
                cipher::AES_256_CBC,
                cipher::AES_192_CBC,
                cipher::AES_128_CBC,
            ],
        );
    }
    preferred
}

fn append_unique<T: Copy + PartialEq>(base: &[T], additions: &[T]) -> Cow<'static, [T]> {
    let mut values = base.to_vec();
    for &addition in additions {
        if !values.contains(&addition) {
            values.push(addition);
        }
    }
    Cow::Owned(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names<T: AsRef<str>>(values: &[T]) -> Vec<&str> {
        values.iter().map(|value| value.as_ref()).collect()
    }

    #[test]
    fn missing_policy_defaults_to_compatible() {
        assert_eq!(
            SshAlgorithmPolicy::parse(None),
            SshAlgorithmPolicy::Compatible
        );
        assert_eq!(
            SshAlgorithmPolicy::parse(Some("")),
            SshAlgorithmPolicy::Compatible
        );
        assert_eq!(
            SshAlgorithmPolicy::parse(Some("compatible")),
            SshAlgorithmPolicy::Compatible
        );
    }

    #[test]
    fn unknown_policy_fails_closed_to_secure() {
        assert_eq!(
            SshAlgorithmPolicy::parse(Some("typo")),
            SshAlgorithmPolicy::Secure
        );
    }

    #[test]
    fn secure_profile_matches_russh_default() {
        let actual = preferred(SshAlgorithmPolicy::Secure);
        let expected = Preferred::default();
        assert_eq!(names(actual.mac.as_ref()), names(expected.mac.as_ref()));
        assert_eq!(names(actual.kex.as_ref()), names(expected.kex.as_ref()));
        assert_eq!(
            names(actual.cipher.as_ref()),
            names(expected.cipher.as_ref())
        );
        assert!(!actual.mac.as_ref().contains(&mac::HMAC_SHA1));
        assert!(!actual.mac.as_ref().contains(&mac::HMAC_SHA1_ETM));
        assert!(!actual.kex.as_ref().contains(&kex::DH_G14_SHA1));
        assert!(!actual.cipher.as_ref().contains(&cipher::AES_128_CBC));
    }

    #[test]
    fn compatible_appends_sha1_mac_without_weakening_kex_or_cipher() {
        let actual = preferred(SshAlgorithmPolicy::Compatible);
        let defaults = Preferred::default();
        assert_eq!(
            &actual.mac.as_ref()[..defaults.mac.len()],
            defaults.mac.as_ref()
        );
        assert_eq!(actual.mac.as_ref().last(), Some(&mac::HMAC_SHA1));
        assert!(actual.mac.as_ref().contains(&mac::HMAC_SHA1_ETM));
        assert_eq!(actual.kex.as_ref(), defaults.kex.as_ref());
        assert_eq!(actual.cipher.as_ref(), defaults.cipher.as_ref());
    }

    #[test]
    fn legacy_appends_only_known_old_kex_and_ciphers() {
        let actual = preferred(SshAlgorithmPolicy::Legacy);
        assert!(actual.kex.as_ref().contains(&kex::DH_G14_SHA1));
        assert!(actual.kex.as_ref().contains(&kex::DH_GEX_SHA1));
        assert!(actual.kex.as_ref().contains(&kex::DH_G1_SHA1));
        assert!(actual.cipher.as_ref().contains(&cipher::AES_128_CBC));
        assert!(actual.cipher.as_ref().contains(&cipher::AES_256_CBC));
        assert!(!actual
            .cipher
            .as_ref()
            .iter()
            .any(|name| name.as_ref() == "3des-cbc"));
    }
}

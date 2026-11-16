//! Time-based one-time codes (RFC 6238, SHA-1, 30 s steps), the same codes an
//! authenticator app shows. Used only for automatic broker sign-in.

use hmac::{Hmac, Mac};
use sha1::Sha1;

const WINDOW_MS: u64 = 30_000;

/// The `secret=` part of an `otpauth://` link, or the input unchanged. People
/// paste the whole link from their authenticator setup as often as the key.
fn extract_secret(s: &str) -> String {
    let raw = s.trim();
    if !raw.to_ascii_lowercase().starts_with("otpauth://") {
        return raw.to_string();
    }
    raw.split_once('?')
        .and_then(|(_, q)| q.split('&').find_map(|p| p.strip_prefix("secret=").map(str::to_string)))
        .unwrap_or_else(|| raw.to_string())
}

/// Decodes a base32 secret as authenticator setups print it: upper or lower
/// case, with or without spaces, dashes and `=` padding, or inside an
/// `otpauth://` link.
pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in extract_secret(s).chars() {
        if c == ' ' || c == '-' || c == '=' {
            continue;
        }
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u32 - 'A' as u32,
            c @ '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Checks a secret before it is used, with a message that says what to paste.
pub fn validate_secret(s: &str) -> Result<Vec<u8>, String> {
    let key = base32_decode(s).ok_or_else(|| {
        "the TOTP secret is not valid base32: paste the long key from the authenticator setup (not the 6-digit code, an API secret or the PIN)".to_string()
    })?;
    if key.len() < 10 {
        return Err("the TOTP secret is too short to be an authenticator key: paste the TOTP secret from the broker, not the API secret or the PIN".into());
    }
    Ok(key)
}

/// The code for `unix_secs`, `digits` long.
pub fn code_at(key: &[u8], unix_secs: u64, digits: u32) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(&(unix_secs / 30).to_be_bytes());
    let h = mac.finalize().into_bytes();
    let o = (h[19] & 0x0f) as usize;
    let n = u32::from_be_bytes([h[o] & 0x7f, h[o + 1], h[o + 2], h[o + 3]]);
    format!("{:0width$}", n % 10u32.pow(digits), width = digits as usize)
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// True when `at_ms` is in the last `edge_ms` of its 30 s window.
pub fn near_window_edge(at_ms: u64, edge_ms: u64) -> bool {
    WINDOW_MS - (at_ms % WINDOW_MS) <= edge_ms
}

/// Milliseconds until the next window opens, plus a 250 ms margin.
pub fn ms_until_next_window(at_ms: u64) -> u64 {
    WINDOW_MS - (at_ms % WINDOW_MS) + 250
}

/// The current 6-digit code for a base32 secret (a plain read, no waiting).
pub fn current_code(secret_b32: &str) -> Option<String> {
    Some(code_at(&base32_decode(secret_b32)?, now_ms() / 1000, 6))
}

/// A 6-digit code that will still be valid when the broker checks it.
///
/// A code minted in the last 3 s of its window has very likely rolled over by
/// the time the broker validates it, and that failure looks exactly like a wrong
/// secret. So this waits for the next window instead. `offset_ms` shifts the
/// clock (to follow a broker's own clock instead of this computer's).
pub async fn safe_code(secret: &str, offset_ms: i64) -> Result<String, String> {
    let key = validate_secret(secret)?;
    let shifted = |now: u64| (now as i64 + offset_ms).max(0) as u64;
    let mut at = shifted(now_ms());
    if near_window_edge(at, 3_000) {
        tokio::time::sleep(std::time::Duration::from_millis(ms_until_next_window(at))).await;
        at = shifted(now_ms());
    }
    Ok(code_at(&key, at / 1000, 6))
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 appendix B: secret "12345678901234567890", 8 digits.
    #[test]
    fn rfc6238_vectors() {
        let k = b"12345678901234567890";
        assert_eq!(code_at(k, 59, 8), "94287082");
        assert_eq!(code_at(k, 1111111109, 8), "07081804");
        assert_eq!(code_at(k, 20000000000, 8), "65353130");
    }

    #[test]
    fn base32_forms() {
        let want = b"12345678901234567890".to_vec();
        assert_eq!(base32_decode("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap(), want);
        assert_eq!(base32_decode("gezd gnbv-gy3t qojq gezd gnbv gy3t qojq").unwrap(), want);
        assert_eq!(
            base32_decode("otpauth://totp/Kite:AB1234?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Kite").unwrap(),
            want
        );
        assert!(base32_decode("not base32!").is_none());
        assert!(base32_decode("").is_none());
    }

    #[test]
    fn short_or_wrong_secrets_are_refused_with_advice() {
        assert!(validate_secret("GEZDGNBVGY3TQOJQ").is_ok());
        assert!(validate_secret("MFRGG").unwrap_err().contains("too short"));
        assert!(validate_secret("123456").unwrap_err().contains("base32"));
    }

    #[test]
    fn window_edge_maths() {
        assert!(near_window_edge(29_000, 3_000));
        assert!(!near_window_edge(12_000, 3_000));
        assert_eq!(ms_until_next_window(29_000), 1_250);
    }
}

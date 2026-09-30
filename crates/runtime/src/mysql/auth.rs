//! The authentication plugins every MySQL server since 5.7 offers.
//!
//! Neither of the two scrambles sends the password: each proves knowledge of it
//! against the 20-byte nonce the server sent in its greeting, so a recorded
//! exchange cannot be replayed against the next connection.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use es_runtime_providers::Entropy;
use sha1::Sha1;
use sha2::{Digest, Sha256};

fn xor(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.iter()
        .enumerate()
        .map(|(i, x)| x ^ b[i % b.len()])
        .collect()
}

/// `mysql_native_password`: `SHA1(pw) XOR SHA1(scramble + SHA1(SHA1(pw)))`.
/// The server stores `SHA1(SHA1(pw))`, so it can check this without ever having
/// held the password. An empty password sends an empty response.
pub(super) fn native_password(password: &str, scramble: &[u8]) -> Vec<u8> {
    if password.is_empty() {
        return Vec::new();
    }
    let stage1 = Sha1::digest(password.as_bytes());
    let stage2 = Sha1::digest(stage1);
    let mut hasher = Sha1::new();
    hasher.update(scramble);
    hasher.update(stage2);
    xor(&stage1, &hasher.finalize())
}

/// `caching_sha2_password`'s fast path:
/// `SHA256(pw) XOR SHA256(SHA256(SHA256(pw)) + scramble)`. Enough when the
/// server has this account's hash cached; when it has not, it asks for the
/// password itself (full authentication).
pub(super) fn caching_sha2(password: &str, scramble: &[u8]) -> Vec<u8> {
    if password.is_empty() {
        return Vec::new();
    }
    let stage1 = Sha256::digest(password.as_bytes());
    let stage2 = Sha256::digest(stage1);
    let mut hasher = Sha256::new();
    hasher.update(stage2);
    hasher.update(scramble);
    xor(&stage1, &hasher.finalize())
}

/// The password as full authentication wants it: NUL-terminated.
pub(super) fn password_bytes(password: &str) -> Vec<u8> {
    let mut out = password.as_bytes().to_vec();
    out.push(0);
    out
}

/// The password for full authentication over a connection that is **not**
/// encrypted: XORed with the scramble and encrypted to the server's RSA key
/// with OAEP (SHA-1), which is what the server decrypts. Over TLS the password
/// goes as it is; the fix for trusting a key the server just sent is TLS.
pub(super) fn encrypt_password(
    entropy: &dyn Entropy,
    password: &str,
    scramble: &[u8],
    public_key_pem: &str,
) -> Result<Vec<u8>, String> {
    let body: String = public_key_pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .flat_map(|line| line.split_whitespace())
        .collect();
    let der = STANDARD
        .decode(body)
        .map_err(|_| "the server's public key is not PEM".to_string())?;
    let plain = xor(&password_bytes(password), scramble);
    crate::rsa_ops::oaep_encrypt(entropy, "SHA-1", &[], &der, &plain)
        .map_err(|_| "encrypting the password to the server's public key failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Vectors computed independently (Python's hashlib), since a server only
    /// ever says yes or no.
    #[test]
    fn scrambles_match_independent_vectors() {
        let scramble: Vec<u8> = (1..=20).collect();
        assert_eq!(
            hex(&native_password("secret", &scramble)),
            "b32bb3a583e1340c0a1108d58b1be49781ad8c2f"
        );
        assert_eq!(
            hex(&caching_sha2("secret", &scramble)),
            "746ebe205d56a0707acb3e796e834e0dd7b1d61743b26bd5202c7a623230c7c9"
        );
        assert!(native_password("", &scramble).is_empty());
        assert!(caching_sha2("", &scramble).is_empty());
        assert_eq!(password_bytes("é"), [0xc3, 0xa9, 0]);
    }
}

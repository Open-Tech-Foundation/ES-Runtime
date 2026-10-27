//! SCRAM (RFC 5802) with SHA-256 (RFC 7677) or SHA-1: the authentication
//! PostgreSQL has defaulted to since version 14, and MongoDB's two password
//! mechanisms. Shared by both drivers.
//!
//! The password arrives already prepared: normalized (NFKC, the part of
//! SASLprep that matters in practice) for SHA-256, and MongoDB's MD5 digest of
//! it for SCRAM-SHA-1, because both are questions about the value and are
//! answered where the value came from.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::digest::KeyInit as _;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest as _, Sha256};

/// The hash a SCRAM exchange is built on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hash {
    Sha1,
    Sha256,
}

/// One exchange, from the client's first message to verifying the server's
/// proof.
pub(crate) struct Scram {
    hash: Hash,
    /// The fewest iterations the server may ask for; a challenge with fewer is
    /// refused rather than answered with a cheaply-attacked proof.
    min_iterations: u32,
    client_nonce: String,
    bare: String,
    /// Set once the server's challenge has been answered: the signature the
    /// server has to produce to prove it knows the password too.
    expected: Option<String>,
}

impl Scram {
    /// Begins an exchange with `nonce` — random in production, fixed in the
    /// tests. PostgreSQL takes the user from the startup packet and ignores the
    /// one here, which is why it passes an empty `username`.
    pub(crate) fn new(hash: Hash, min_iterations: u32, nonce: &[u8], username: &str) -> Scram {
        let client_nonce = STANDARD.encode(nonce);
        // RFC 5802 §5.1: `=` and `,` in a name are escaped.
        let username = username.replace('=', "=3D").replace(',', "=2C");
        let bare = format!("n={username},r={client_nonce}");
        Scram {
            hash,
            min_iterations,
            client_nonce,
            bare,
            expected: None,
        }
    }

    /// The `client-first-message`. `n,,` is the GS2 header: no channel
    /// binding, no authorization identity.
    pub(crate) fn initial(&self) -> String {
        format!("n,,{}", self.bare)
    }

    /// Answers the server's challenge with the `client-final-message`.
    pub(crate) fn respond(&mut self, password: &str, server_first: &str) -> Result<String, String> {
        let nonce = attribute(server_first, "r");
        let salt = attribute(server_first, "s");
        let iterations = attribute(server_first, "i").and_then(|i| i.parse::<u32>().ok());
        let (Some(nonce), Some(salt), Some(iterations)) = (nonce, salt, iterations) else {
            return Err(format!(
                "the server's SCRAM challenge is malformed: {server_first}"
            ));
        };
        if iterations == 0 {
            return Err(format!(
                "the server's SCRAM challenge is malformed: {server_first}"
            ));
        }
        if iterations < self.min_iterations {
            return Err(format!(
                "the server asked for {iterations} SCRAM iterations, fewer than the {} allowed",
                self.min_iterations
            ));
        }
        if !nonce.starts_with(&self.client_nonce) {
            // The server must extend our nonce. One that does not is not the
            // server this exchange started with.
            return Err("the server's SCRAM nonce does not extend the client's".to_string());
        }
        let salt = STANDARD
            .decode(salt)
            .map_err(|_| format!("the server's SCRAM salt is not base64: {salt}"))?;

        // `biws` is base64("n,,"), the GS2 header again, as the channel-binding
        // attribute of a client that offered no binding.
        let without_proof = format!("c=biws,r={nonce}");
        let auth_message = format!("{},{server_first},{without_proof}", self.bare);

        let salted = match self.hash {
            Hash::Sha1 => {
                let mut out = vec![0u8; 20];
                pbkdf2::pbkdf2_hmac::<Sha1>(password.as_bytes(), &salt, iterations, &mut out);
                out
            }
            Hash::Sha256 => {
                let mut out = vec![0u8; 32];
                pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, iterations, &mut out);
                out
            }
        };
        let client_key = self.hmac(&salted, b"Client Key");
        let stored_key = self.digest(&client_key);
        let signature = self.hmac(&stored_key, auth_message.as_bytes());
        let proof: Vec<u8> = client_key
            .iter()
            .zip(signature.iter())
            .map(|(a, b)| a ^ b)
            .collect();

        let server_key = self.hmac(&salted, b"Server Key");
        self.expected = Some(STANDARD.encode(self.hmac(&server_key, auth_message.as_bytes())));
        Ok(format!("{without_proof},p={}", STANDARD.encode(proof)))
    }

    /// Checks the server's proof. Mutual authentication: without it the
    /// client has proved itself and learned nothing about who answered.
    pub(crate) fn verify(&self, server_final: &str) -> Result<(), String> {
        match (&self.expected, attribute(server_final, "v")) {
            (Some(expected), Some(signature)) if expected == signature => Ok(()),
            _ => Err("the server failed to prove it knows the password".to_string()),
        }
    }
}

impl Scram {
    fn hmac(&self, key: &[u8], data: &[u8]) -> Vec<u8> {
        match self.hash {
            Hash::Sha1 => {
                let mut mac =
                    Hmac::<Sha1>::new_from_slice(key).expect("HMAC takes a key of any length");
                mac.update(data);
                mac.finalize().into_bytes().to_vec()
            }
            Hash::Sha256 => {
                let mut mac =
                    Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
                mac.update(data);
                mac.finalize().into_bytes().to_vec()
            }
        }
    }

    fn digest(&self, data: &[u8]) -> Vec<u8> {
        match self.hash {
            Hash::Sha1 => Sha1::digest(data).to_vec(),
            Hash::Sha256 => Sha256::digest(data).to_vec(),
        }
    }
}

/// The value of `name` in a SCRAM message's `a=…,b=…` list.
fn attribute<'a>(message: &'a str, name: &str) -> Option<&'a str> {
    message.split(',').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7677 §3's published exchange, with its fixed nonce and user.
    #[test]
    fn matches_the_rfc_7677_vectors() {
        let nonce = STANDARD.decode("rOprNGfwEbeRWgbNEkqO").unwrap();
        let mut scram = Scram::new(Hash::Sha256, 4096, &nonce, "user");
        assert_eq!(scram.initial(), "n,,n=user,r=rOprNGfwEbeRWgbNEkqO");
        let server_first = "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        let client_final = scram.respond("pencil", server_first).unwrap();
        assert_eq!(
            client_final,
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );
        scram
            .verify("v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=")
            .unwrap();
        assert!(scram.verify("v=AAAA").is_err());
    }

    #[test]
    fn refuses_a_nonce_the_server_did_not_extend() {
        let mut scram = Scram::new(Hash::Sha256, 0, b"abc", "");
        let err = scram
            .respond("pw", "r=someone-else,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096")
            .unwrap_err();
        assert!(err.contains("does not extend"), "{err}");
    }

    #[test]
    fn refuses_a_malformed_challenge() {
        let mut scram = Scram::new(Hash::Sha256, 0, b"abc", "");
        assert!(scram.respond("pw", "r=YWJj,i=0,s=AA==").is_err());
        assert!(scram.respond("pw", "garbage").is_err());
    }

    /// RFC 5802 §5's published SCRAM-SHA-1 exchange.
    #[test]
    fn matches_the_rfc_5802_sha1_vectors() {
        let nonce = STANDARD.decode("fyko+d2lbbFgONRv9qkxdawL").unwrap();
        let mut scram = Scram::new(Hash::Sha1, 4096, &nonce, "user");
        assert_eq!(scram.initial(), "n,,n=user,r=fyko+d2lbbFgONRv9qkxdawL");
        let server_first = "r=fyko+d2lbbFgONRv9qkxdawL3rfcNHYJY1ZVvWVs7j,s=QSXCR+Q6sek8bf92,i=4096";
        let client_final = scram.respond("pencil", server_first).unwrap();
        assert_eq!(
            client_final,
            "c=biws,r=fyko+d2lbbFgONRv9qkxdawL3rfcNHYJY1ZVvWVs7j,p=v0X8v3Bz2T0CJGbJQyF0X+HI4Ts="
        );
        scram.verify("v=rmF9pqV8S7suAoZWja4dJRkFsKQ=").unwrap();
    }

    #[test]
    fn escapes_the_username_and_enforces_the_iteration_floor() {
        let scram = Scram::new(Hash::Sha256, 4096, b"abc", "a=b,c");
        assert_eq!(scram.initial(), "n,,n=a=3Db=2Cc,r=YWJj");
        let mut scram = Scram::new(Hash::Sha256, 4096, b"abc", "u");
        let err = scram
            .respond("pw", "r=YWJjXYZ,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=1024")
            .unwrap_err();
        assert!(err.contains("fewer than"), "{err}");
    }
}

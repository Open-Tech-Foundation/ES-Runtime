//! SCRAM-SHA-256 (RFC 5802, RFC 7677): the authentication PostgreSQL has
//! defaulted to since version 14.
//!
//! The password arrives already normalized (NFKC, the part of SASLprep that
//! matters in practice), because normalizing a string is a question about the
//! value and is answered where the value came from.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::digest::KeyInit as _;
use hmac::{Hmac, Mac};
use sha2::{Digest as _, Sha256};

/// One exchange, from the client's first message to verifying the server's
/// proof.
pub(super) struct Scram {
    client_nonce: String,
    bare: String,
    /// Set once the server's challenge has been answered: the signature the
    /// server has to produce to prove it knows the password too.
    expected: Option<String>,
}

impl Scram {
    /// Begins an exchange with `nonce` — random in production, fixed in the
    /// tests. PostgreSQL takes the user from the startup packet and ignores the
    /// one here, which is why production passes an empty `username`.
    pub(super) fn new(nonce: &[u8], username: &str) -> Scram {
        let client_nonce = STANDARD.encode(nonce);
        let bare = format!("n={username},r={client_nonce}");
        Scram {
            client_nonce,
            bare,
            expected: None,
        }
    }

    /// The `client-first-message`. `n,,` is the GS2 header: no channel
    /// binding, no authorization identity.
    pub(super) fn initial(&self) -> String {
        format!("n,,{}", self.bare)
    }

    /// Answers the server's challenge with the `client-final-message`.
    pub(super) fn respond(&mut self, password: &str, server_first: &str) -> Result<String, String> {
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
        if !nonce.starts_with(&self.client_nonce) {
            // The server must extend our nonce. One that does not is not the
            // server this exchange started with.
            return Err("the server's SCRAM nonce does not extend the client's".to_string());
        }
        let salt = STANDARD
            .decode(salt)
            .map_err(|_| format!("the server's SCRAM salt is not base64: {salt}"))?;

        let mut salted = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, iterations, &mut salted);

        // `biws` is base64("n,,"), the GS2 header again, as the channel-binding
        // attribute of a client that offered no binding.
        let without_proof = format!("c=biws,r={nonce}");
        let auth_message = format!("{},{server_first},{without_proof}", self.bare);

        let client_key = hmac(&salted, b"Client Key");
        let stored_key = Sha256::digest(client_key);
        let signature = hmac(&stored_key, auth_message.as_bytes());
        let proof: Vec<u8> = client_key
            .iter()
            .zip(signature.iter())
            .map(|(a, b)| a ^ b)
            .collect();

        let server_key = hmac(&salted, b"Server Key");
        self.expected = Some(STANDARD.encode(hmac(&server_key, auth_message.as_bytes())));
        Ok(format!("{without_proof},p={}", STANDARD.encode(proof)))
    }

    /// Checks the server's proof. Mutual authentication: without it the
    /// client has proved itself and learned nothing about who answered.
    pub(super) fn verify(&self, server_final: &str) -> Result<(), String> {
        match (&self.expected, attribute(server_final, "v")) {
            (Some(expected), Some(signature)) if expected == signature => Ok(()),
            _ => Err("the server failed to prove it knows the password".to_string()),
        }
    }
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().into()
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
        let mut scram = Scram::new(&nonce, "user");
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
        let mut scram = Scram::new(b"abc", "");
        let err = scram
            .respond("pw", "r=someone-else,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096")
            .unwrap_err();
        assert!(err.contains("does not extend"), "{err}");
    }

    #[test]
    fn refuses_a_malformed_challenge() {
        let mut scram = Scram::new(b"abc", "");
        assert!(scram.respond("pw", "r=YWJj,i=0,s=AA==").is_err());
        assert!(scram.respond("pw", "garbage").is_err());
    }
}

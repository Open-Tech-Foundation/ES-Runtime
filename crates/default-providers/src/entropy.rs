//! OS-backed [`Entropy`].

use es_runtime_providers::{Entropy, ProviderError};

/// An [`Entropy`] source drawing from the operating system's CSPRNG via
/// `getrandom` (DECISIONS.md D9 is unaffected: this is raw OS entropy, not the
/// `crypto.subtle` algorithm backend).
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn fill(&self, dest: &mut [u8]) -> Result<(), ProviderError> {
        getrandom::fill(dest).map_err(|e| ProviderError::Entropy(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_the_whole_buffer() {
        // Start from a sentinel and require it is overwritten. (Astronomically
        // unlikely to draw all-0x77, so this reliably proves bytes were written.)
        let mut buf = [0x77u8; 32];
        OsEntropy.fill(&mut buf).expect("os entropy");
        assert!(buf.iter().any(|&b| b != 0x77));
    }

    #[test]
    fn distinct_draws_differ() {
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        OsEntropy.fill(&mut a).unwrap();
        OsEntropy.fill(&mut b).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn empty_buffer_is_ok() {
        OsEntropy.fill(&mut []).expect("empty fill is a no-op");
    }
}

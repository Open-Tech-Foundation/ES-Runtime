//! Why the filesystem jail refused a path — **without saying where it led**
//! (DECISIONS D141).
//!
//! The jail decides on canonical paths: a symlink is followed before it is
//! judged, and a relative path is joined onto the base. Those are the paths the
//! checks hold, and they are exactly what a refusal must not repeat. A canonical
//! path outside the root says where a link really points and whether a file
//! exists; a granted root names part of the host the program was never told
//! about; and a message that carries either one carries it into every log the
//! error reaches.
//!
//! So a [`Refusal`] holds no path at all. The only way to turn one into a
//! [`ProviderError`] is [`Refusal::named`], which takes the spelling the
//! program wrote — the one path in the whole exchange that it already knew.

use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;

use crate::path_allowlist::Access;

/// A path the jail would not admit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// It resolved outside every root. `granted` says whether the command line
    /// added roots outside the jail (D54), so the message can say the grant did
    /// not cover it either — without naming what the grant was.
    Escape { granted: bool },
    /// It is inside a root but not in the scope list for this access (D38).
    NotAllowed(Access),
    /// A mutation whose target is a root itself.
    Root,
}

impl Refusal {
    /// The refusal as the error the program sees, naming `shown` — the path as
    /// the program wrote it.
    pub(crate) fn named(self, shown: &str) -> ProviderError {
        let (code, message) = match self {
            Refusal::Escape { granted: false } => (
                ErrorCode::JailEscape,
                format!(
                    "path {shown} escapes the filesystem root \
                     (access outside the root is not permitted)"
                ),
            ),
            Refusal::Escape { granted: true } => (
                ErrorCode::JailEscape,
                format!(
                    "path {shown} escapes the filesystem root and every path granted \
                     on the command line (access outside them is not permitted)"
                ),
            ),
            Refusal::NotAllowed(access) => (
                ErrorCode::PermissionDenied,
                format!("{shown} is not an allowed path ({})", access.as_str()),
            ),
            Refusal::Root => (
                ErrorCode::InvalidPath,
                format!(
                    "refusing to modify {shown}: it is a filesystem root itself \
                     (mutating a root would destroy the sandbox; name an entry inside it)"
                ),
            ),
        };
        ProviderError::Coded { code, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_refusal_names_the_spelling_and_keeps_its_code() {
        for (refusal, code) in [
            (Refusal::Escape { granted: false }, ErrorCode::JailEscape),
            (Refusal::Escape { granted: true }, ErrorCode::JailEscape),
            (
                Refusal::NotAllowed(Access::Read),
                ErrorCode::PermissionDenied,
            ),
            (Refusal::Root, ErrorCode::InvalidPath),
        ] {
            let err = refusal.named("../secret.txt");
            assert_eq!(err.code(), Some(code), "{err}");
            assert!(err.to_string().contains("../secret.txt"), "{err}");
        }
    }

    #[test]
    fn a_granted_root_is_mentioned_but_not_named() {
        let err = Refusal::Escape { granted: true }.named("/x");
        assert!(
            err.to_string().contains("granted on the command line"),
            "{err}"
        );
    }
}

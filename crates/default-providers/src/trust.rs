//! The process trust store: the anchors every TLS client verifies a server
//! against (DECISIONS D197).
//!
//! One value, built once at startup and handed to `fetch`, `runtime:net`,
//! `WebSocket`, the database drivers and the telemetry exporter, so no API can
//! trust a server another one refuses. The default is the bundled Mozilla set;
//! `--tls-ca` replaces it with exactly the sources it lists. Nothing else
//! changes it: no environment variable, no operating-system store.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;
use tokio_rustls::rustls::crypto::aws_lc_rs;
use tokio_rustls::rustls::pki_types::CertificateDer;
use tokio_rustls::rustls::pki_types::pem::{self, SectionKind};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// One `--tls-ca` value: the embedded roots, or a PEM file of anchors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustSource {
    /// The bundled Mozilla roots (`bundled`).
    Bundled,
    /// A PEM file, relative to the working directory.
    File(PathBuf),
}

impl TrustSource {
    /// Parses one `--tls-ca` value. `bundled` names the embedded set; anything
    /// else is a path, so a file of that name is spelled `./bundled`.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "" => Err("--tls-ca takes a PEM file or `bundled`, e.g. --tls-ca=ca.pem".to_string()),
            "bundled" => Ok(Self::Bundled),
            path => Ok(Self::File(PathBuf::from(path))),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Bundled => "bundled".to_string(),
            Self::File(path) => path.display().to_string(),
        }
    }
}

/// The anchors a TLS client verifies against. Cheap to clone: an `Arc` inside.
#[derive(Clone)]
pub struct TrustStore {
    roots: Arc<RootCertStore>,
}

impl Default for TrustStore {
    fn default() -> Self {
        Self::bundled()
    }
}

impl std::fmt::Debug for TrustStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustStore")
            .field("anchors", &self.roots.len())
            .finish()
    }
}

impl TrustStore {
    /// The bundled Mozilla roots, built once per process.
    pub fn bundled() -> Self {
        static BUNDLED: OnceLock<Arc<RootCertStore>> = OnceLock::new();
        let roots = BUNDLED.get_or_init(|| {
            let mut store = RootCertStore::empty();
            store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(store)
        });
        Self {
            roots: roots.clone(),
        }
    }

    /// The store `--tls-ca` describes: the bundled roots when `sources` is
    /// empty, and otherwise exactly what it lists.
    ///
    /// Fails closed. A file that cannot be read, holds no certificate, holds
    /// anything but certificates, or has a certificate outside its validity
    /// period is an error, because a process that starts anyway trusts
    /// something other than what its operator wrote. Errors name the flag and
    /// the file and never quote the file's contents: a key pasted into the
    /// wrong file must not end up in a log.
    pub fn load(sources: &[TrustSource]) -> Result<Self, String> {
        Self::load_at(sources, unix_now())
    }

    fn load_at(sources: &[TrustSource], now: i64) -> Result<Self, String> {
        if sources.is_empty() {
            return Ok(Self::bundled());
        }
        for (i, source) in sources.iter().enumerate() {
            if sources[..i].contains(source) {
                return Err(format!("--tls-ca={} is given twice", source.describe()));
            }
        }
        let mut store = RootCertStore::empty();
        for source in sources {
            match source {
                TrustSource::Bundled => {
                    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                }
                TrustSource::File(path) => {
                    let flag = format!("--tls-ca={}", path.display());
                    let bytes =
                        std::fs::read(path).map_err(|e| format!("{flag}: cannot read it: {e}"))?;
                    for cert in certificates(&bytes, now).map_err(|e| format!("{flag}: {e}"))? {
                        store
                            .add(cert)
                            .map_err(|_| format!("{flag}: a certificate is not a valid anchor"))?;
                    }
                }
            }
        }
        Ok(Self {
            roots: Arc::new(store),
        })
    }

    /// How many anchors the store holds.
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    /// Whether the store holds no anchor (never true of one [`load`](Self::load) built).
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// A store trusting exactly `certs` — for tests standing up their own CA.
    pub fn from_certificates(
        certs: impl IntoIterator<Item = CertificateDer<'static>>,
    ) -> Result<Self, String> {
        let mut store = RootCertStore::empty();
        for cert in certs {
            store.add(cert).map_err(|e| e.to_string())?;
        }
        Ok(Self {
            roots: Arc::new(store),
        })
    }

    /// The anchors for one connection: this store, plus `ca` (PEM) when given.
    ///
    /// Added, never instead: a caller naming a private authority is saying
    /// "trust this as well", and hostname and chain checks still apply, so a
    /// per-connection `ca` can widen whom a connection trusts but never turn
    /// verification off.
    pub(crate) fn roots_with(&self, ca: &[u8]) -> Result<Arc<RootCertStore>, ProviderError> {
        if ca.is_empty() {
            return Ok(self.roots.clone());
        }
        let mut store = (*self.roots).clone();
        let mut added = 0usize;
        for cert in <CertificateDer as pem::PemObject>::pem_slice_iter(ca) {
            store.add(cert.map_err(tls_err)?).map_err(tls_err)?;
            added += 1;
        }
        if added == 0 {
            return Err(tls_err("the supplied CA contains no PEM certificate"));
        }
        Ok(Arc::new(store))
    }

    /// A client configuration verifying against this store and offering
    /// `alpn`. The `aws-lc-rs` provider is chosen explicitly: both ring and
    /// aws-lc-rs are linked, so the process default is ambiguous and
    /// `ClientConfig::builder()` would panic.
    pub fn client_config(&self, alpn: &[&str]) -> ClientConfig {
        client_config(self.roots.clone(), alpn)
    }
}

/// A client configuration verifying against `roots`, offering `alpn`.
pub(crate) fn client_config(roots: Arc<RootCertStore>, alpn: &[&str]) -> ClientConfig {
    let provider = Arc::new(aws_lc_rs::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the aws-lc-rs provider supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    config
}

fn tls_err(e: impl ToString) -> ProviderError {
    ProviderError::Coded {
        code: ErrorCode::Tls,
        message: e.to_string(),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The certificates in a `--tls-ca` file, each checked to be in date at `now`.
fn certificates(bytes: &[u8], now: i64) -> Result<Vec<CertificateDer<'static>>, String> {
    let mut reader = bytes;
    let mut certs = Vec::new();
    loop {
        let section = pem::from_buf(&mut reader).map_err(|e| pem_error(&e))?;
        let Some((kind, der)) = section else { break };
        if kind != SectionKind::Certificate {
            return Err(match kind {
                SectionKind::PrivateKey
                | SectionKind::RsaPrivateKey
                | SectionKind::EcPrivateKey => {
                    "it holds a private key; a trust file holds certificates only, never a key"
                        .to_string()
                }
                other => format!(
                    "it holds a {} block; only CERTIFICATE blocks are accepted",
                    label(other)
                ),
            });
        }
        let n = certs.len() + 1;
        let cert = CertificateDer::from(der);
        if RootCertStore::empty().add(cert.clone()).is_err() {
            return Err(format!("certificate {n} is not a valid X.509 certificate"));
        }
        let (not_before, not_after) = validity(&cert)
            .ok_or_else(|| format!("certificate {n} is not a valid X.509 certificate"))?;
        if now < not_before.unix {
            return Err(format!("certificate {n} is not valid until {not_before}"));
        }
        if now > not_after.unix {
            return Err(format!("certificate {n} expired on {not_after}"));
        }
        certs.push(cert);
    }
    if certs.is_empty() {
        return Err("it holds no CERTIFICATE block (expected PEM)".to_string());
    }
    Ok(certs)
}

/// A PEM error, described without the bytes it was about.
fn pem_error(e: &pem::Error) -> String {
    match e {
        pem::Error::MissingSectionEnd { .. } => "a PEM block has no END line".to_string(),
        pem::Error::IllegalSectionStart { .. } => "a PEM BEGIN line is malformed".to_string(),
        pem::Error::Base64Decode(_) => "a PEM block is not valid base64".to_string(),
        pem::Error::SectionTooLarge => "a PEM block is too large".to_string(),
        _ => "it is not valid PEM".to_string(),
    }
}

fn label(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::Certificate => "CERTIFICATE",
        SectionKind::PublicKey => "PUBLIC KEY",
        SectionKind::Crl => "X509 CRL",
        SectionKind::Csr => "CERTIFICATE REQUEST",
        SectionKind::EchConfigList => "ECHCONFIG",
        _ => "non-certificate",
    }
}

/// A certificate's `notBefore` or `notAfter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Time {
    unix: i64,
    date: (i64, u32, u32),
}

impl std::fmt::Display for Time {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (y, m, d) = self.date;
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

/// `(notBefore, notAfter)` of a DER certificate.
///
/// Walks only as far as the validity field — `Certificate` → `tbsCertificate`
/// → version, serial, signature algorithm, issuer → `validity` (RFC 5280
/// §4.1). rustls has already parsed the certificate as an anchor; this reads
/// the two dates it does not expose. Any structure it does not expect is
/// `None`, never a panic.
fn validity(der: &[u8]) -> Option<(Time, Time)> {
    let (cert, _) = element(der, 0x30)?;
    let (tbs, _) = element(cert, 0x30)?;
    let mut rest = tbs;
    if rest.first() == Some(&0xa0) {
        rest = element(rest, 0xa0)?.1;
    }
    rest = element(rest, 0x02)?.1; // serialNumber
    rest = element(rest, 0x30)?.1; // signature
    rest = element(rest, 0x30)?.1; // issuer
    let (validity, _) = element(rest, 0x30)?;
    let (not_before, rest) = time(validity)?;
    let (not_after, _) = time(rest)?;
    Some((not_before, not_after))
}

/// One DER element with `tag`: `(contents, what follows)`.
fn element(input: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&found, rest) = input.split_first()?;
    if found != tag {
        return None;
    }
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return None;
        }
        let len = rest[..count]
            .iter()
            .fold(0usize, |len, &b| (len << 8) | usize::from(b));
        (len, &rest[count..])
    };
    if rest.len() < len {
        return None;
    }
    Some(rest.split_at(len))
}

/// A `Time` (RFC 5280 §4.1.2.5): `UTCTime` as `YYMMDDHHMMSSZ` or
/// `GeneralizedTime` as `YYYYMMDDHHMMSSZ`, the only forms the profile allows.
fn time(input: &[u8]) -> Option<(Time, &[u8])> {
    let tag = *input.first()?;
    let (body, rest) = element(input, tag)?;
    let (year, digits) = match (tag, body.len()) {
        // RFC 5280: a two-digit year of 50 or more is 19YY, otherwise 20YY.
        (0x17, 13) => {
            let yy = number(&body[..2])?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &body[2..])
        }
        (0x18, 15) => (number(&body[..4])?, &body[4..]),
        _ => return None,
    };
    if digits.last() != Some(&b'Z') {
        return None;
    }
    let month = number(&digits[0..2])?;
    let day = number(&digits[2..4])?;
    let hour = number(&digits[4..6])?;
    let minute = number(&digits[6..8])?;
    let second = number(&digits[8..10])?;
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let unix = days * 86_400 + hour * 3_600 + minute * 60 + second;
    let date = (year, u32::try_from(month).ok()?, u32::try_from(day).ok()?);
    Some((Time { unix, date }, rest))
}

fn number(digits: &[u8]) -> Option<i64> {
    digits.iter().try_fold(0i64, |n, &b| {
        b.is_ascii_digit().then(|| n * 10 + i64::from(b - b'0'))
    })
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};

    const DAY: i64 = 86_400;

    fn ca_with(not_before: (i32, u8, u8), not_after: (i32, u8, u8)) -> (String, String) {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.not_before = rcgen::date_time_ymd(not_before.0, not_before.1, not_before.2);
        params.not_after = rcgen::date_time_ymd(not_after.0, not_after.1, not_after.2);
        let key = KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        (cert.pem(), key.serialize_pem())
    }

    fn ca() -> (String, String) {
        ca_with((2020, 1, 1), (2090, 1, 1))
    }

    fn write(name: &str, contents: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esrun-trust-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn file(name: &str, contents: &[u8]) -> TrustSource {
        TrustSource::File(write(name, contents))
    }

    fn now() -> i64 {
        unix_now()
    }

    #[test]
    fn no_flag_is_the_bundled_roots() {
        let store = TrustStore::load(&[]).unwrap();
        assert_eq!(store.len(), webpki_roots::TLS_SERVER_ROOTS.len());
        assert_eq!(TrustStore::default().len(), store.len());
    }

    #[test]
    fn a_file_alone_replaces_the_bundled_roots() {
        let (pem, _) = ca();
        let store = TrustStore::load(&[file("only.pem", pem.as_bytes())]).unwrap();
        assert_eq!(
            store.len(),
            1,
            "the bundled roots must not survive --tls-ca=<file>"
        );
    }

    #[test]
    fn bundled_and_a_file_trust_both() {
        let (pem, _) = ca();
        let store =
            TrustStore::load(&[TrustSource::Bundled, file("both.pem", pem.as_bytes())]).unwrap();
        assert_eq!(store.len(), webpki_roots::TLS_SERVER_ROOTS.len() + 1);
    }

    #[test]
    fn a_file_may_hold_several_certificates() {
        let (a, _) = ca();
        let (b, _) = ca();
        let store = TrustStore::load(&[file("two.pem", format!("{a}{b}").as_bytes())]).unwrap();
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn parses_the_flag_value() {
        assert_eq!(TrustSource::parse("bundled").unwrap(), TrustSource::Bundled);
        assert_eq!(
            TrustSource::parse("./bundled").unwrap(),
            TrustSource::File(PathBuf::from("./bundled"))
        );
        assert!(TrustSource::parse("").is_err());
    }

    #[test]
    fn a_source_given_twice_is_refused() {
        let err = TrustStore::load(&[TrustSource::Bundled, TrustSource::Bundled]).unwrap_err();
        assert!(err.contains("given twice"), "{err}");
        let (pem, _) = ca();
        let source = file("twice.pem", pem.as_bytes());
        let err = TrustStore::load(&[source.clone(), source]).unwrap_err();
        assert!(err.contains("given twice"), "{err}");
    }

    #[test]
    fn a_missing_file_is_refused_with_its_name() {
        let missing = TrustSource::File(PathBuf::from("/nonexistent/esrun-ca.pem"));
        let err = TrustStore::load(&[missing]).unwrap_err();
        assert!(
            err.starts_with("--tls-ca=/nonexistent/esrun-ca.pem: cannot read it"),
            "{err}"
        );
    }

    #[test]
    fn a_file_without_a_certificate_is_refused() {
        for (name, contents) in [
            ("empty.pem", &b""[..]),
            ("text.pem", b"not a certificate\n"),
            ("der.pem", &[0x30, 0x82, 0x01, 0x00, 0x02, 0x01][..]),
        ] {
            let err = TrustStore::load(&[file(name, contents)]).unwrap_err();
            assert!(err.contains("no CERTIFICATE block"), "{name}: {err}");
        }
    }

    #[test]
    fn a_private_key_is_refused_without_quoting_it() {
        let (cert, key) = ca();
        let rsa = key.replace("PRIVATE KEY", "RSA PRIVATE KEY");
        let ec = key.replace("PRIVATE KEY", "EC PRIVATE KEY");
        for (name, contents) in [
            ("key.pem", key.clone()),
            ("cert-and-key.pem", format!("{cert}{key}")),
            ("key-then-cert.pem", format!("{key}{cert}")),
            ("rsa.pem", rsa),
            ("ec.pem", ec),
        ] {
            let err = TrustStore::load(&[file(name, contents.as_bytes())]).unwrap_err();
            assert!(err.contains("private key"), "{name}: {err}");
            // Not one line of the key's base64 may reach the message.
            for line in key.lines().filter(|l| !l.starts_with("-----")) {
                assert!(!err.contains(line), "{name}: the error quotes the key");
            }
        }
    }

    #[test]
    fn other_pem_blocks_are_refused() {
        let (cert, _) = ca();
        for (label, name) in [
            ("PUBLIC KEY", "public.pem"),
            ("X509 CRL", "crl.pem"),
            ("CERTIFICATE REQUEST", "csr.pem"),
        ] {
            let block = format!("-----BEGIN {label}-----\nAAAA\n-----END {label}-----\n");
            let err =
                TrustStore::load(&[file(name, format!("{cert}{block}").as_bytes())]).unwrap_err();
            assert!(err.contains(&format!("{label} block")), "{label}: {err}");
        }
    }

    #[test]
    fn malformed_pem_is_refused() {
        let (cert, _) = ca();
        let unterminated = cert.replace("-----END CERTIFICATE-----", "");
        let err =
            TrustStore::load(&[file("unterminated.pem", unterminated.as_bytes())]).unwrap_err();
        assert!(err.contains("no END line"), "{err}");
        let bad_base64 = "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n";
        let err = TrustStore::load(&[file("base64.pem", bad_base64.as_bytes())]).unwrap_err();
        assert!(err.contains("base64"), "{err}");
    }

    #[test]
    fn a_certificate_block_that_is_not_x509_is_refused() {
        let junk = "-----BEGIN CERTIFICATE-----\nAAECAwQFBgc=\n-----END CERTIFICATE-----\n";
        let err = TrustStore::load(&[file("junk.pem", junk.as_bytes())]).unwrap_err();
        assert!(
            err.contains("certificate 1 is not a valid X.509 certificate"),
            "{err}"
        );
    }

    #[test]
    fn an_expired_certificate_is_refused() {
        let (pem, _) = ca_with((2001, 1, 1), (2002, 6, 30));
        let err = TrustStore::load(&[file("expired.pem", pem.as_bytes())]).unwrap_err();
        assert!(err.contains("certificate 1 expired on 2002-06-30"), "{err}");
    }

    #[test]
    fn a_certificate_not_yet_valid_is_refused() {
        let (pem, _) = ca_with((2080, 3, 1), (2090, 1, 1));
        let err = TrustStore::load(&[file("future.pem", pem.as_bytes())]).unwrap_err();
        assert!(
            err.contains("certificate 1 is not valid until 2080-03-01"),
            "{err}"
        );
    }

    #[test]
    fn the_bad_certificate_is_named_by_position() {
        let (good, _) = ca();
        let (expired, _) = ca_with((2001, 1, 1), (2002, 1, 1));
        let err = TrustStore::load(&[file("second.pem", format!("{good}{expired}").as_bytes())])
            .unwrap_err();
        assert!(err.contains("certificate 2 expired"), "{err}");
    }

    #[test]
    fn validity_is_checked_against_the_clock_given() {
        let (pem, _) = ca_with((2030, 1, 1), (2031, 1, 1));
        let bytes = pem.as_bytes();
        let start = days_from_civil(2030, 1, 1) * DAY;
        let end = days_from_civil(2031, 1, 1) * DAY;
        assert!(certificates(bytes, start - 1).is_err());
        assert!(certificates(bytes, start).is_ok());
        assert!(certificates(bytes, end).is_ok());
        assert!(certificates(bytes, end + 1).is_err());
        assert!(certificates(bytes, now()).is_err());
    }

    #[test]
    fn reads_both_time_forms() {
        // UTCTime 500101000000Z is 1950; 491231235959Z is 2049 (RFC 5280).
        let utc = |s: &str| {
            let mut der = vec![0x17, s.len() as u8];
            der.extend_from_slice(s.as_bytes());
            time(&der).map(|(t, _)| t.date)
        };
        assert_eq!(utc("500101000000Z"), Some((1950, 1, 1)));
        assert_eq!(utc("491231235959Z"), Some((2049, 12, 31)));
        let generalized = |s: &str| {
            let mut der = vec![0x18, s.len() as u8];
            der.extend_from_slice(s.as_bytes());
            time(&der).map(|(t, _)| t)
        };
        let t = generalized("20240229120000Z").unwrap();
        assert_eq!(t.date, (2024, 2, 29));
        assert_eq!(t.unix, 1_709_208_000);
        assert_eq!(
            generalized("99991231235959Z").map(|t| t.date),
            Some((9999, 12, 31))
        );
    }

    #[test]
    fn refuses_times_outside_the_profile() {
        let utc = |s: &str| {
            let mut der = vec![0x17, s.len() as u8];
            der.extend_from_slice(s.as_bytes());
            time(&der).map(|(t, _)| t)
        };
        for bad in [
            "500101000000",      // no Z
            "5001010000Z",       // no seconds
            "501301000000Z",     // month 13
            "500132000000Z",     // day 32
            "230229000000Z",     // 2023 is not a leap year
            "500101240000Z",     // hour 24
            "500101006000Z",     // minute 60
            "500101000060Z",     // second 60
            "50010100000+Z",     // not a digit
            "500101000000+0100", // an offset
        ] {
            assert!(utc(bad).is_none(), "{bad} was accepted");
        }
    }

    #[test]
    fn malformed_der_is_none_never_a_panic() {
        let (pem, _) = ca();
        let der = <CertificateDer as pem::PemObject>::from_pem_slice(pem.as_bytes()).unwrap();
        let der = der.as_ref();
        assert!(validity(der).is_some());
        // Every truncation, and every single-byte corruption, of a real
        // certificate either parses or is refused — none may panic.
        for len in 0..der.len() {
            let _ = validity(&der[..len]);
        }
        for i in 0..der.len() {
            for flip in [0x01u8, 0x80, 0xff] {
                let mut bytes = der.to_vec();
                bytes[i] ^= flip;
                let _ = validity(&bytes);
            }
        }
        // Lengths that claim more than is there, or use a forbidden form.
        assert!(element(&[0x30, 0x85, 1, 1, 1, 1, 1], 0x30).is_none());
        assert!(element(&[0x30, 0x80], 0x30).is_none());
        assert!(element(&[0x30, 0x84, 0xff, 0xff, 0xff, 0xff], 0x30).is_none());
        assert!(element(&[0x30, 0x05, 1, 2], 0x30).is_none());
    }

    #[test]
    fn a_per_connection_ca_adds_to_the_store() {
        let (a, _) = ca();
        let (b, _) = ca();
        let store = TrustStore::load(&[file("base.pem", a.as_bytes())]).unwrap();
        let roots = store.roots_with(b.as_bytes()).unwrap();
        assert_eq!(roots.len(), 2, "the per-connection CA replaced the store");
        assert_eq!(
            store.len(),
            1,
            "the per-connection CA leaked into the process store"
        );
        assert_eq!(store.roots_with(b"").unwrap().len(), 1);
        assert!(store.roots_with(b"no certificate here").is_err());
    }
}

/// A private CA and the TLS servers it signs, for the providers' hermetic
/// trust tests.
#[cfg(test)]
pub(crate) mod test_pki {
    use std::sync::Arc;

    use rcgen::{
        BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;
    use tokio_rustls::rustls::ServerConfig;
    use tokio_rustls::rustls::crypto::aws_lc_rs;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

    use super::TrustStore;

    pub(crate) struct Ca {
        params: CertificateParams,
        key: KeyPair,
        pub(crate) pem: String,
        pub(crate) der: CertificateDer<'static>,
    }

    pub(crate) struct Leaf {
        pub(crate) chain: Vec<CertificateDer<'static>>,
        pub(crate) key: PrivateKeyDer<'static>,
    }

    impl Ca {
        pub(crate) fn new(name: &str) -> Ca {
            let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
            params.distinguished_name.push(DnType::CommonName, name);
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
            let key = KeyPair::generate().unwrap();
            let cert = params.self_signed(&key).unwrap();
            Ca {
                pem: cert.pem(),
                der: cert.der().clone(),
                params,
                key,
            }
        }

        /// A server certificate for `names`, valid from `not_before` to
        /// `not_after` (years).
        pub(crate) fn leaf_valid(&self, names: &[&str], not_before: i32, not_after: i32) -> Leaf {
            let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
            let mut params = CertificateParams::new(names).unwrap();
            params.not_before = rcgen::date_time_ymd(not_before, 1, 1);
            params.not_after = rcgen::date_time_ymd(not_after, 1, 1);
            let key = KeyPair::generate().unwrap();
            let issuer = Issuer::from_params(&self.params, &self.key);
            let cert = params.signed_by(&key, &issuer).unwrap();
            Leaf {
                chain: vec![cert.der().clone()],
                key: PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key.serialize_der())),
            }
        }

        pub(crate) fn leaf(&self, names: &[&str]) -> Leaf {
            self.leaf_valid(names, 2020, 2090)
        }

        /// A store trusting this CA alone.
        pub(crate) fn trust(&self) -> TrustStore {
            TrustStore::from_certificates([self.der.clone()]).unwrap()
        }
    }

    /// A one-connection TLS server presenting `leaf` and advertising `alpn`.
    /// It answers `response` after the first read; the task resolves to the
    /// ALPN protocol negotiated, or `None` when the handshake failed.
    pub(crate) async fn tls_server(
        leaf: Leaf,
        alpn: &[&str],
        response: &'static [u8],
    ) -> (u16, tokio::task::JoinHandle<Option<Option<Vec<u8>>>>) {
        let mut config =
            ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(leaf.chain, leaf.key)
                .unwrap();
        config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.ok()?;
            let mut tls = acceptor.accept(tcp).await.ok()?;
            let negotiated = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
            let mut buf = [0u8; 4096];
            let _ = tls.read(&mut buf).await;
            let _ = tls.write_all(response).await;
            let _ = tls.shutdown().await;
            Some(negotiated)
        });
        (port, task)
    }
}

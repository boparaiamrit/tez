//! tez's own certificate authority, at `config/ca/root.crt` and `root.key`, shared by both web
//! servers. Caddy issues its certificates from it (the Caddyfile's `pki` block); for Nginx, tez
//! signs one certificate that covers every host name and renews it when the hosts change or it
//! nears expiry. `tez trust` puts this root into the system's trust store.

use crate::paths;
use crate::util::R;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384,
};
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};

pub fn ca_dir() -> PathBuf { paths::config_dir().join("ca") }
pub fn root_crt() -> PathBuf { paths::local_ca_root() }
pub fn root_key() -> PathBuf { ca_dir().join("root.key") }

/// Where Caddy kept its own local root before tez owned one.
fn caddy_root() -> PathBuf { paths::caddy_storage().join("pki").join("authorities").join("local") }

/// Make sure the authority exists. The first time, tez adopts Caddy's local root if there is
/// one, so a browser that already trusts it keeps working; otherwise it creates an ECDSA P-256
/// root valid for ten years.
pub fn ensure_ca() -> R<()> {
    let complete = || root_crt().exists() && root_key().exists();
    if complete() { return Ok(()); }
    let (c, k) = (caddy_root().join("root.crt"), caddy_root().join("root.key"));
    let caddy = match (std::fs::read_to_string(&c), std::fs::read_to_string(&k)) {
        // Only adopt it if tez can sign with it.
        (Ok(cert), Ok(key)) if signing_key(&key).and_then(|sk| Ok(Issuer::from_ca_cert_pem(&cert, sk)?)).is_ok() => Some((cert, key)),
        _ => None,
    };
    let (cert, key) = match caddy { Some(pair) => pair, None => new_root()? };
    // Both files go into a new folder that is moved into place in one step, so two tez processes
    // starting at once cannot leave one's certificate beside the other's key.
    let tmp = paths::config_dir().join(format!("ca.new-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    write_secret(&tmp.join("root.key"), &key)?;
    std::fs::write(tmp.join("root.crt"), &cert)?;
    if !ca_dir().exists() && std::fs::rename(&tmp, ca_dir()).is_ok() { return Ok(()); }
    let _ = std::fs::remove_dir_all(&tmp);
    if complete() { return Ok(()); }
    // A folder left half-written by an older run: fill it in.
    std::fs::create_dir_all(ca_dir())?;
    write_secret(&root_key(), &key)?;
    std::fs::write(root_crt(), cert)?;
    Ok(())
}

/// A new root: ECDSA P-256, valid for ten years. Returns (certificate, key) as PEM.
pub fn new_root() -> R<(String, String)> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let now = OffsetDateTime::now_utc();
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, "tez");
    dn.push(DnType::CommonName, format!("tez Local Authority - {} ECC Root", now.year()));
    p.distinguished_name = dn;
    // Unconstrained: Caddy puts its own intermediate between this root and the site certificates.
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    p.not_before = now - Duration::days(1);
    p.not_after = now + Duration::days(3650);
    let cert = p.self_signed(&key)?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// A server certificate for `names`, signed by the root, valid for `days`: (certificate, key).
pub fn issue(root_pem: &str, root_key_pem: &str, names: &[String], days: i64) -> R<(String, String)> {
    let issuer = Issuer::from_ca_cert_pem(root_pem, signing_key(root_key_pem)?)?;
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let now = OffsetDateTime::now_utc();
    let mut p = CertificateParams::new(names.to_vec())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, "tez");
    dn.push(DnType::CommonName, "tez local sites");
    p.distinguished_name = dn;
    p.is_ca = IsCa::ExplicitNoCa;
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    p.use_authority_key_identifier_extension = true;
    p.not_before = now - Duration::hours(1);
    p.not_after = now + Duration::days(days);
    let cert = p.signed_by(&key, &issuer)?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// The root's private key as rcgen wants it. Caddy writes SEC1 ("EC PRIVATE KEY"); ring only
/// reads PKCS#8, so a SEC1 key is wrapped first.
fn signing_key(pem_text: &str) -> R<KeyPair> {
    if let Ok(k) = KeyPair::from_pem(pem_text) { return Ok(k); }
    let der = pem_body(pem_text, "EC PRIVATE KEY").ok_or("the key is neither PKCS#8 nor an EC private key")?;
    const P256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
    const P384: &[u8] = &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22];
    // The curve named inside the key first; a key without one is tried as P-256, then P-384.
    let order = if contains(&der, P384) { [(P384, &PKCS_ECDSA_P384_SHA384), (P256, &PKCS_ECDSA_P256_SHA256)] }
        else { [(P256, &PKCS_ECDSA_P256_SHA256), (P384, &PKCS_ECDSA_P384_SHA384)] };
    let mut last = None;
    for (curve, alg) in order {
        // PrivateKeyInfo { version 0, AlgorithmIdentifier { id-ecPublicKey, curve }, OCTET STRING sec1 }
        let mut alg_id = vec![0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
        alg_id.extend_from_slice(curve);
        let mut body = vec![0x02, 0x01, 0x00];
        body.extend(der_tlv(0x30, &alg_id));
        body.extend(der_tlv(0x04, &der));
        let pkcs8 = der_tlv(0x30, &body);
        match KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8.as_slice().into(), alg) {
            Ok(k) => return Ok(k),
            Err(e) => last = Some(e),
        }
    }
    Err(format!("the CA key could not be read: {}", last.map(|e| e.to_string()).unwrap_or_default()).into())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool { hay.windows(needle.len()).any(|w| w == needle) }

fn der_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let n = content.len();
    let mut out = vec![tag];
    if n < 0x80 { out.push(n as u8); }
    else if n < 0x100 { out.extend([0x81, n as u8]); }
    else { out.extend([0x82, (n >> 8) as u8, n as u8]); }
    out.extend_from_slice(content);
    out
}

/// The bytes inside one `-----BEGIN <label>-----` block.
fn pem_body(text: &str, label: &str) -> Option<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let start = text.find(&begin)? + begin.len();
    let stop = start + text[start..].find(&end)?;
    let b64: String = text[start..stop].chars().filter(|c| !c.is_whitespace()).collect();
    base64_decode(&b64)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c { b'A'..=b'Z' => c - b'A', b'a'..=b'z' => c - b'a' + 26, b'0'..=b'9' => c - b'0' + 52, b'+' => 62, b'/' => 63, _ => return None } as u32)
    };
    let bytes: Vec<u8> = s.bytes().filter(|&c| c != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() { acc |= val(c)? << (18 - 6 * i); }
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 { out.push((acc >> 8) as u8); }
        if chunk.len() > 3 { out.push(acc as u8); }
    }
    Some(out)
}

/// A private key file only this user can read (macOS and Linux; Windows inherits the folder's ACL).
fn write_secret(path: &Path, text: &str) -> R<()> {
    std::fs::write(path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// A short fingerprint of the root certificate, so a new root forces a new site certificate.
fn fingerprint(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() { h ^= b as u64; h = h.wrapping_mul(0x0100_0000_01b3); }
    format!("{h:016x}")
}

/// Days a site certificate is valid, and how close to the end tez renews it.
const LEAF_DAYS: i64 = 90;
const RENEW_DAYS: i64 = 30;

/// One certificate for every host Nginx serves, signed by tez's root: `(cert, key)` paths.
/// It is written again only when the host list or the root changes, or within 30 days of expiry.
pub fn ensure_leaf(dir: &Path, hosts: &[String]) -> R<(PathBuf, PathBuf)> {
    ensure_ca()?;
    std::fs::create_dir_all(dir)?;
    let (crt, key, meta) = (dir.join("sites.crt"), dir.join("sites.key"), dir.join("sites.json"));
    let mut names: Vec<String> = hosts.iter().map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()).collect();
    names.sort();
    names.dedup();
    let root_pem = std::fs::read_to_string(root_crt())?;
    let ca = fingerprint(&root_pem);
    let now = OffsetDateTime::now_utc();
    if crt.exists() && key.exists() {
        if let Ok(v) = std::fs::read_to_string(&meta).map(|t| serde_json::from_str::<serde_json::Value>(&t).unwrap_or_default()) {
            let same_hosts = v["hosts"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).eq(names.iter().map(|s| s.as_str()))).unwrap_or(false);
            let fresh = v["not_after"].as_i64().map(|t| t - now.unix_timestamp() > RENEW_DAYS * 86_400).unwrap_or(false);
            if same_hosts && fresh && v["ca"].as_str() == Some(ca.as_str()) { return Ok((crt, key)); }
        }
    }
    let (cert_pem, key_pem) = issue(&root_pem, &std::fs::read_to_string(root_key())?, &names, LEAF_DAYS)?;
    write_secret(&key, &key_pem)?;
    std::fs::write(&crt, cert_pem)?;
    let not_after = (now + Duration::days(LEAF_DAYS)).unix_timestamp();
    std::fs::write(&meta, serde_json::to_string_pretty(&serde_json::json!({ "hosts": names, "not_after": not_after, "ca": ca }))?)?;
    Ok((crt, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_der() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGk=").unwrap(), b"hi");
        assert_eq!(der_tlv(0x04, &[1, 2]), vec![0x04, 2, 1, 2]);
        assert_eq!(der_tlv(0x30, &[0u8; 200])[..3], [0x30, 0x81, 200]);
    }

    /// One DER element: (content, everything after it).
    fn tlv(d: &[u8]) -> (&[u8], &[u8]) {
        let (len, hdr) = match d[1] { n if n < 0x80 => (n as usize, 2), 0x81 => (d[2] as usize, 3), _ => ((d[2] as usize) << 8 | d[3] as usize, 4) };
        (&d[hdr..hdr + len], &d[hdr + len..])
    }

    fn b64(data: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for j in 0..4 { if j <= c.len() { s.push(T[(n >> (18 - 6 * j) & 63) as usize] as char); } else { s.push('='); } }
        }
        s
    }

    #[test]
    fn sec1_keys_are_accepted() {
        // Caddy stores its root key as SEC1 ("EC PRIVATE KEY"): the structure PKCS#8 carries in
        // its last OCTET STRING. Take a fresh key apart that way and read it back.
        let k = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let pkcs8 = k.serialize_der();
        let (inner, _) = tlv(&pkcs8);
        let (_, rest) = tlv(inner); // version
        let (_, rest) = tlv(rest); // algorithm
        let (sec1, _) = tlv(rest);
        let pem = format!("-----BEGIN EC PRIVATE KEY-----\n{}\n-----END EC PRIVATE KEY-----\n", b64(sec1));
        let back = signing_key(&pem).unwrap();
        assert_eq!(back.public_key_raw(), k.public_key_raw());
        assert_eq!(base64_decode(&b64(sec1)).unwrap(), sec1);
    }

    #[test]
    fn site_certificate_chains_to_the_root() {
        use x509_parser::prelude::{FromDer, X509Certificate};
        let (root_pem, root_key) = new_root().unwrap();
        let names = vec!["hello.test".to_string(), "hello.localhost".into(), "tez.localhost".into()];
        let (leaf_pem, leaf_key) = issue(&root_pem, &root_key, &names, LEAF_DAYS).unwrap();
        assert!(leaf_key.contains("PRIVATE KEY"));
        let root_der = pem_body(&root_pem, "CERTIFICATE").unwrap();
        let leaf_der = pem_body(&leaf_pem, "CERTIFICATE").unwrap();
        let (_, root) = X509Certificate::from_der(&root_der).unwrap();
        let (_, leaf) = X509Certificate::from_der(&leaf_der).unwrap();
        assert!(root.is_ca() && !leaf.is_ca());
        assert_eq!(leaf.issuer().as_raw(), root.subject().as_raw());
        leaf.verify_signature(Some(root.public_key())).unwrap();
        root.verify_signature(None).unwrap();
        let sans: Vec<String> = leaf.subject_alternative_name().unwrap().unwrap().value.general_names.iter().map(|g| g.to_string()).collect();
        assert_eq!(sans.len(), 3);
        assert!(sans.iter().any(|s| s.contains("hello.localhost")));
        let days = (leaf.validity().not_after.timestamp() - leaf.validity().not_before.timestamp()) / 86_400;
        assert!((89..=91).contains(&days));
    }
}

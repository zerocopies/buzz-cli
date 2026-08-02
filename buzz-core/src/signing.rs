//! Ed25519 signing for compliance-report exports (deck slide 08:
//! "sha256: 8f21…c04a — signed by fleet key #003").
//!
//! IMPORTANT — this is NOT a reuse of existing key infrastructure. The
//! deck and earlier comments in this crate (`buzz-gateway/src/audit.rs`,
//! now deleted) assumed an AES-256-GCM vault / Argon2id-protected Ed25519
//! key already existed elsewhere in this project. It does not — verified
//! by an exhaustive search (both repos, every branch, every Cargo.lock)
//! before writing this file, not assumed. This module generates and
//! persists its own Ed25519 keypair instead, using the exact same
//! "generate on first use, persist under `~/.buzz/`, 0600 permissions"
//! convention `buzz-gateway/src/auth.rs` already uses for the gateway's
//! bearer token. If real vault-level key material exists or gets built
//! later, swapping it in means replacing `load_or_generate_key_pair`
//! below — a single, obvious seam — not unwinding something that
//! pretended to be that from the start.

use crate::audit::expand_tilde;
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Default private-key location — hex-encoded 32-byte seed, 0600.
/// "Private" in the Unix-permissions sense only, same as the gateway
/// token: this is a single-user-machine threat model, not a hardened
/// secrets store.
pub fn default_key_path() -> PathBuf {
    resolve("~/.buzz/audit_signing.key")
}

/// Default public-key location — hex-encoded 32 bytes, safe to copy
/// elsewhere or hand to whoever needs to verify a report without ever
/// touching the private key.
pub fn default_pubkey_path() -> PathBuf {
    resolve("~/.buzz/audit_signing.pub")
}

fn resolve(path: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    expand_tilde(path, &home)
}

/// Loads the signing key at `key_path`, generating a fresh one (and
/// writing both `key_path` and `pubkey_path`) if it doesn't exist yet.
/// Mirrors `buzz-gateway::auth::issue_and_persist` exactly: same
/// directory convention, same "create if missing" shape, same 0600 perms
/// on the secret half.
pub fn load_or_generate_key_pair(key_path: &Path, pubkey_path: &Path) -> std::io::Result<SigningKey> {
    if let Ok(hex_seed) = std::fs::read_to_string(key_path) {
        let seed = decode_hex_32(hex_seed.trim()).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} does not contain a valid 32-byte hex-encoded Ed25519 seed",
                    key_path.display()
                ),
            )
        })?;
        return Ok(SigningKey::from_bytes(&seed));
    }

    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let signing_key = SigningKey::from_bytes(&seed);

    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(key_path, encode_hex(&seed))?;
    std::fs::write(pubkey_path, encode_hex(&signing_key.verifying_key().to_bytes()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(signing_key)
}

/// Loads just the public half — what `verify` needs, and *all* it should
/// ever need: verifying a signature never requires the private key.
pub fn load_verifying_key(pubkey_path: &Path) -> std::io::Result<VerifyingKey> {
    let hex = std::fs::read_to_string(pubkey_path).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("could not read public key at {}: {e}", pubkey_path.display()),
        )
    })?;
    let bytes = decode_hex_32(hex.trim()).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} does not contain a valid 32-byte hex-encoded Ed25519 public key", pubkey_path.display()),
        )
    })?;
    VerifyingKey::from_bytes(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// Short, stable identifier for a public key — deck slide 08's "signed
/// by fleet key #003", except derived from the actual key material
/// (first 16 hex chars of SHA-256(pubkey)) instead of an arbitrary
/// sequential number, so it's actually meaningful: two reports with the
/// same `key_id` are provably signed by the same key, and a verifier can
/// confirm a given public key produces a given `key_id` before trusting
/// it.
pub fn key_id(verifying_key: &VerifyingKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifying_key.to_bytes());
    let digest = hasher.finalize();
    format!("key-{}", encode_hex(&digest[..8]))
}

pub fn sign(signing_key: &SigningKey, message: &[u8]) -> [u8; 64] {
    signing_key.sign(message).to_bytes()
}

pub fn verify(verifying_key: &VerifyingKey, message: &[u8], signature: &[u8; 64]) -> bool {
    let sig = ed25519_dalek::Signature::from_bytes(signature);
    verifying_key.verify(message, &sig).is_ok()
}

pub fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn decode_hex_32(s: &str) -> Option<[u8; 32]> {
    decode_hex(s)?.try_into().ok()
}

pub fn decode_hex_64(s: &str) -> Option<[u8; 64]> {
    decode_hex(s)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir();
        let key = dir.join(format!("buzz-signing-{name}-{:?}.key", std::thread::current().id()));
        let pubkey = dir.join(format!("buzz-signing-{name}-{:?}.pub", std::thread::current().id()));
        let _ = std::fs::remove_file(&key);
        let _ = std::fs::remove_file(&pubkey);
        (key, pubkey)
    }

    #[test]
    fn generates_and_persists_a_keypair_on_first_use() {
        let (key_path, pubkey_path) = temp_paths("generate");
        assert!(!key_path.exists());

        let signing_key = load_or_generate_key_pair(&key_path, &pubkey_path).unwrap();
        assert!(key_path.exists());
        assert!(pubkey_path.exists());

        let verifying_key = load_verifying_key(&pubkey_path).unwrap();
        assert_eq!(verifying_key, signing_key.verifying_key());

        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_file(&pubkey_path);
    }

    #[test]
    fn reloads_the_same_key_on_second_call() {
        let (key_path, pubkey_path) = temp_paths("reload");
        let first = load_or_generate_key_pair(&key_path, &pubkey_path).unwrap();
        let second = load_or_generate_key_pair(&key_path, &pubkey_path).unwrap();
        assert_eq!(first.to_bytes(), second.to_bytes());

        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_file(&pubkey_path);
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let (key_path, pubkey_path) = temp_paths("roundtrip");
        let signing_key = load_or_generate_key_pair(&key_path, &pubkey_path).unwrap();
        let verifying_key = signing_key.verifying_key();

        let message = b"some report bytes";
        let sig = sign(&signing_key, message);
        assert!(verify(&verifying_key, message, &sig));
        assert!(!verify(&verifying_key, b"tampered bytes", &sig));

        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_file(&pubkey_path);
    }

    #[test]
    fn key_id_is_stable_for_the_same_key_and_differs_across_keys() {
        let (key_path_a, pubkey_path_a) = temp_paths("id-a");
        let (key_path_b, pubkey_path_b) = temp_paths("id-b");
        let key_a = load_or_generate_key_pair(&key_path_a, &pubkey_path_a).unwrap();
        let key_b = load_or_generate_key_pair(&key_path_b, &pubkey_path_b).unwrap();

        assert_eq!(key_id(&key_a.verifying_key()), key_id(&key_a.verifying_key()));
        assert_ne!(key_id(&key_a.verifying_key()), key_id(&key_b.verifying_key()));

        let _ = std::fs::remove_file(&key_path_a);
        let _ = std::fs::remove_file(&pubkey_path_a);
        let _ = std::fs::remove_file(&key_path_b);
        let _ = std::fs::remove_file(&pubkey_path_b);
    }
}

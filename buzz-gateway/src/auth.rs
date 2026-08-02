//! Local-only bearer token auth.
//!
//! Design intent from the deck (slide 05): "Local token auth, issued and
//! rotated per machine." This is the whole implementation of that promise —
//! keep it boring and hard to misuse. No JWTs, no expiry math, no network
//! calls. A random token, written to disk with owner-only permissions,
//! checked in constant time.

use rand::RngCore;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;

const TOKEN_BYTES: usize = 32; // 256 bits

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("io error writing token file: {0}")]
    Io(#[from] io::Error),
}

/// Generates a new random token, hex-encoded.
fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Where the token lives. Mirrors buzz-cli's existing ~/.buzz/config.toml
/// convention — TODO: pull the base dir from the same helper buzz-core
/// already uses for config, instead of hardcoding "~/.buzz" resolution here.
pub fn default_token_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".buzz").join("gateway.token")
}

/// Issues a fresh token and writes it to disk with 0600 permissions.
/// Called on every `buzz-cli serve` startup by default (design intent:
/// "issued and rotated per machine" — v1 rotates on every restart, which
/// is the simplest thing that satisfies the claim. A `buzz-cli token
/// rotate` subcommand that rotates without a restart is a v2 nicety, not
/// required for the gateway to ship.)
pub fn issue_and_persist(path: &Path) -> Result<String, AuthError> {
    let token = generate_token();

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, &token)?;

    // Owner read/write only. Non-Unix targets silently skip this —
    // TODO: decide whether Windows support is in scope at all; the deck's
    // threat model (loopback boundary, local code execution) assumes a
    // single-user Unix-like machine.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }

    Ok(token)
}

/// Constant-time token check. Never use `==` on secrets — timing
/// differences on early-exit comparison are a real (if narrow) side
/// channel, and there's no reason to accept that risk when the fix is
/// this cheap.
pub fn verify(expected: &str, provided: &str) -> bool {
    expected.as_bytes().ct_eq(provided.as_bytes()).into()
}

// --- Minimal hex encoding, avoids pulling in the `hex` crate for one fn ---
// TODO: if `hex` is already a transitive dependency elsewhere in the
// workspace, delete this and add `hex = "0.4"` to Cargo.toml instead.
mod hex {
    pub fn encode(bytes: [u8; super::TOKEN_BYTES]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

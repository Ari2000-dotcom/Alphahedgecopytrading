//! Seals the accounts file with AES-256-GCM. The key is derived from a master
//! secret that never touches the disk (ACCOUNTS_KEY, or the engine token).

use crate::error::{Error, Result};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};

pub struct Vault {
    cipher: Aes256Gcm,
}

impl Vault {
    pub fn new(master: &str) -> Self {
        // Domain-separated so the same string used as the API token gives a different key.
        let key = Sha256::digest(format!("kite-engine accounts v1\0{master}").as_bytes());
        Self { cipher: Aes256Gcm::new_from_slice(&key).expect("32-byte key") }
    }

    /// nonce (12 bytes) followed by the ciphertext.
    pub fn seal(&self, plain: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let mut out = nonce.to_vec();
        out.extend(self.cipher.encrypt(Nonce::from_slice(&nonce), plain).expect("encryption cannot fail"));
        out
    }

    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < 12 + 16 {
            return Err(Error::BadResponse("accounts file is too short".into()));
        }
        let (nonce, ct) = sealed.split_at(12);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), ct)
            .map_err(|_| Error::BadResponse("accounts file cannot be opened: wrong ACCOUNTS_KEY / token, or the file is damaged".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_wrong_key() {
        let v = Vault::new("a-long-master-secret");
        let sealed = v.seal(b"hello secrets");
        assert!(!String::from_utf8_lossy(&sealed).contains("hello"));
        assert_eq!(v.open(&sealed).unwrap(), b"hello secrets");
        assert!(Vault::new("another").open(&sealed).is_err());
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(v.open(&bad).is_err());
        assert_ne!(v.seal(b"x"), v.seal(b"x"), "fresh nonce each time");
    }
}

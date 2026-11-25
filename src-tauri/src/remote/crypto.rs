//! End-to-end cryptography for RoALT Remote Dashboard (`remote`).
//!
//! Uses NIST P-256 (ECDH + ECDSA SHA-256), HKDF-SHA256, and AES-256-GCM so every operation
//! matches browser native `SubtleCrypto` byte-for-byte with non-extractable keys in IndexedDB.
//!
//! Wire formats:
//! - Public keys: 65-byte SEC1 uncompressed P-256 (`0x04 || X(32) || Y(32)`).
//! - ECDSA signatures: 64-byte IEEE P1363 raw `r || s` over SHA-256.
//! - Handshake frame (`kind = 0x01`): `[0x01][ephem_pub: 65B][ecdsa_sig: 64B]` (130 bytes).
//! - Encrypted frame (`kind = 0x02`): `[0x02][seq: u64 BE][ciphertext || 16B GCM tag]`.
//!   - Nonce (12 bytes): `[dir: 1B][0, 0, 0][seq: u64 BE]` (`dir = 0x01` PC->Viewer, `0x02` Viewer->PC).
//!   - AAD (10 bytes): `[0x01][dir: 1B][seq: u64 BE]`.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hkdf::Hkdf;
use p256::ecdh::diffie_hellman;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::error::{AppError, Result};
use crate::vault::master::random_bytes;

pub const FRAME_HANDSHAKE: u8 = 0x01;
pub const FRAME_DATA: u8 = 0x02;
pub const DIR_PC_TO_VIEWER: u8 = 0x01;
pub const DIR_VIEWER_TO_PC: u8 = 0x02;

const HANDSHAKE_DOMAIN: &[u8] = b"roalt-remote-v1-hs";
const SESSION_INFO: &[u8] = b"roalt-remote-v1-session";
const QR_PAIR_INFO: &[u8] = b"roalt-remote-v2-pair-qr";
const MANUAL_PAIR_INFO: &[u8] = b"roalt-remote-v2-manual";
const MANUAL_SAS_INFO: &[u8] = b"roalt-remote-v2-sas";
const PAIR_TRANSCRIPT_DOMAIN: &[u8] = b"roalt-remote-v2-pair";
const PAIR_COMMIT_DOMAIN: &[u8] = b"roalt-remote-v2-commit";
const PAIR_BLOB_DOMAIN: &[u8] = b"roalt-remote-v2-blob";
const DEVICE_BIND_DOMAIN: &[u8] = b"roalt-remote-v2-device";
const PC_BIND_DOMAIN: &[u8] = b"roalt-remote-v2-pc";
const RELAY_SECRET_SALT: &[u8] = b"roalt-remote-v2-relay-secret";

pub const PAIR_MODE_QR: u8 = 0x01;
pub const PAIR_MODE_MANUAL: u8 = 0x02;

const MANUAL_ALPHABET: &[u8; 31] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

/// Persistent PC identity stored in the SQLCipher-encrypted database.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PcIdentity {
    pub pc_id: Uuid,
    /// Random 32-byte secret (base64url) whose SHA-256 authenticates the PC to the relay.
    pub pc_secret: String,
    /// 32-byte raw P-256 static private scalar (base64url).
    pub static_priv_b64: String,
}

impl PcIdentity {
    pub fn generate() -> Self {
        let mut priv_bytes = Zeroizing::new(random_bytes::<32>());
        // Retry until valid non-zero scalar < curve order (virtually always 1 try).
        let sk = loop {
            if let Ok(sk) = SecretKey::from_slice(&*priv_bytes) {
                break sk;
            }
            *priv_bytes = random_bytes::<32>();
        };
        let raw_priv = Zeroizing::new(sk.to_bytes());
        let pc_secret = URL_SAFE_NO_PAD.encode(random_bytes::<32>());
        Self {
            pc_id: pc_id_for_secret_hash(&sha256_hex(&pc_secret)),
            pc_secret,
            static_priv_b64: URL_SAFE_NO_PAD.encode(raw_priv.as_slice()),
        }
    }

    pub fn pc_secret_hash_hex(&self) -> String {
        sha256_hex(&self.pc_secret)
    }

    /// The identity this PC shows `relay_url`. The official relay gets the stored secret; any other
    /// relay gets one derived for that URL alone (and so its own pcId), so what a custom relay
    /// learns can't be replayed to take this PC's place on the official relay.
    pub fn for_relay(&self, relay_url: &str, official: bool) -> Result<Self> {
        if official {
            return Ok(self.clone());
        }
        let hk = Hkdf::<Sha256>::new(Some(RELAY_SECRET_SALT), self.pc_secret.as_bytes());
        let mut okm = Zeroizing::new([0u8; 32]);
        hk.expand(relay_url.as_bytes(), okm.as_mut()).map_err(|_| AppError::Crypto)?;
        let pc_secret = URL_SAFE_NO_PAD.encode(okm.as_slice());
        Ok(Self { pc_id: pc_id_for_secret_hash(&sha256_hex(&pc_secret)), pc_secret, static_priv_b64: self.static_priv_b64.clone() })
    }

    pub fn signing_key(&self) -> Result<SigningKey> {
        let raw = Zeroizing::new(URL_SAFE_NO_PAD.decode(&self.static_priv_b64).map_err(|_| AppError::Crypto)?);
        SigningKey::from_slice(&raw).map_err(|_| AppError::Crypto)
    }

    /// 65-byte SEC1 uncompressed public key.
    pub fn public_key_bytes(&self) -> Result<Vec<u8>> {
        let sk = self.signing_key()?;
        Ok(sk.verifying_key().to_encoded_point(false).as_bytes().to_vec())
    }
}

/// The pcId the relay accepts for a secret hash: the first 16 bytes of its SHA-256 (the relay's
/// `pc_id_for_secret_hash`), so a pcId alone can't claim the PC's room.
pub fn pc_id_for_secret_hash(secret_hash_hex: &str) -> Uuid {
    let d = Sha256::digest(secret_hash_hex.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    Uuid::from_bytes(b)
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

pub fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

pub fn random_pair_id() -> String {
    hex::encode(random_bytes::<8>())
}

/// 8-character human-friendly manual code formatted `ABCD-1234`.
pub fn random_manual_code() -> String {
    let mut out = String::with_capacity(9);
    let mut count = 0;
    while count < 8 {
        let [b] = random_bytes::<1>();
        if b < 248 {
            if count == 4 {
                out.push('-');
            }
            out.push(MANUAL_ALPHABET[(b % 31) as usize] as char);
            count += 1;
        }
    }
    out
}

pub fn new_ephemeral_key() -> SecretKey {
    let mut raw = Zeroizing::new(random_bytes::<32>());
    loop {
        if let Ok(sk) = SecretKey::from_slice(&*raw) {
            return sk;
        }
        *raw = random_bytes::<32>();
    }
}

pub fn pubkey_bytes(sk: &SecretKey) -> [u8; 65] {
    let pt = sk.public_key().to_encoded_point(false);
    let mut out = [0u8; 65];
    out.copy_from_slice(pt.as_bytes());
    out
}

pub fn parse_pubkey(bytes: &[u8]) -> Result<PublicKey> {
    if bytes.len() != 65 || bytes[0] != 0x04 {
        return Err(AppError::Crypto);
    }
    PublicKey::from_sec1_bytes(bytes).map_err(|_| AppError::Crypto)
}

fn handshake_transcript(role: u8, pc_id: Uuid, device_id: Uuid, ephem_pub: &[u8; 65]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(HANDSHAKE_DOMAIN.len() + 1 + 16 + 16 + 65);
    msg.extend_from_slice(HANDSHAKE_DOMAIN);
    msg.push(role);
    msg.extend_from_slice(pc_id.as_bytes());
    msg.extend_from_slice(device_id.as_bytes());
    msg.extend_from_slice(ephem_pub);
    msg
}

/// Builds a 130-byte handshake frame: `[0x01][ephem_pub: 65B][ecdsa_sig: 64B]`.
pub fn build_handshake(
    static_signing: &SigningKey,
    ephem_sk: &SecretKey,
    role: u8,
    pc_id: Uuid,
    device_id: Uuid,
) -> Vec<u8> {
    let ephem_pub = pubkey_bytes(ephem_sk);
    let transcript = handshake_transcript(role, pc_id, device_id, &ephem_pub);
    let sig: Signature = static_signing.sign(&transcript);
    let mut out = Vec::with_capacity(130);
    out.push(FRAME_HANDSHAKE);
    out.extend_from_slice(&ephem_pub);
    out.extend_from_slice(&sig.to_bytes());
    out
}

/// Verifies a peer's 130-byte handshake frame and returns its ephemeral public key (65 bytes).
pub fn verify_handshake(
    peer_static_pub_bytes: &[u8],
    frame: &[u8],
    expected_peer_role: u8,
    pc_id: Uuid,
    device_id: Uuid,
) -> Result<[u8; 65]> {
    if frame.len() != 130 || frame[0] != FRAME_HANDSHAKE {
        return Err(AppError::Crypto);
    }
    let peer_vk = VerifyingKey::from_sec1_bytes(peer_static_pub_bytes).map_err(|_| AppError::Crypto)?;
    let mut ephem_pub = [0u8; 65];
    ephem_pub.copy_from_slice(&frame[1..66]);
    // Validate point is on curve.
    let _ = parse_pubkey(&ephem_pub)?;
    let sig = Signature::from_slice(&frame[66..130]).map_err(|_| AppError::Crypto)?;
    let transcript = handshake_transcript(expected_peer_role, pc_id, device_id, &ephem_pub);
    peer_vk.verify(&transcript, &sig).map_err(|_| AppError::Crypto)?;
    Ok(ephem_pub)
}

/// Bidirectional AES-256-GCM session keys with replay-proof monotonic counters.
pub struct SessionCipher {
    tx_key: Zeroizing<[u8; 32]>,
    rx_key: Zeroizing<[u8; 32]>,
    tx_dir: u8,
    rx_dir: u8,
    tx_seq: u64,
    rx_seq_next: u64,
}

impl SessionCipher {
    /// Derives session keys from ephemeral ECDH.
    /// `is_pc = true`: PC sends with `DIR_PC_TO_VIEWER`, receives with `DIR_VIEWER_TO_PC`.
    pub fn derive(
        my_ephem_sk: &SecretKey,
        peer_ephem_pub: &[u8; 65],
        pc_ephem_pub: &[u8; 65],
        viewer_ephem_pub: &[u8; 65],
        is_pc: bool,
    ) -> Result<Self> {
        let peer_pk = parse_pubkey(peer_ephem_pub)?;
        let shared = diffie_hellman(my_ephem_sk.to_nonzero_scalar(), peer_pk.as_affine());
        let mut salt = [0u8; 130];
        salt[..65].copy_from_slice(pc_ephem_pub);
        salt[65..].copy_from_slice(viewer_ephem_pub);
        let hk = Hkdf::<Sha256>::new(Some(&salt), shared.raw_secret_bytes().as_slice());
        let mut okm = Zeroizing::new([0u8; 64]);
        hk.expand(SESSION_INFO, okm.as_mut()).map_err(|_| AppError::Crypto)?;
        let mut k_pc = Zeroizing::new([0u8; 32]);
        let mut k_viewer = Zeroizing::new([0u8; 32]);
        k_pc.copy_from_slice(&okm[..32]);
        k_viewer.copy_from_slice(&okm[32..]);

        if is_pc {
            Ok(Self {
                tx_key: k_pc,
                rx_key: k_viewer,
                tx_dir: DIR_PC_TO_VIEWER,
                rx_dir: DIR_VIEWER_TO_PC,
                tx_seq: 1,
                rx_seq_next: 1,
            })
        } else {
            Ok(Self {
                tx_key: k_viewer,
                rx_key: k_pc,
                tx_dir: DIR_VIEWER_TO_PC,
                rx_dir: DIR_PC_TO_VIEWER,
                tx_seq: 1,
                rx_seq_next: 1,
            })
        }
    }

    fn nonce_and_aad(dir: u8, seq: u64) -> ([u8; 12], [u8; 10]) {
        let seq_be = seq.to_be_bytes();
        let mut nonce = [0u8; 12];
        nonce[0] = dir;
        nonce[4..12].copy_from_slice(&seq_be);
        let mut aad = [0u8; 10];
        aad[0] = 0x01;
        aad[1] = dir;
        aad[2..10].copy_from_slice(&seq_be);
        (nonce, aad)
    }

    /// Encrypts `plaintext` into `[0x02][seq: u64 BE][ciphertext || 16B tag]`.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let seq = self.tx_seq;
        self.tx_seq = self.tx_seq.checked_add(1).ok_or(AppError::Crypto)?;
        let (nonce, aad) = Self::nonce_and_aad(self.tx_dir, seq);
        let ct = Aes256Gcm::new((&*self.tx_key).into())
            .encrypt(&Nonce::from(nonce), Payload { msg: plaintext, aad: &aad })
            .map_err(|_| AppError::Crypto)?;
        let mut out = Vec::with_capacity(9 + ct.len());
        out.push(FRAME_DATA);
        out.extend_from_slice(&seq.to_be_bytes());
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Decrypts `[0x02][seq: u64 BE][ciphertext || 16B tag]` enforcing strictly increasing `seq`.
    pub fn open(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        if frame.len() < 9 + 16 || frame[0] != FRAME_DATA {
            return Err(AppError::Crypto);
        }
        let seq = u64::from_be_bytes(frame[1..9].try_into().map_err(|_| AppError::Crypto)?);
        if seq < self.rx_seq_next {
            return Err(AppError::Crypto);
        }
        let (nonce, aad) = Self::nonce_and_aad(self.rx_dir, seq);
        let pt = Aes256Gcm::new((&*self.rx_key).into())
            .decrypt(&Nonce::from(nonce), Payload { msg: &frame[9..], aad: &aad })
            .map_err(|_| AppError::Crypto)?;
        self.rx_seq_next = seq.checked_add(1).ok_or(AppError::Crypto)?;
        Ok(pt)
    }
}

/// Derives the 32-byte AES-256-GCM key for QR-code pairing from `(pair_secret, pair_id)`.
pub fn derive_qr_pair_key(pair_secret: &[u8], pair_id: &str) -> Result<Zeroizing<[u8; 32]>> {
    let hk = Hkdf::<Sha256>::new(Some(pair_id.as_bytes()), pair_secret);
    let mut key = Zeroizing::new([0u8; 32]);
    hk.expand(QR_PAIR_INFO, key.as_mut()).map_err(|_| AppError::Crypto)?;
    Ok(key)
}

/// Pairing transcript hash: `SHA-256("roalt-remote-v2-pair" || mode || len(pair_id): u8 || pair_id
/// || pc_ephem || browser_ephem || nonce(32))`. QR pairing has no ephemerals (both empty).
pub fn pair_transcript(mode: u8, pair_id: &str, pc_ephem: &[u8], br_ephem: &[u8], nonce: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(PAIR_TRANSCRIPT_DOMAIN);
    h.update([mode, pair_id.len() as u8]);
    h.update(pair_id.as_bytes());
    h.update(pc_ephem);
    h.update(br_ephem);
    h.update(nonce);
    h.finalize().into()
}

/// The browser's manual-pairing commitment, sent before it sees the PC's ephemeral key:
/// `SHA-256("roalt-remote-v2-commit" || browser_ephem(65) || nonce(32))`.
pub fn pair_commit(br_ephem: &[u8; 65], nonce: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(PAIR_COMMIT_DOMAIN);
    h.update(br_ephem);
    h.update(nonce);
    h.finalize().into()
}

/// Derives `(pair_key, 6-digit SAS "123 456")` for manual-code ECDH pairing, salted with the transcript.
pub fn derive_manual_pair_keys(
    my_ephem_sk: &SecretKey,
    peer_ephem_pub: &[u8; 65],
    transcript: &[u8; 32],
) -> Result<(Zeroizing<[u8; 32]>, String)> {
    let peer_pk = parse_pubkey(peer_ephem_pub)?;
    let shared = diffie_hellman(my_ephem_sk.to_nonzero_scalar(), peer_pk.as_affine());
    let hk = Hkdf::<Sha256>::new(Some(transcript), shared.raw_secret_bytes().as_slice());
    let mut key = Zeroizing::new([0u8; 32]);
    hk.expand(MANUAL_PAIR_INFO, key.as_mut()).map_err(|_| AppError::Crypto)?;
    let mut sas_bytes = [0u8; 4];
    hk.expand(MANUAL_SAS_INFO, &mut sas_bytes).map_err(|_| AppError::Crypto)?;
    let code = u32::from_be_bytes(sas_bytes) % 1_000_000;
    let sas = format!("{:03} {:03}", code / 1000, code % 1000);
    Ok((key, sas))
}

fn pair_blob_aad(dir: u8, pair_id: &str) -> Vec<u8> {
    [PAIR_BLOB_DOMAIN, &[dir], pair_id.as_bytes()].concat()
}

/// Encrypts a pairing JSON payload as `base64url(nonce(12) || ciphertext+tag)` with
/// AAD = `"roalt-remote-v2-blob" || dir || pair_id`, so a blob can't be reflected back to its sender.
pub fn seal_pair_blob(key: &[u8; 32], dir: u8, pair_id: &str, plaintext: &[u8]) -> Result<String> {
    let nonce = random_bytes::<12>();
    let aad = pair_blob_aad(dir, pair_id);
    let ct = Aes256Gcm::new(key.into())
        .encrypt(&Nonce::from(nonce), Payload { msg: plaintext, aad: &aad })
        .map_err(|_| AppError::Crypto)?;
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(URL_SAFE_NO_PAD.encode(&out))
}

/// Inverse of `seal_pair_blob`.
pub fn open_pair_blob(key: &[u8; 32], dir: u8, pair_id: &str, b64: &str) -> Result<Vec<u8>> {
    let blob = URL_SAFE_NO_PAD.decode(b64).map_err(|_| AppError::Crypto)?;
    if blob.len() < 12 + 16 {
        return Err(AppError::Crypto);
    }
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&blob[..12]);
    let aad = pair_blob_aad(dir, pair_id);
    Aes256Gcm::new(key.into())
        .decrypt(&Nonce::from(nonce), Payload { msg: &blob[12..], aad: &aad })
        .map_err(|_| AppError::Crypto)
}

fn device_bind_message(transcript: &[u8; 32], device_id: Uuid, device_pub: &[u8]) -> Vec<u8> {
    [DEVICE_BIND_DOMAIN, transcript, device_id.as_bytes(), device_pub].concat()
}

fn pc_bind_message(transcript: &[u8; 32], pc_id: Uuid, pc_pub: &[u8], device_id: Uuid) -> Vec<u8> {
    [PC_BIND_DOMAIN, transcript, pc_id.as_bytes(), pc_pub, device_id.as_bytes()].concat()
}

/// Checks the browser's proof that it holds `device_pub`'s private key for this pairing:
/// ECDSA over `"roalt-remote-v2-device" || transcript || device_id(16) || device_pub(65)`.
pub fn verify_device_binding(device_pub: &[u8], transcript: &[u8; 32], device_id: Uuid, sig: &[u8]) -> Result<()> {
    let vk = VerifyingKey::from_sec1_bytes(device_pub).map_err(|_| AppError::Crypto)?;
    let sig = Signature::from_slice(sig).map_err(|_| AppError::Crypto)?;
    vk.verify(&device_bind_message(transcript, device_id, device_pub), &sig).map_err(|_| AppError::Crypto)
}

#[cfg(test)]
pub fn sign_device_binding(device: &SigningKey, transcript: &[u8; 32], device_id: Uuid, device_pub: &[u8]) -> Vec<u8> {
    let sig: Signature = device.sign(&device_bind_message(transcript, device_id, device_pub));
    sig.to_bytes().to_vec()
}

/// The PC's 64-byte signature binding its static key to this pairing:
/// ECDSA over `"roalt-remote-v2-pc" || transcript || pc_id(16) || pc_pub(65) || device_id(16)`.
pub fn sign_pc_binding(signing: &SigningKey, transcript: &[u8; 32], pc_id: Uuid, device_id: Uuid) -> Vec<u8> {
    let pc_pub = signing.verifying_key().to_encoded_point(false);
    let sig: Signature = signing.sign(&pc_bind_message(transcript, pc_id, pc_pub.as_bytes(), device_id));
    sig.to_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pc_id_matches_relay_rule() {
        // Same vector as the relay's `pc_id_rule_vector` test (server/remote-relay).
        assert_eq!(pc_id_for_secret_hash(&sha256_hex("pc-secret")).to_string(), "d5d6cc8b-7d41-ca38-749e-6786be4d1849");
        let id = PcIdentity::generate();
        assert_eq!(id.pc_id, pc_id_for_secret_hash(&id.pc_secret_hash_hex()));
    }

    #[test]
    fn each_custom_relay_gets_its_own_secret_and_pc_id() {
        let id = PcIdentity::generate();
        let official = id.for_relay("wss://remote.roalt.dev/ws/remote", true).unwrap();
        assert_eq!((official.pc_id, official.pc_secret_hash_hex()), (id.pc_id, id.pc_secret_hash_hex()));
        let a = id.for_relay("wss://a.example/ws", false).unwrap();
        let b = id.for_relay("wss://b.example/ws", false).unwrap();
        assert_ne!(a.pc_secret_hash_hex(), id.pc_secret_hash_hex());
        assert_ne!(a.pc_secret_hash_hex(), b.pc_secret_hash_hex());
        assert_eq!(a.pc_id, pc_id_for_secret_hash(&a.pc_secret_hash_hex()));
        assert_eq!(a.pc_secret_hash_hex(), id.for_relay("wss://a.example/ws", false).unwrap().pc_secret_hash_hex());
        assert_eq!(a.public_key_bytes().unwrap(), id.public_key_bytes().unwrap());
    }

    #[test]
    fn handshake_session_roundtrip_and_replay_rejection() {
        let pc = PcIdentity::generate();
        let viewer = PcIdentity::generate();
        let pc_id = pc.pc_id;
        let device_id = Uuid::new_v4();

        let pc_ephem = new_ephemeral_key();
        let viewer_ephem = new_ephemeral_key();

        let pc_hs = build_handshake(&pc.signing_key().unwrap(), &pc_ephem, DIR_PC_TO_VIEWER, pc_id, device_id);
        let viewer_hs = build_handshake(&viewer.signing_key().unwrap(), &viewer_ephem, DIR_VIEWER_TO_PC, pc_id, device_id);

        let pc_pub_from_hs = verify_handshake(&pc.public_key_bytes().unwrap(), &pc_hs, DIR_PC_TO_VIEWER, pc_id, device_id).unwrap();
        let viewer_pub_from_hs = verify_handshake(&viewer.public_key_bytes().unwrap(), &viewer_hs, DIR_VIEWER_TO_PC, pc_id, device_id).unwrap();

        // Wrong peer key or wrong role must fail.
        assert!(verify_handshake(&viewer.public_key_bytes().unwrap(), &pc_hs, DIR_PC_TO_VIEWER, pc_id, device_id).is_err());
        assert!(verify_handshake(&pc.public_key_bytes().unwrap(), &pc_hs, DIR_VIEWER_TO_PC, pc_id, device_id).is_err());

        let mut pc_cipher = SessionCipher::derive(&pc_ephem, &viewer_pub_from_hs, &pc_pub_from_hs, &viewer_pub_from_hs, true).unwrap();
        let mut viewer_cipher = SessionCipher::derive(&viewer_ephem, &pc_pub_from_hs, &pc_pub_from_hs, &viewer_pub_from_hs, false).unwrap();

        // PC -> Viewer
        let f1 = pc_cipher.seal(b"hello viewer").unwrap();
        assert_eq!(viewer_cipher.open(&f1).unwrap(), b"hello viewer");
        // Replay must fail!
        assert!(viewer_cipher.open(&f1).is_err());

        // Tampered frame must fail!
        let mut f2 = pc_cipher.seal(b"second").unwrap();
        let last = f2.len() - 1;
        f2[last] ^= 0xFF;
        assert!(viewer_cipher.open(&f2).is_err());

        // Viewer -> PC
        let r1 = viewer_cipher.seal(b"launch alts").unwrap();
        assert_eq!(pc_cipher.open(&r1).unwrap(), b"launch alts");
    }

    #[test]
    fn qr_and_manual_pairing_crypto_match() {
        let pair_id = random_pair_id();
        let secret = random_bytes::<32>();
        let k = derive_qr_pair_key(&secret, &pair_id).unwrap();
        let blob = seal_pair_blob(&k, DIR_VIEWER_TO_PC, &pair_id, br#"{"deviceId":"abc"}"#).unwrap();
        assert_eq!(open_pair_blob(&k, DIR_VIEWER_TO_PC, &pair_id, &blob).unwrap(), br#"{"deviceId":"abc"}"#);
        assert!(open_pair_blob(&k, DIR_VIEWER_TO_PC, "wrong-id", &blob).is_err());
        assert!(open_pair_blob(&k, DIR_PC_TO_VIEWER, &pair_id, &blob).is_err(), "a join must not open as an ack");

        let pc_e = new_ephemeral_key();
        let br_e = new_ephemeral_key();
        let nonce = random_bytes::<32>();
        let th = pair_transcript(PAIR_MODE_MANUAL, &pair_id, &pubkey_bytes(&pc_e), &pubkey_bytes(&br_e), &nonce);
        let (k_pc, sas_pc) = derive_manual_pair_keys(&pc_e, &pubkey_bytes(&br_e), &th).unwrap();
        let (k_br, sas_br) = derive_manual_pair_keys(&br_e, &pubkey_bytes(&pc_e), &th).unwrap();
        assert_eq!(*k_pc, *k_br);
        assert_eq!(sas_pc, sas_br);
        assert_eq!(sas_pc.len(), 7);

        let other = pair_transcript(PAIR_MODE_MANUAL, &pair_id, &pubkey_bytes(&pc_e), &pubkey_bytes(&br_e), &random_bytes::<32>());
        assert_ne!(*derive_manual_pair_keys(&pc_e, &pubkey_bytes(&br_e), &other).unwrap().0, *k_pc);
        assert_ne!(pair_transcript(PAIR_MODE_QR, &pair_id, &[], &[], &nonce), th);
    }

    #[test]
    fn pairing_bindings_verify_only_their_transcript() {
        let pc = PcIdentity::generate();
        let device = SigningKey::from_slice(&[0x66u8; 32]).unwrap();
        let device_pub = device.verifying_key().to_encoded_point(false).as_bytes().to_vec();
        let device_id = Uuid::new_v4();
        let th = pair_transcript(PAIR_MODE_QR, "0123456789abcdef", &[], &[], &[7u8; 32]);
        let other = pair_transcript(PAIR_MODE_QR, "0123456789abcdef", &[], &[], &[8u8; 32]);

        let sig: Signature = device.sign(&device_bind_message(&th, device_id, &device_pub));
        assert!(verify_device_binding(&device_pub, &th, device_id, &sig.to_bytes()).is_ok());
        assert!(verify_device_binding(&device_pub, &other, device_id, &sig.to_bytes()).is_err());
        assert!(verify_device_binding(&device_pub, &th, Uuid::new_v4(), &sig.to_bytes()).is_err());
        assert!(verify_device_binding(&pc.public_key_bytes().unwrap(), &th, device_id, &sig.to_bytes()).is_err());

        let pc_sig = sign_pc_binding(&pc.signing_key().unwrap(), &th, pc.pc_id, device_id);
        let vk = VerifyingKey::from_sec1_bytes(&pc.public_key_bytes().unwrap()).unwrap();
        let msg = pc_bind_message(&th, pc.pc_id, &pc.public_key_bytes().unwrap(), device_id);
        assert!(vk.verify(&msg, &Signature::from_slice(&pc_sig).unwrap()).is_ok());
    }

    #[test]
    fn manual_code_format() {
        let code = random_manual_code();
        assert_eq!(code.len(), 9);
        assert_eq!(code.as_bytes()[4], b'-');
    }

    const INTEROP_PC_STATIC_PUB: &str = "BAIX5hfwtkQ5KCePlpmeaaI6TywVK99tbN9m5bgCgtTtGUp968uXcS0t2jyoWqh2Wlb0X8dYWZZS8ol8ZTBuV5Q";
    const INTEROP_PC_HS: &str = "AQTWWpOXfKo9GwgYUv9Xp55GXxZgV3MEuurVBd06SFic81AYXolTct9iIeo6E3VX5HP922dV8FvVB8PFM_zpyRKFh0WGmYWxHxuvWsRuSPySACGb2ySSuApgzYbOyrZGRdpz00GFgdJ9mL1I6jwQv3Bn9-cPw7Ne4_CSbXkqu0byTg";
    const INTEROP_VIEWER_EPHEM_PUB: &str = "BFGnWAgziY6hsYPL1zUKQJkHjG7xweGOlwzXaDA18l59ARBSJxKwtafP8IFoVIaYSpTmgx7axG5zYPqdg0p6gaE";
    const INTEROP_FRAME: &str = "AgAAAAAAAAABGz9lADf3exgLBceQ8mcZIQplYmM_OlVj3SLHXgD2QA";
    const INTEROP_QR_BLOB: &str = "JtM7ujKBI809qllUdXgVYlyGrSBpY-Ik3XLmuNgACXTl";
    const INTEROP_MANUAL_BLOB: &str = "nxrFIADmScVoH9RN6lyVp6uh0YdEFM3n22DC05R5Yta2xxVc_A";
    const INTEROP_PC_BIND_SIG: &str = "lgHJXVPUMByxy8ADSyM9a0J4dJPNY4ql_oeg7HjMppZliQ-9qae_rc-O9z1uSYWjyi-GzLYEGPe3WStjqTXMSg";
    const INTEROP_COMMIT: &str = "e179054dbf57cebe74ee5be8ed103a7f47072eedb5e3e036475be4ba0d6c998f";
    const INTEROP_QR_TRANSCRIPT: &str = "76d48323f8391c641079a1d56f6113e1de94ccd31ae6b2c1042a5f5b52478513";
    const INTEROP_MANUAL_TRANSCRIPT: &str = "579f983dd0316b473126e5622174c23bb57919fdee3a740b863c8949ad8cc281";
    const INTEROP_SAS: &str = "788 711";
    const WEB_DEVICE_PUB: &str = "BBec6s4dd5vbaoX71zsuDENxCZE5RZPn8rRA4ZYmjlwTS2nbg9daAIOyTgO-9o6ZjZ8sbxk0BgUz0nLr0MlhKrw";
    const WEB_DEVICE_SIG: &str = "p28FIx4nF0_p4xyPOY1e2S5TdogJQLnpmKMU4B0itVOYcOeJ1McD5GBBcSBUIc8wq42bS_rIXx749FMe83CMnA";
    const WEB_MANUAL_BLOB: &str = "XDlLyVJvYt_mSIR6B6r8X31Lsj5CFq9lcZco-4oydVhuQg";

    #[test]
    fn deterministic_webcrypto_interop_vector() {
        let pc_static_sk = SigningKey::from_slice(&[0x11u8; 32]).unwrap();
        let pc_ephem = SecretKey::from_slice(&[0x22u8; 32]).unwrap();
        let viewer_ephem = SecretKey::from_slice(&[0x33u8; 32]).unwrap();
        let pc_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
        let device_id = Uuid::parse_str("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap();

        let b64 = |bytes: &[u8]| URL_SAFE_NO_PAD.encode(bytes);
        let pc_static_pub = pc_static_sk.verifying_key().to_encoded_point(false).as_bytes().to_vec();
        assert_eq!(b64(&pc_static_pub), INTEROP_PC_STATIC_PUB);
        let pc_hs = build_handshake(&pc_static_sk, &pc_ephem, DIR_PC_TO_VIEWER, pc_id, device_id);
        assert_eq!(b64(&pc_hs), INTEROP_PC_HS);
        let pc_ephem_pub = pubkey_bytes(&pc_ephem);
        let viewer_ephem_pub = pubkey_bytes(&viewer_ephem);
        assert_eq!(b64(&viewer_ephem_pub), INTEROP_VIEWER_EPHEM_PUB);

        let mut pc_cipher = SessionCipher::derive(&pc_ephem, &viewer_ephem_pub, &pc_ephem_pub, &viewer_ephem_pub, true).unwrap();
        assert_eq!(b64(&pc_cipher.seal(b"hello from rust").unwrap()), INTEROP_FRAME);

        let pair_id = "0123456789abcdef";
        let nonce = [0x55u8; 32];
        assert_eq!(hex::encode(pair_commit(&viewer_ephem_pub, &nonce)), INTEROP_COMMIT);
        let qr_th = pair_transcript(PAIR_MODE_QR, pair_id, &[], &[], &nonce);
        assert_eq!(hex::encode(qr_th), INTEROP_QR_TRANSCRIPT);
        let manual_th = pair_transcript(PAIR_MODE_MANUAL, pair_id, &pc_ephem_pub, &viewer_ephem_pub, &nonce);
        assert_eq!(hex::encode(manual_th), INTEROP_MANUAL_TRANSCRIPT);
        let (manual_key, sas) = derive_manual_pair_keys(&pc_ephem, &viewer_ephem_pub, &manual_th).unwrap();
        assert_eq!(sas, INTEROP_SAS);
        assert_eq!(open_pair_blob(&manual_key, DIR_PC_TO_VIEWER, pair_id, INTEROP_MANUAL_BLOB).unwrap(), b"manual-ok");
        let qr_key = derive_qr_pair_key(&[0x44u8; 32], pair_id).unwrap();
        assert_eq!(open_pair_blob(&qr_key, DIR_PC_TO_VIEWER, pair_id, INTEROP_QR_BLOB).unwrap(), b"qr-ok");
        assert_eq!(b64(&sign_pc_binding(&pc_static_sk, &manual_th, pc_id, device_id)), INTEROP_PC_BIND_SIG);

        let web_device_pub = URL_SAFE_NO_PAD.decode(WEB_DEVICE_PUB).unwrap();
        let web_device_sig = URL_SAFE_NO_PAD.decode(WEB_DEVICE_SIG).unwrap();
        assert!(verify_device_binding(&web_device_pub, &manual_th, device_id, &web_device_sig).is_ok());
        let web_join = open_pair_blob(&manual_key, DIR_VIEWER_TO_PC, pair_id, WEB_MANUAL_BLOB).unwrap();
        assert_eq!(web_join, b"web-ok");
    }
}

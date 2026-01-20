use anyhow::{Context, Result};
use argon2::{
    password_hash::rand_core::{OsRng, RngCore},
    Argon2, Params,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use zeroize::Zeroizing;

// Argon2 Recommended Parameters (OWASP)
// m=memory (KiB), t=iterations, p=parallelism
const ARGON2_M_COST: u32 = 65536; // 64 MiB
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 4;

/// パスフレーズとソルトから暗号化キーを導出する
pub fn derive_key(passphrase: &str, salt_b64: &str) -> Result<Zeroizing<[u8; 32]>> {
    // Saltのデコード (APIからはBase64で渡される)
    let salt_bytes = BASE64
        .decode(salt_b64)
        .context("Failed to decode salt from Base64")?;

    // Argon2idの設定
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(32))
        .map_err(|e| anyhow::anyhow!("Invalid Argon2 params: {}", e))?;

    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

    // 鍵生成 (32 bytes output)
    let mut key = [0u8; 32];

    // SaltはAPIからBase64で渡されるが、Argon2にはバイト列として直接渡す。
    // SaltString変換やPHC文字列パースを回避し、直接出力バッファに書き込む (hash_password_into)。
    argon2
        .hash_password_into(passphrase.as_bytes(), &salt_bytes, &mut key)
        .map_err(|e| anyhow::anyhow!("Failed to hash password: {}", e))?;

    Ok(Zeroizing::new(key))
}

/// 非同期版の鍵導出 (UIスレッドをブロックしない)
pub async fn derive_key_async(passphrase: String, salt_b64: String) -> Result<Zeroizing<[u8; 32]>> {
    tokio::task::spawn_blocking(move || derive_key(&passphrase, &salt_b64))
        .await
        .context("Crypto task panicked")?
}

/// 暗号化 (Payload = Nonce + Ciphertext)
pub fn encrypt(content: &str, key: &[u8; 32]) -> Result<String> {
    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng); // 96-bits; unique per message

    let ciphertext = cipher
        .encrypt(&nonce, content.as_bytes())
        .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

    // Nonce + Ciphertext を結合
    let mut payload = nonce.to_vec();
    payload.extend_from_slice(&ciphertext);

    // Base64 Encode
    Ok(BASE64.encode(payload))
}

/// 復号化
pub fn decrypt(payload_b64: &str, key: &[u8; 32]) -> Result<String> {
    let payload = BASE64
        .decode(payload_b64)
        .context("Failed to decode payload from Base64")?;

    if payload.len() < 12 {
        return Err(anyhow::anyhow!("Payload too short (missing nonce)"));
    }

    let (nonce_bytes, ciphertext) = payload.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = ChaCha20Poly1305::new(key.into());

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("Decryption failed (invalid key or corrupted data): {}", e))?;

    let content = String::from_utf8(plaintext).context("Decrypted content is not valid UTF-8")?;

    Ok(content)
}

/// ランダムなソルト(16バイト)を生成しBase64エンコードして返す
pub fn generate_salt() -> String {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    BASE64.encode(salt)
}

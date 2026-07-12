use std::path::Path;

use anyhow::Context;
use iroh::SecretKey;

/// Load an Iroh `SecretKey` from a file at the given path.
///
/// If the file exists, reads the 32-byte secret key from it.
/// If the file does not exist, generates a new key and persists it
/// so that the endpoint's identity is stable across restarts.
pub fn load_secret_key(path: &str) -> anyhow::Result<SecretKey> {
  let path = Path::new(path);
  if path.exists() {
    let bytes = std::fs::read(path).with_context(|| {
      format!("Failed to read secret key at {path:?}")
    })?;
    let arr: [u8; 32] =
      bytes.as_slice().try_into().map_err(|_| {
        anyhow::anyhow!("Secret key file must be exactly 32 bytes")
      })?;
    Ok(SecretKey::from_bytes(&arr))
  } else {
    let key = SecretKey::generate();
    save_secret_key(&key, path)?;
    Ok(key)
  }
}

/// Persist an Iroh `SecretKey` to disk as 32 raw bytes.
pub fn save_secret_key(
  key: &SecretKey,
  path: &Path,
) -> anyhow::Result<()> {
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent)?;
  }
  std::fs::write(path, key.to_bytes())?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_secret_key_persistence() {
    let path = std::env::temp_dir()
      .join(format!("test_iroh_secret_{}.key", std::process::id()));
    let _ = std::fs::remove_file(&path);

    // First call: file doesn't exist → generate + save
    let key1 = load_secret_key(path.to_str().unwrap()).unwrap();
    // Second call: file exists → load
    let key2 = load_secret_key(path.to_str().unwrap()).unwrap();
    assert_eq!(key1.to_bytes(), key2.to_bytes());

    let _ = std::fs::remove_file(&path);
  }
}

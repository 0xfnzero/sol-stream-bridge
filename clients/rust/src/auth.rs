//! Validation shared by server and clients. Keys are never included in errors.
pub fn validate_api_key(key: &str) -> std::io::Result<()> {
    if key.len() > 256 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "API key must contain at most 256 visible ASCII bytes",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_are_optional_bounded_and_safe_for_metadata() {
        for key in ["".to_owned(), "key-123".to_owned(), "x".repeat(256)] {
            assert!(validate_api_key(&key).is_ok());
        }
        for key in [
            "secret\n".to_owned(),
            "has space".to_owned(),
            "密钥".to_owned(),
            "x".repeat(257),
        ] {
            let error = validate_api_key(&key).unwrap_err().to_string();
            assert!(!error.contains(&key));
        }
    }
}

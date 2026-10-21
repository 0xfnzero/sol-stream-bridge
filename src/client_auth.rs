use crate::config::ApiKeys;
use anyhow::Result;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use yellowstone_grpc_proto::tonic::{metadata::MetadataMap, Status};

pub(crate) struct ClientAuth(Vec<(String, [u8; 32])>);
impl ClientAuth {
    pub fn new(keys: &ApiKeys) -> Result<Self> {
        keys.validate()?;
        Ok(Self(
            keys.0
                .iter()
                .filter(|(_, key)| !key.is_empty())
                .map(|(user, key)| (user.clone(), Sha256::digest(key.as_bytes()).into()))
                .collect(),
        ))
    }
    pub fn authenticate(&self, metadata: &MetadataMap, peer: &str) -> Result<String, Status> {
        if self.0.is_empty() {
            return Ok(format!("ip:{peer}"));
        }
        let denied = || Status::unauthenticated("missing or invalid API key");
        let read = |name| {
            let mut values = metadata.get_all(name).iter();
            let first = values.next();
            if values.next().is_some() {
                return Err(denied());
            }
            first.map(|v| v.to_str().map_err(|_| denied())).transpose()
        };
        let explicit = read("x-api-key")?;
        let conventional = read("x-token")?;
        if explicit.is_some() && conventional.is_some() && explicit != conventional {
            return Err(denied());
        }
        let key = explicit.or(conventional).ok_or_else(denied)?;
        stream_bridge_client::auth::validate_api_key(key).map_err(|_| denied())?;
        if key.is_empty() {
            return Err(denied());
        }
        let hash: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        let mut user = None;
        for (name, expected) in &self.0 {
            if bool::from(expected.ct_eq(&hash)) {
                user = Some(name.clone());
            }
        }
        user.ok_or_else(denied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yellowstone_grpc_proto::tonic::Code;

    #[test]
    fn optional_keys_aliases_and_redaction() {
        let empty = ApiKeys([("alice".into(), String::new())].into());
        let auth = ClientAuth::new(&empty).unwrap();
        assert_eq!(
            auth.authenticate(&MetadataMap::new(), "127.0.0.1").unwrap(),
            "ip:127.0.0.1"
        );
        let keys = ApiKeys(
            [
                ("alice".into(), "secret-a".into()),
                ("bob".into(), "secret-b".into()),
            ]
            .into(),
        );
        assert!(!format!("{keys:?}").contains("secret"));
        let auth = ClientAuth::new(&keys).unwrap();
        assert_eq!(
            auth.authenticate(&MetadataMap::new(), "")
                .unwrap_err()
                .code(),
            Code::Unauthenticated
        );
        for header in ["x-api-key", "x-token"] {
            for (key, user) in [("secret-a", "alice"), ("secret-b", "bob")] {
                let mut md = MetadataMap::new();
                md.insert(header, key.parse().unwrap());
                assert_eq!(auth.authenticate(&md, "").unwrap(), user);
                md.append(header, key.parse().unwrap());
                assert!(auth.authenticate(&md, "").is_err());
            }
        }
        let mut md = MetadataMap::new();
        md.insert("x-token", "secret-a".parse().unwrap());
        md.insert("x-api-key", "secret-b".parse().unwrap());
        assert!(auth.authenticate(&md, "").is_err());
        md.insert("x-api-key", "secret-a".parse().unwrap());
        assert_eq!(auth.authenticate(&md, "").unwrap(), "alice");
        for key in ["", "wrong", " "] {
            let mut md = MetadataMap::new();
            md.insert("x-token", key.parse().unwrap());
            assert!(auth.authenticate(&md, "").is_err());
        }
        assert!(ClientAuth::new(&ApiKeys(
            [("a".into(), "same".into()), ("b".into(), "same".into())].into()
        ))
        .is_err());
        assert!(ClientAuth::new(&ApiKeys([("a".into(), "secret\n".into())].into())).is_err());
    }
}

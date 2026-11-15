//! Validated codecs for public identity fields in persisted trust records.
use serde::{Deserialize, Deserializer, Serializer};

macro_rules! public_hex {
    ($module:ident, $ty:ty) => {
        pub mod $module {
            use super::*;
            pub fn serialize<S: Serializer>(value: &$ty, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&value.to_hex())
            }
            pub fn deserialize<'de, D: Deserializer<'de>>(
                deserializer: D,
            ) -> Result<$ty, D::Error> {
                <$ty>::from_hex(&String::deserialize(deserializer)?)
                    .map_err(serde::de::Error::custom)
            }
        }
    };
}
public_hex!(node_id, polar_identity::NodeId);
public_hex!(binding_key, polar_identity::BindingPublicKey);

pub mod node_name {
    use super::*;
    pub fn serialize<S: Serializer>(
        value: &polar_identity::NodeName,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value.as_str())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<polar_identity::NodeName, D::Error> {
        polar_identity::NodeName::parse(&String::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}
pub mod timestamp {
    use super::*;
    pub fn serialize<S: Serializer>(
        value: &polar_identity::UnixTimestamp,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(value.as_secs())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<polar_identity::UnixTimestamp, D::Error> {
        Ok(polar_identity::UnixTimestamp::from_secs(u64::deserialize(
            deserializer,
        )?))
    }
}
pub mod optional_timestamp {
    use super::*;
    use serde::Serialize;
    pub fn serialize<S: Serializer>(
        value: &Option<polar_identity::UnixTimestamp>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.map(|v| v.as_secs()).serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<polar_identity::UnixTimestamp>, D::Error> {
        Ok(Option::<u64>::deserialize(deserializer)?.map(polar_identity::UnixTimestamp::from_secs))
    }
}

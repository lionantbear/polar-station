//! Preserve optional field presence as well as values across typed adapters.
use super::*;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "Value")]
pub struct PortableHostV1 {
    pub source: HostRefV1,
    pub display_name: String,
    pub connection: PortableHostConnectionV1,
    pub observation: HostIdentityObservationV1,
    /// Original wire shape; never an authority or a local verification record.
    #[doc(hidden)]
    pub wire_shape: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    source: HostRefV1,
    display_name: String,
    connection: PortableHostConnectionV1,
    observation: HostIdentityObservationV1,
}

impl TryFrom<Value> for PortableHostV1 {
    type Error = serde_json::Error;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let fields: Fields = serde_json::from_value(value.clone())?;
        Ok(Self {
            source: fields.source,
            display_name: fields.display_name,
            connection: fields.connection,
            observation: fields.observation,
            wire_shape: Some(value),
        })
    }
}

impl From<PortableHostV1> for Value {
    fn from(host: PortableHostV1) -> Self {
        let mut value = serde_json::json!({
            "source": host.source, "display_name": host.display_name,
            "connection": host.connection, "observation": host.observation,
        });
        if let Some(original) = &host.wire_shape {
            preserve_omitted_options(&mut value, original);
        }
        value
    }
}

fn preserve_omitted_options(value: &mut Value, original: &Value) {
    if let (Value::Object(current), Value::Object(original)) = (value, original) {
        current.retain(|key, value| {
            if let Some(old) = original.get(key) {
                preserve_omitted_options(value, old);
                true
            } else {
                !value.is_null()
            }
        });
    }
}

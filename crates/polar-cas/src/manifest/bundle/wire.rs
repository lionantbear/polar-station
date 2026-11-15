//! Private wire types keep capability serialization an explicit output action.
use super::*;
use serde::de::{MapAccess, Visitor};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleWire {
    schema: String,
    root: ContentRef,
    directories: Vec<DirectoryRecord>,
    storages: Vec<StorageDescriptor>,
    sources: Vec<ContentSourcesWire>,
    access: AccessMode,
    completeness: Completeness,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentSourcesWire {
    content: ContentRef,
    routes: Vec<RouteWire>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RouteWire {
    Durable {
        storage: String,
        locator: SourceLocator,
    },
    Temporary {
        storage: String,
        locator: SourceLocator,
        request: RequestWire,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestWire {
    method: String,
    url: String,
    #[serde(deserialize_with = "unique_map")]
    headers: BTreeMap<String, String>,
    expires_at_ms: u64,
}

pub(super) fn unique_map<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error> {
    struct UniqueMap;
    impl<'de> Visitor<'de> for UniqueMap {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string map with unique keys")
        }
        fn visit_map<A: MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if result.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate map key"));
                }
            }
            Ok(result)
        }
    }
    d.deserialize_map(UniqueMap)
}

pub(super) fn encode(draft: &BundleDraft) -> Result<Vec<u8>> {
    let wire = BundleWire {
        schema: BUNDLE_SCHEMA.into(),
        root: draft.root.clone(),
        directories: draft.directories.clone(),
        storages: draft.storages.clone(),
        access: draft.access,
        completeness: draft.completeness,
        sources: draft
            .sources
            .iter()
            .map(|source| ContentSourcesWire {
                content: source.content.clone(),
                routes: source
                    .routes
                    .iter()
                    .map(|route| match route {
                        ReadRoute::Durable { storage, locator } => RouteWire::Durable {
                            storage: storage.clone(),
                            locator: locator.clone(),
                        },
                        ReadRoute::Temporary {
                            storage,
                            locator,
                            request,
                        } => RouteWire::Temporary {
                            storage: storage.clone(),
                            locator: locator.clone(),
                            request: RequestWire {
                                method: request.method.clone(),
                                url: request.url.clone(),
                                headers: request.headers.clone(),
                                expires_at_ms: request.expires_at_ms,
                            },
                        },
                    })
                    .collect(),
            })
            .collect(),
    };
    crate::canonical::canonical_json_bytes(&wire)
        .map_err(|_| Error::Invalid("cannot encode manifest bundle".into()))
}

pub(super) fn decode(bytes: &[u8]) -> Result<BundleDraft> {
    let wire: BundleWire = serde_json::from_slice(bytes)
        .map_err(|_| Error::Invalid("invalid manifest bundle encoding".into()))?;
    if wire.schema != BUNDLE_SCHEMA {
        return Err(Error::Invalid("unknown manifest bundle schema".into()));
    }
    Ok(BundleDraft {
        root: wire.root,
        directories: wire.directories,
        storages: wire.storages,
        access: wire.access,
        completeness: wire.completeness,
        sources: wire
            .sources
            .into_iter()
            .map(|source| ContentSources {
                content: source.content,
                routes: source
                    .routes
                    .into_iter()
                    .map(|route| match route {
                        RouteWire::Durable { storage, locator } => {
                            ReadRoute::Durable { storage, locator }
                        }
                        RouteWire::Temporary {
                            storage,
                            locator,
                            request,
                        } => ReadRoute::Temporary {
                            storage,
                            locator,
                            request: TemporaryRequest {
                                method: request.method,
                                url: request.url,
                                headers: request.headers,
                                expires_at_ms: request.expires_at_ms,
                            },
                        },
                    })
                    .collect(),
            })
            .collect(),
    })
}

use std::{fmt, marker::PhantomData};

use secrecy::SecretString;
use serde::{
    Deserialize, Deserializer,
    de::{Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor},
};
use url::Url;

const MAX_STREAMS_PER_MAP: usize = 32;
const MAX_STREAM_KEY_BYTES: usize = 128;
const MAX_STREAM_VALUE_BYTES: usize = 4_096;
const MAX_IVIDEON_TOKEN_BYTES: usize = 16 * 1024;
const MAX_LOCKS_PER_ITEM: usize = 64;
const MAX_INTERCOMS_PER_CAMERA: usize = 64;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpringPage<T> {
    pub(crate) content: Vec<T>,
    pub(crate) total_elements: u64,
    pub(crate) total_pages: u32,
    pub(crate) number: u32,
    pub(crate) size: u32,
    pub(crate) last: bool,
    pub(crate) first: bool,
    pub(crate) empty: bool,
}

#[derive(Deserialize)]
pub(crate) struct AccessPointDto {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) camera: Option<String>,
    #[serde(default)]
    pub(crate) locks: Option<BoundedVec<LockDto, MAX_LOCKS_PER_ITEM>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LockDto {
    pub(crate) number: i64,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) is_permanently_open: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CameraDto {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) streams: Option<StreamMapEvidence>,
    #[serde(default)]
    pub(crate) streams_to_watch: Option<StreamMapEvidence>,
    #[serde(default)]
    pub(crate) ivideon: Option<IvideonEvidence>,
    #[serde(default)]
    pub(crate) intercoms: Option<BoundedVec<CameraIntercomDto, MAX_INTERCOMS_PER_CAMERA>>,
}

pub(crate) struct BoundedVec<T, const LIMIT: usize>(Vec<T>);

impl<T, const LIMIT: usize> BoundedVec<T, LIMIT> {
    pub(crate) fn into_vec(self) -> Vec<T> {
        self.0
    }
}

impl<'de, T, const LIMIT: usize> Deserialize<'de> for BoundedVec<T, LIMIT>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(BoundedVecVisitor::<T, LIMIT>(PhantomData))
    }
}

struct BoundedVecVisitor<T, const LIMIT: usize>(PhantomData<T>);

impl<'de, T, const LIMIT: usize> Visitor<'de> for BoundedVecVisitor<T, LIMIT>
where
    T: Deserialize<'de>,
{
    type Value = BoundedVec<T, LIMIT>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "a sequence containing at most {LIMIT} items")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut items = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(LIMIT));
        while items.len() < LIMIT {
            let Some(item) = sequence.next_element()? else {
                return Ok(BoundedVec(items));
            };
            items.push(item);
        }
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(A::Error::custom("sequence exceeds its item bound"));
        }
        Ok(BoundedVec(items))
    }
}

pub(crate) struct StreamMapEvidence {
    preferred: Option<SecretString>,
}

impl StreamMapEvidence {
    pub(crate) fn available(&self) -> bool {
        self.preferred.is_some()
    }

    pub(crate) fn selected(&self) -> Option<SecretString> {
        self.preferred.clone()
    }
}

impl<'de> Deserialize<'de> for StreamMapEvidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(StreamMapVisitor)
    }
}

struct StreamMapVisitor;

impl<'de> Visitor<'de> for StreamMapVisitor {
    type Value = StreamMapEvidence;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded map of stream names to private URLs")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut count = 0_usize;
        let mut first = None;
        let mut rtsp = None;
        while let Some(key) = map.next_key::<String>()? {
            count += 1;
            if count > MAX_STREAMS_PER_MAP || key.len() > MAX_STREAM_KEY_BYTES {
                return Err(A::Error::custom("stream map exceeds its bound"));
            }
            let value = map.next_value::<String>()?;
            if value.len() > MAX_STREAM_VALUE_BYTES {
                return Err(A::Error::custom("stream value exceeds its bound"));
            }
            if let Some(source) = live_source(&value) {
                if first.is_none() {
                    first = Some(source.clone());
                }
                if key == "rtsp" {
                    rtsp = Some(source);
                }
            }
        }
        Ok(StreamMapEvidence {
            preferred: rtsp.or(first),
        })
    }
}

fn live_source(value: &str) -> Option<SecretString> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "rtsp" | "rtsps") || url.host().is_none() || url.fragment().is_some()
    {
        return None;
    }
    Some(SecretString::from(value.to_owned()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IvideonEvidence {
    #[serde(default)]
    pub(crate) camera_id: Option<String>,
    #[serde(default, deserialize_with = "optional_ivideon_token")]
    pub(crate) token: Option<SecretString>,
    #[serde(default)]
    pub(crate) depth: Option<u64>,
}

fn optional_ivideon_token<'de, D>(deserializer: D) -> Result<Option<SecretString>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    if value
        .as_ref()
        .is_some_and(|value| value.len() > MAX_IVIDEON_TOKEN_BYTES)
    {
        return Err(D::Error::custom("Ivideon token exceeds its byte bound"));
    }
    Ok(value.map(SecretString::from))
}

#[derive(Deserialize)]
pub(crate) struct CameraIntercomDto {
    #[serde(default)]
    pub(crate) id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{AccessPointDto, CameraDto};

    #[test]
    fn access_point_lock_limit_is_enforced_during_deserialization() {
        let locks = (0..65)
            .map(|number| {
                serde_json::json!({
                    "number": number,
                    "name": format!("Door {number}"),
                    "isPermanentlyOpen": null
                })
            })
            .collect::<Vec<_>>();

        let result = serde_json::from_value::<AccessPointDto>(serde_json::json!({
            "id": "access-bounded",
            "name": "Bounded access point",
            "locks": locks
        }));

        assert!(result.is_err());
    }

    #[test]
    fn camera_intercom_limit_is_enforced_during_deserialization() {
        let intercoms = (0..65)
            .map(|index| serde_json::json!({"id": format!("intercom-{index}")}))
            .collect::<Vec<_>>();

        let result = serde_json::from_value::<CameraDto>(serde_json::json!({
            "id": "camera-bounded",
            "name": "Bounded camera",
            "intercoms": intercoms
        }));

        assert!(result.is_err());
    }
}

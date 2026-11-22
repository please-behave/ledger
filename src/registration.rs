use crate::attestation::{valid_rule_identity, valid_version};
use crate::rule_digest::{RuleDigestInput, RuleKind, rule_digest};
use anyhow::{Context, Result, bail, ensure};
use serde::de::value::MapAccessDeserializer;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use std::fmt;
use std::marker::PhantomData;

const MAX_RULES: usize = 100;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistrationRequest {
    pub sender: String,
    pub commit: String,
    #[serde(deserialize_with = "rule_objects")]
    pub rules: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rule {
    pub source: String,
    pub path: String,
    pub id: String,
    pub version: String,
    pub kind: Kind,
    #[serde(default, deserialize_with = "present")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub blocking: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub prompt: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub knowledge: Option<Box<RawValue>>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Check,
    Review,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Review => "review",
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Outcome {
    pub outcome: OutcomeKind,
    pub digest: String,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OutcomeKind {
    Created,
    Unchanged,
    Conflict,
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

fn rule_objects<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Rule>, D::Error> {
    Vec::<JsonObject<Rule>>::deserialize(deserializer)
        .map(|objects| objects.into_iter().map(|object| object.0).collect())
}

// Serde's derived struct visitors also accept positional arrays.
pub(crate) struct JsonObject<T>(pub T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for JsonObject<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = JsonObject<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(JsonObject)
            }
        }

        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
}

impl RegistrationRequest {
    pub fn parse(body: &[u8]) -> Result<(Self, Vec<String>)> {
        let JsonObject(request) = serde_json::from_slice::<JsonObject<Self>>(body)?;
        ensure!(
            !request.sender.is_empty() && !request.sender.contains('\0'),
            "invalid sender"
        );
        ensure!(
            matches!(request.commit.len(), 40 | 64)
                && request.commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid registering commit"
        );
        ensure!(
            (1..=MAX_RULES).contains(&request.rules.len()),
            "invalid number of rules"
        );
        let digests = request
            .rules
            .iter()
            .map(Rule::digest)
            .collect::<Result<_>>()?;
        Ok((request, digests))
    }
}

impl Rule {
    fn digest(&self) -> Result<String> {
        ensure!(
            valid_rule_identity(&self.source, &self.path, &self.id) && valid_version(&self.version),
            "invalid rule identity or version"
        );
        ensure!(
            [&self.name, &self.description, &self.prompt]
                .into_iter()
                .flatten()
                .all(|value| !value.contains('\0')),
            "invalid rule text"
        );
        let kind = match (self.kind, self.blocking, self.prompt.as_deref()) {
            (Kind::Check, None, None) => RuleKind::Check,
            (Kind::Review, Some(blocking), Some(prompt)) => RuleKind::Review { blocking, prompt },
            _ => bail!("invalid fields for rule kind"),
        };
        let knowledge = self
            .knowledge
            .as_deref()
            .map(|raw| serde_json::from_str::<KnowledgeValue>(raw.get()))
            .transpose()?;
        let knowledge = knowledge
            .as_ref()
            .map(|value| value.0.as_object().context("knowledge must be an object"))
            .transpose()?;
        rule_digest(&RuleDigestInput {
            source: &self.source,
            path: &self.path,
            id: &self.id,
            version: &self.version,
            kind,
            name: self.name.as_deref(),
            description: self.description.as_deref(),
            knowledge,
        })
    }
}

// Parse objects before insertion into a JSON map would discard duplicate keys.
struct KnowledgeValue(Value);

impl<'de> Deserialize<'de> for KnowledgeValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(KnowledgeVisitor)
    }
}

struct KnowledgeVisitor;

impl<'de> Visitor<'de> for KnowledgeVisitor {
    type Value = KnowledgeValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string, boolean, array or object")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(KnowledgeValue(Value::String(value.to_owned())))
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(KnowledgeValue(Value::Bool(value)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(KnowledgeValue(value)) = sequence.next_element()? {
            items.push(value);
        }
        Ok(KnowledgeValue(Value::Array(items)))
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut members = Map::new();
        while let Some((key, KnowledgeValue(value))) = map.next_entry()? {
            if members.insert(key, value).is_some() {
                return Err(serde::de::Error::custom("duplicate knowledge member"));
            }
        }
        Ok(KnowledgeValue(Value::Object(members)))
    }
}

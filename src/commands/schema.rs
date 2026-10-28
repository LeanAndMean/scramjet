use super::{COMMAND_CATALOG_BYTES, CommandError, CommandPolicy};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use std::fmt;

#[derive(Clone)]
pub(super) struct Catalog {
    pub definitions: Vec<Definition>,
}
#[derive(Clone)]
pub(super) struct Definition {
    pub name: String,
    pub outcome: Outcome,
    pub steps: Vec<String>,
    pub next: Option<Edge>,
    pub target: Option<String>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    Equal,
    Readable,
    ReplaceVerified,
    AllReaders,
}
#[derive(Clone)]
pub(super) struct Edge {
    pub target: String,
    pub policy: CommandPolicy,
}

struct Text<const N: usize>(String);
impl<'de, const N: usize> Deserialize<'de> for Text<N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct TextVisitor<const N: usize>;
        impl<const N: usize> Visitor<'_> for TextVisitor<N> {
            type Value = Text<N>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "text at most {N} bytes")
            }
            fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
                if text.len() > N {
                    return Err(E::custom("text ceiling"));
                }
                Ok(Text(text.to_owned()))
            }
        }
        d.deserialize_str(TextVisitor::<N>)
    }
}
struct Pair<T>(Vec<T>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Pair<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct PairVisitor<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for PairVisitor<T> {
            type Value = Pair<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("one or two items")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::with_capacity(2);
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                    if items.len() == 2 {
                        // Reject a third item by type, without decoding its potentially nested value.
                        if seq.next_element::<End>()?.is_some() {
                            unreachable!();
                        }
                        break;
                    }
                }
                if items.is_empty() {
                    return Err(de::Error::custom("empty sequence"));
                }
                Ok(Pair(items))
            }
        }
        d.deserialize_seq(PairVisitor(std::marker::PhantomData))
    }
}
struct End;
impl<'de> Deserialize<'de> for End {
    fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Err(de::Error::custom("item ceiling"))
    }
}

macro_rules! object {
    ($name:ident, $($field:ident : $ty:ty),+ $(,)?) => {
        struct $name { $($field: Option<$ty>),+ }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct ObjectVisitor;
                impl<'de> Visitor<'de> for ObjectVisitor {
                    type Value = $name;
                    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("schema object") }
                    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                        $(let mut $field = None;)+
                        while let Some(key) = map.next_key::<Text<16>>()? {
                            match key.0.as_str() {
                                $(stringify!($field) => {
                                    if $field.is_some() { return Err(de::Error::custom("duplicate field")); }
                                    $field = Some(map.next_value()?);
                                })+
                                _ => return Err(de::Error::custom("unknown field")),
                            }
                        }
                        Ok($name { $($field),+ })
                    }
                }
                d.deserialize_map(ObjectVisitor)
            }
        }
    };
}
object!(RawCatalog, commands: Pair<RawDefinition>);
object!(RawDefinition, name: Text<32>, outcome: Text<16>, steps: Pair<RawStep>, next: RawEdge, target: Text<64>);
object!(RawStep, read: Text<64>);
object!(RawEdge, command: Text<32>, policy: Text<16>);

pub(super) fn validate_command_name(name: &str) -> Result<(), CommandError> {
    if name.is_empty()
        || name.len() > 32
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(CommandError("invalid command name"));
    }
    Ok(())
}
fn validate_basename(name: &str) -> Result<(), CommandError> {
    if name.is_empty()
        || name.len() > 64
        || name.bytes().all(|b| b == b'.')
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(CommandError("invalid read basename"));
    }
    Ok(())
}
fn required<T>(value: Option<T>) -> Result<T, CommandError> {
    value.ok_or(CommandError("missing definition field"))
}

pub(super) fn decode(text: &str) -> Result<Catalog, CommandError> {
    if text.len() > COMMAND_CATALOG_BYTES {
        return Err(CommandError("catalog byte ceiling"));
    }
    let raw: RawCatalog =
        serde_json::from_str(text).map_err(|_| CommandError("invalid definition schema"))?;
    let mut definitions = Vec::with_capacity(2);
    for definition in required(raw.commands)?.0 {
        let name = required(definition.name)?.0;
        validate_command_name(&name)?;
        let outcome = match required(definition.outcome)?.0.as_str() {
            "Equal" => Outcome::Equal,
            "Readable" => Outcome::Readable,
            "ReplaceVerified" => Outcome::ReplaceVerified,
            "AllReaders" => Outcome::AllReaders,
            _ => return Err(CommandError("unsupported outcome")),
        };
        let mut steps = Vec::with_capacity(2);
        let target = if outcome == Outcome::AllReaders {
            if name != "summarize"
                || definition.steps.is_some()
                || definition.next.is_some()
                || definition.target.is_some()
            {
                return Err(CommandError(
                    "reader definition forbids steps, targets and edges",
                ));
            }
            None
        } else if outcome == Outcome::ReplaceVerified {
            if name != "replace" || definition.steps.is_some() || definition.next.is_some() {
                return Err(CommandError(
                    "coding definition forbids read steps and edges",
                ));
            }
            let target = required(definition.target)?.0;
            if target != "sample.txt" {
                return Err(CommandError("unsupported coding target"));
            }
            Some(target)
        } else {
            if definition.target.is_some() {
                return Err(CommandError("read definition forbids coding target"));
            }
            for step in required(definition.steps)?.0 {
                let read = required(step.read)?.0;
                validate_basename(&read)?;
                steps.push(read);
            }
            if steps.len() != if outcome == Outcome::Equal { 2 } else { 1 } {
                return Err(CommandError("outcome step count"));
            }
            None
        };
        let next = definition
            .next
            .map(|edge| {
                let target = required(edge.command)?.0;
                validate_command_name(&target)?;
                let policy = match required(edge.policy)?.0.as_str() {
                    "Pause" => CommandPolicy::Pause,
                    "Automatic" => CommandPolicy::Automatic,
                    _ => return Err(CommandError("unsupported edge policy")),
                };
                Ok(Edge { target, policy })
            })
            .transpose()?;
        if definitions.iter().any(|d: &Definition| d.name == name) {
            return Err(CommandError("duplicate command"));
        }
        definitions.push(Definition {
            name,
            outcome,
            steps,
            next,
            target,
        });
    }
    for definition in &definitions {
        let mut cursor = definition;
        for depth in 0..=definitions.len() {
            let Some(edge) = &cursor.next else {
                break;
            };
            if depth == definitions.len() {
                return Err(CommandError("continuation cycle"));
            }
            cursor = definitions
                .iter()
                .find(|d| d.name == edge.target)
                .ok_or(CommandError("unknown edge target"))?;
            if matches!(
                cursor.outcome,
                Outcome::ReplaceVerified | Outcome::AllReaders
            ) {
                return Err(CommandError("execution continuation forbidden"));
            }
        }
    }
    Ok(Catalog { definitions })
}

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

struct Budget {
    nodes: usize,
    strings: usize,
}
impl Budget {
    fn node<E: de::Error>(&mut self) -> Result<(), E> {
        self.nodes = self
            .nodes
            .checked_sub(1)
            .ok_or_else(|| E::custom("haiku selected JSON resource limit"))?;
        Ok(())
    }
    fn string<E: de::Error>(&mut self, bytes: usize) -> Result<(), E> {
        self.strings = self
            .strings
            .checked_sub(bytes)
            .ok_or_else(|| E::custom("haiku selected strings resource limit"))?;
        Ok(())
    }
}
struct Seed<'a>(&'a mut Budget);
impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        self.0.node()?;
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Seed<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded duplicate-free selected JSON")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("nonfinite JSON"))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        self.0.string(value.len())?;
        Ok(value.into())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = access.next_element_seed(Seed(self.0))? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = access.next_key::<String>()? {
            self.0.string(key.len())?;
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate selected JSON field"));
            }
            values.insert(key, access.next_value_seed(Seed(self.0))?);
        }
        Ok(Value::Object(values))
    }
}
pub(super) fn items<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Value>, D::Error> {
    struct Items;
    impl<'de> Visitor<'de> for Items {
        type Value = Vec<Value>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("at most eight complete selected items")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Vec<Value>, A::Error> {
            let mut budget = Budget {
                nodes: crate::PROVIDER_ENVELOPE_BYTES / 2,
                strings: crate::PROVIDER_ENVELOPE_BYTES,
            };
            let mut items = Vec::with_capacity(8);
            while let Some(item) = access.next_element_seed(Seed(&mut budget))? {
                if items.len() == 8 {
                    return Err(de::Error::custom("selected item resource limit"));
                }
                items.push(item);
            }
            Ok(items)
        }
    }
    d.deserialize_seq(Items)
}

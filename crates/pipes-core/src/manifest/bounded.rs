//! Budgets apply during deserialization, including every expansion of a YAML alias.

use std::fmt;

use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor},
};

const VALUE_LIMIT: usize = 1024 * 1024;
const SCALAR_BYTE_LIMIT: usize = 8 * 1024 * 1024;
const COLLECTION_DEPTH_LIMIT: usize = 96;

struct Budget {
    values: usize,
    scalar_bytes: usize,
}

impl Budget {
    fn value<E: de::Error>(&mut self) -> Result<(), E> {
        if self.values == 0 {
            return Err(E::custom("expanded YAML value budget exceeded"));
        }
        self.values -= 1;
        Ok(())
    }

    fn scalar<E: de::Error>(&mut self, bytes: usize) -> Result<(), E> {
        if bytes > self.scalar_bytes {
            return Err(E::custom("expanded YAML scalar-byte budget exceeded"));
        }
        self.scalar_bytes -= bytes;
        Ok(())
    }
}

pub(super) fn from_str<'de, T: Deserialize<'de>>(
    yaml: &'de str,
) -> Result<T, serde_yaml_ng::Error> {
    let mut budget = Budget {
        values: VALUE_LIMIT,
        scalar_bytes: SCALAR_BYTE_LIMIT,
    };
    T::deserialize(Bounded {
        inner: serde_yaml_ng::Deserializer::from_str(yaml),
        budget: &mut budget,
        depth: 0,
    })
}

struct Bounded<'a, D> {
    inner: D,
    budget: &'a mut Budget,
    depth: usize,
}

macro_rules! deserialize {
    ($method:ident $(, $argument:ident: $kind:ty)*) => {
        fn $method<V: Visitor<'de>>(
            self,
            $($argument: $kind,)*
            visitor: V,
        ) -> Result<V::Value, Self::Error> {
            self.budget.value()?;
            self.inner.$method($($argument,)* BoundedVisitor {
                inner: visitor,
                budget: self.budget,
                depth: self.depth,
            })
        }
    };
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Bounded<'_, D> {
    type Error = D::Error;

    deserialize!(deserialize_any);
    deserialize!(deserialize_bool);
    deserialize!(deserialize_i8);
    deserialize!(deserialize_i16);
    deserialize!(deserialize_i32);
    deserialize!(deserialize_i64);
    deserialize!(deserialize_i128);
    deserialize!(deserialize_u8);
    deserialize!(deserialize_u16);
    deserialize!(deserialize_u32);
    deserialize!(deserialize_u64);
    deserialize!(deserialize_u128);
    deserialize!(deserialize_f32);
    deserialize!(deserialize_f64);
    deserialize!(deserialize_char);
    deserialize!(deserialize_str);
    deserialize!(deserialize_string);
    deserialize!(deserialize_bytes);
    deserialize!(deserialize_byte_buf);
    deserialize!(deserialize_option);
    deserialize!(deserialize_unit);
    deserialize!(deserialize_unit_struct, name: &'static str);
    deserialize!(deserialize_newtype_struct, name: &'static str);
    deserialize!(deserialize_seq);
    deserialize!(deserialize_tuple, length: usize);
    deserialize!(deserialize_tuple_struct, name: &'static str, length: usize);
    deserialize!(deserialize_map);
    deserialize!(deserialize_struct, name: &'static str, fields: &'static [&'static str]);
    deserialize!(deserialize_enum, name: &'static str, variants: &'static [&'static str]);
    deserialize!(deserialize_identifier);
    deserialize!(deserialize_ignored_any);

    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}

struct BoundedVisitor<'a, V> {
    inner: V,
    budget: &'a mut Budget,
    depth: usize,
}

macro_rules! visit_scalar {
    ($method:ident, $kind:ty) => {
        fn $method<E: de::Error>(self, value: $kind) -> Result<Self::Value, E> {
            self.inner.$method(value)
        }
    };
}

impl<'de, V: Visitor<'de>> Visitor<'de> for BoundedVisitor<'_, V> {
    type Value = V::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.expecting(formatter)
    }

    visit_scalar!(visit_bool, bool);
    visit_scalar!(visit_i8, i8);
    visit_scalar!(visit_i16, i16);
    visit_scalar!(visit_i32, i32);
    visit_scalar!(visit_i64, i64);
    visit_scalar!(visit_i128, i128);
    visit_scalar!(visit_u8, u8);
    visit_scalar!(visit_u16, u16);
    visit_scalar!(visit_u32, u32);
    visit_scalar!(visit_u64, u64);
    visit_scalar!(visit_u128, u128);
    visit_scalar!(visit_f32, f32);
    visit_scalar!(visit_f64, f64);

    fn visit_char<E: de::Error>(self, value: char) -> Result<Self::Value, E> {
        self.budget.scalar(value.len_utf8())?;
        self.inner.visit_char(value)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.budget.scalar(value.len())?;
        self.inner.visit_str(value)
    }

    fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<Self::Value, E> {
        self.budget.scalar(value.len())?;
        self.inner.visit_borrowed_str(value)
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        self.budget.scalar(value.len())?;
        self.inner.visit_string(value)
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        self.budget.scalar(value.len())?;
        self.inner.visit_bytes(value)
    }

    fn visit_borrowed_bytes<E: de::Error>(self, value: &'de [u8]) -> Result<Self::Value, E> {
        self.budget.scalar(value.len())?;
        self.inner.visit_borrowed_bytes(value)
    }

    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        self.budget.scalar(value.len())?;
        self.inner.visit_byte_buf(value)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_unit()
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_none()
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        self.inner.visit_some(Bounded {
            inner: deserializer,
            budget: self.budget,
            depth: self.depth,
        })
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        self.inner.visit_newtype_struct(Bounded {
            inner: deserializer,
            budget: self.budget,
            depth: self.depth,
        })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, sequence: A) -> Result<Self::Value, A::Error> {
        if self.depth >= COLLECTION_DEPTH_LIMIT {
            return Err(de::Error::custom(
                "YAML collection nesting exceeds 96 levels",
            ));
        }
        self.inner.visit_seq(BoundedAccess {
            inner: sequence,
            budget: self.budget,
            depth: self.depth + 1,
        })
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        if self.depth >= COLLECTION_DEPTH_LIMIT {
            return Err(de::Error::custom(
                "YAML collection nesting exceeds 96 levels",
            ));
        }
        self.inner.visit_map(BoundedAccess {
            inner: map,
            budget: self.budget,
            depth: self.depth + 1,
        })
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_enum(BoundedAccess {
            inner: data,
            budget: self.budget,
            depth: self.depth,
        })
    }
}

struct BoundedSeed<'a, S> {
    inner: S,
    budget: &'a mut Budget,
    depth: usize,
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for BoundedSeed<'_, S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        self.inner.deserialize(Bounded {
            inner: deserializer,
            budget: self.budget,
            depth: self.depth,
        })
    }
}

struct BoundedAccess<'a, A> {
    inner: A,
    budget: &'a mut Budget,
    depth: usize,
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for BoundedAccess<'_, A> {
    type Error = A::Error;

    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        self.inner.next_element_seed(BoundedSeed {
            inner: seed,
            budget: self.budget,
            depth: self.depth,
        })
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner
            .size_hint()
            .map(|size| size.min(self.budget.values))
    }
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for BoundedAccess<'_, A> {
    type Error = A::Error;

    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        self.inner.next_key_seed(BoundedSeed {
            inner: seed,
            budget: self.budget,
            depth: self.depth,
        })
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        self.inner.next_value_seed(BoundedSeed {
            inner: seed,
            budget: self.budget,
            depth: self.depth,
        })
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner
            .size_hint()
            .map(|size| size.min(self.budget.values))
    }
}

impl<'de, 'a, A: EnumAccess<'de>> EnumAccess<'de> for BoundedAccess<'a, A> {
    type Error = A::Error;
    type Variant = BoundedAccess<'a, A::Variant>;

    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self::Variant), Self::Error> {
        let (value, variant) = self.inner.variant_seed(BoundedSeed {
            inner: seed,
            budget: self.budget,
            depth: self.depth,
        })?;
        Ok((
            value,
            BoundedAccess {
                inner: variant,
                budget: self.budget,
                depth: self.depth,
            },
        ))
    }
}

impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for BoundedAccess<'_, A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        self.inner.unit_variant()
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        self.inner.newtype_variant_seed(BoundedSeed {
            inner: seed,
            budget: self.budget,
            depth: self.depth,
        })
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        length: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.tuple_variant(
            length,
            BoundedVisitor {
                inner: visitor,
                budget: self.budget,
                depth: self.depth,
            },
        )
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.struct_variant(
            fields,
            BoundedVisitor {
                inner: visitor,
                budget: self.budget,
                depth: self.depth,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_expansions_are_charged_before_building_the_full_value() {
        let mut budget = Budget {
            values: 100,
            scalar_bytes: 10,
        };
        let result = Vec::<String>::deserialize(Bounded {
            inner: serde_yaml_ng::Deserializer::from_str("[&word abcdef, *word, *word]"),
            budget: &mut budget,
            depth: 0,
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("scalar-byte budget")
        );
        assert_eq!(budget.scalar_bytes, 4);
    }

    #[test]
    fn sequence_values_share_one_budget() {
        let mut budget = Budget {
            values: 3,
            scalar_bytes: 100,
        };
        let result = Vec::<String>::deserialize(Bounded {
            inner: serde_yaml_ng::Deserializer::from_str("[a, b, c]"),
            budget: &mut budget,
            depth: 0,
        });
        assert!(result.unwrap_err().to_string().contains("value budget"));
        assert_eq!(budget.values, 0);
    }
}

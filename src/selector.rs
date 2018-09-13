use std::fmt;
use std::ops::Deref;

use scraper::Selector as RawSelector;
use serde::de::{self, Deserialize, Deserializer, Visitor};

#[derive(Debug)]
pub struct SelectorEx {
    pub selector: Selector,
    pub attr: Option<String>,
}

impl<'de> Deserialize<'de> for SelectorEx {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SelectorExVisitor)
    }
}

struct SelectorExVisitor;

impl Visitor<'_> for SelectorExVisitor {
    type Value = SelectorEx;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a valid CSS selector and attr")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let mut iter = v.splitn(2, '|');
        let selector = SelectorVisitor.visit_str(
            iter.next()
                .ok_or_else(|| de::Error::invalid_length(1, &self))?,
        )?;
        let attr = iter.next().map(ToOwned::to_owned);
        Ok(SelectorEx { selector, attr })
    }
}

#[derive(Debug)]
pub struct Selector(RawSelector);

impl<'de> Deserialize<'de> for Selector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SelectorVisitor)
    }
}

impl Deref for Selector {
    type Target = RawSelector;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct SelectorVisitor;

impl Visitor<'_> for SelectorVisitor {
    type Value = Selector;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a valid CSS selector")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if v.len() == 0 {
            return Err(de::Error::invalid_length(1, &self));
        }
        let s = RawSelector::parse(v).map_err(|e| {
            de::Error::custom(SelectorVisitorError(v, e.location.line, e.location.column))
        })?;
        Ok(Selector(s))
    }
}

struct SelectorVisitorError<'a>(&'a str, u32, u32);

impl fmt::Display for SelectorVisitorError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "\"{}\" is not a valid selector({}:{})",
            self.0, self.1, self.2
        )
    }
}

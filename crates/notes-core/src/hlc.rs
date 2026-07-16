//! Hybrid logical clocks used to totally order concurrent field updates.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hlc {
    wall_ms: u64,
    counter: u32,
    device_id: uuid::Uuid,
}

impl Hlc {
    pub fn new(wall_ms: u64, counter: u32, device_id: uuid::Uuid) -> Self {
        Self {
            wall_ms,
            counter,
            device_id,
        }
    }

    pub fn wall_ms(&self) -> u64 {
        self.wall_ms
    }

    pub fn counter(&self) -> u32 {
        self.counter
    }

    pub fn device_id(&self) -> uuid::Uuid {
        self.device_id
    }

    pub fn timestamp_seconds(&self) -> i64 {
        (self.wall_ms / 1_000).min(i64::MAX as u64) as i64
    }

    /// Advance a local clock for a newly emitted event.
    pub fn send(previous: Option<&Self>, now_ms: u64, device_id: uuid::Uuid) -> Self {
        match previous {
            Some(previous) if previous.wall_ms >= now_ms => Self::new(
                previous.wall_ms,
                previous.counter.saturating_add(1),
                device_id,
            ),
            _ => Self::new(now_ms, 0, device_id),
        }
    }

    /// Standard HLC receive rule. The returned clock is a new local event and
    /// therefore always compares greater than both inputs, even under skew.
    pub fn receive(
        local: Option<&Self>,
        remote: &Self,
        now_ms: u64,
        device_id: uuid::Uuid,
    ) -> Self {
        let local_wall = local.map_or(0, |clock| clock.wall_ms);
        let wall_ms = now_ms.max(local_wall).max(remote.wall_ms);
        let counter = match (
            local.filter(|clock| clock.wall_ms == wall_ms),
            remote.wall_ms == wall_ms,
        ) {
            (Some(local), true) => local.counter.max(remote.counter).saturating_add(1),
            (Some(local), false) => local.counter.saturating_add(1),
            (None, true) => remote.counter.saturating_add(1),
            (None, false) => 0,
        };
        Self::new(wall_ms, counter, device_id)
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:016x}-{:08x}-{}",
            self.wall_ms,
            self.counter,
            self.device_id.simple()
        )
    }
}

impl FromStr for Hlc {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let mut parts = value.splitn(3, '-');
        let wall_ms = u64::from_str_radix(parts.next().context("HLC is missing wall time")?, 16)
            .context("invalid HLC wall time")?;
        let counter = u32::from_str_radix(parts.next().context("HLC is missing counter")?, 16)
            .context("invalid HLC counter")?;
        let device = parts.next().context("HLC is missing device suffix")?;
        if device.len() != 32 || !device.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("invalid HLC device suffix");
        }
        let device_id = uuid::Uuid::parse_str(device).context("invalid HLC device UUID")?;
        Ok(Self::new(wall_ms, counter, device_id))
    }
}

impl Serialize for Hlc {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hlc {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl rusqlite::types::ToSql for Hlc {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Owned(
            rusqlite::types::Value::Text(self.to_string()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_order_matches_structural_order() {
        let first = Hlc::new(10, 2, uuid::Uuid::from_u128(1));
        let second = Hlc::new(10, 3, uuid::Uuid::from_u128(1));
        assert!(first < second);
        assert!(first.to_string() < second.to_string());
        assert_eq!(first.to_string().parse::<Hlc>().expect("parse"), first);
    }

    #[test]
    fn receive_stays_monotonic_under_clock_skew() {
        let device = uuid::Uuid::from_u128(1);
        let remote = Hlc::new(10_000, 8, uuid::Uuid::from_u128(2));
        let received = Hlc::receive(None, &remote, 5, device);
        assert!(received > remote);
        let sent = Hlc::send(Some(&received), 6, device);
        assert!(sent > received);
    }
}

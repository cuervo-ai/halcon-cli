//! Deterministic execution primitives for reproducible agent runs.
//!
//! Provides seeded UUID generation and a deterministic clock so that
//! replays can produce identical UUIDs and timestamps.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Clock that can be real-time or deterministic (for replay).
#[derive(Debug, Clone)]
pub enum ExecutionClock {
    /// Uses `Utc::now()` — normal production mode.
    RealTime,
    /// Returns `base + offset` where offset increments by 1ms per call.
    Deterministic {
        base: DateTime<Utc>,
        offset_counter: Arc<AtomicU64>,
    },
}

impl ExecutionClock {
    /// Create a deterministic clock starting at `base`.
    pub fn deterministic(base: DateTime<Utc>) -> Self {
        Self::Deterministic {
            base,
            offset_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get the current time. Real-time returns `Utc::now()`,
    /// deterministic returns base + monotonically increasing offset.
    pub fn now(&self) -> DateTime<Utc> {
        match self {
            Self::RealTime => Utc::now(),
            Self::Deterministic {
                base,
                offset_counter,
            } => {
                let offset_ms = offset_counter.fetch_add(1, Ordering::Relaxed);
                *base + TimeDelta::milliseconds(offset_ms as i64)
            }
        }
    }
}

impl Default for ExecutionClock {
    fn default() -> Self {
        Self::RealTime
    }
}

/// UUID generator that can be random or seeded (for replay determinism).
#[derive(Debug, Clone)]
pub enum UuidGenerator {
    /// Uses `Uuid::new_v4()` — normal production mode.
    Random,
    /// Produces deterministic UUIDs from a seed + counter.
    Seeded {
        seed: [u8; 16],
        counter: Arc<AtomicU64>,
    },
}

impl UuidGenerator {
    /// Create a seeded generator. The seed string is SHA-256 hashed to produce
    /// a 16-byte seed. Same seed string → same UUID sequence.
    pub fn seeded(seed_str: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(seed_str.as_bytes());
        let hash = hasher.finalize();
        let mut seed = [0u8; 16];
        seed.copy_from_slice(&hash[..16]);
        Self::Seeded {
            seed,
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Generate the next UUID.
    pub fn next(&self) -> Uuid {
        match self {
            Self::Random => Uuid::new_v4(),
            Self::Seeded { seed, counter } => {
                let count = counter.fetch_add(1, Ordering::Relaxed);
                let mut hasher = Sha256::new();
                hasher.update(seed);
                hasher.update(count.to_le_bytes());
                let hash = hasher.finalize();
                let mut bytes = [0u8; 16];
                bytes.copy_from_slice(&hash[..16]);
                // Set version 4 and variant bits for valid UUID format.
                bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
                bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 1
                Uuid::from_bytes(bytes)
            }
        }
    }
}

impl Default for UuidGenerator {
    fn default() -> Self {
        Self::Random
    }
}

/// Execution context bundling deterministic primitives.
///
/// In production: uses random UUIDs and real-time clock.
/// In replay/test: uses seeded UUIDs and deterministic clock.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub uuid_gen: UuidGenerator,
    pub clock: ExecutionClock,
    pub execution_id: Uuid,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            uuid_gen: UuidGenerator::default(),
            clock: ExecutionClock::default(),
            execution_id: Uuid::new_v4(),
        }
    }
}

impl ExecutionContext {
    /// Create a deterministic execution context for replay.
    pub fn deterministic(seed: &str, base_time: DateTime<Utc>) -> Self {
        let uuid_gen = UuidGenerator::seeded(seed);
        let execution_id = uuid_gen.next();
        Self {
            uuid_gen,
            clock: ExecutionClock::deterministic(base_time),
            execution_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_generator_random_unique() {
        let gen = UuidGenerator::Random;
        let a = gen.next();
        let b = gen.next();
        assert_ne!(a, b);
    }

    #[test]
    fn uuid_generator_seeded_deterministic() {
        let gen = UuidGenerator::seeded("test-seed");
        let a = gen.next();
        let b = gen.next();
        assert_ne!(a, b); // Different within same sequence

        // Same seed → same sequence
        let gen2 = UuidGenerator::seeded("test-seed");
        let a2 = gen2.next();
        let b2 = gen2.next();
        assert_eq!(a, a2);
        assert_eq!(b, b2);
    }

    #[test]
    fn uuid_generator_seeded_same_seed_same_sequence() {
        let gen1 = UuidGenerator::seeded("hello-world");
        let gen2 = UuidGenerator::seeded("hello-world");
        for _ in 0..10 {
            assert_eq!(gen1.next(), gen2.next());
        }
    }

    #[test]
    fn uuid_generator_different_seeds_differ() {
        let gen1 = UuidGenerator::seeded("seed-a");
        let gen2 = UuidGenerator::seeded("seed-b");
        assert_ne!(gen1.next(), gen2.next());
    }

    #[test]
    fn execution_clock_realtime_advances() {
        let clock = ExecutionClock::RealTime;
        let t1 = clock.now();
        let t2 = clock.now();
        assert!(t2 >= t1);
    }

    #[test]
    fn execution_clock_deterministic_monotonic() {
        let base = Utc::now();
        let clock = ExecutionClock::deterministic(base);
        let t1 = clock.now();
        let t2 = clock.now();
        let t3 = clock.now();
        assert!(t2 > t1);
        assert!(t3 > t2);
    }

    #[test]
    fn execution_clock_deterministic_reproducible() {
        let base = Utc::now();
        let clock1 = ExecutionClock::deterministic(base);
        let clock2 = ExecutionClock::deterministic(base);
        assert_eq!(clock1.now(), clock2.now());
        assert_eq!(clock1.now(), clock2.now());
    }

    #[test]
    fn execution_context_default_is_random() {
        let ctx = ExecutionContext::default();
        assert!(!ctx.execution_id.is_nil());
        matches!(ctx.uuid_gen, UuidGenerator::Random);
        matches!(ctx.clock, ExecutionClock::RealTime);
    }

    #[test]
    fn execution_context_deterministic_from_seed() {
        let base = Utc::now();
        let ctx1 = ExecutionContext::deterministic("my-seed", base);
        let ctx2 = ExecutionContext::deterministic("my-seed", base);
        assert_eq!(ctx1.execution_id, ctx2.execution_id);
        // Subsequent UUIDs also match.
        assert_eq!(ctx1.uuid_gen.next(), ctx2.uuid_gen.next());
    }

    #[test]
    fn uuid_generator_seeded_produces_valid_v4() {
        let gen = UuidGenerator::seeded("validity-check");
        for _ in 0..20 {
            let id = gen.next();
            assert_eq!(id.get_version_num(), 4);
        }
    }

    #[test]
    fn execution_clock_deterministic_base_plus_offset() {
        let base = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = ExecutionClock::deterministic(base);
        let t0 = clock.now();
        let t1 = clock.now();
        assert_eq!(t0, base); // offset=0
        assert_eq!((t1 - base).num_milliseconds(), 1); // offset=1
    }
}

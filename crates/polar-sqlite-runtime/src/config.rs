use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SqliteDurability {
    #[default]
    Normal,
    Full,
}

/// Topology values for one canonical SQLite target.
#[derive(Clone, Debug)]
pub struct SqliteRuntimeConfig {
    pub queue_capacity: usize,
    pub reader_workers: usize,
    pub reader_queue_capacity: usize,
    pub busy_timeout: Duration,
    /// Selected once when opening the writer; cannot be silently changed by a
    /// second handle to the same canonical database.
    pub durability: SqliteDurability,
}

impl Default for SqliteRuntimeConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 256,
            reader_workers: 4,
            reader_queue_capacity: 64,
            busy_timeout: Duration::from_secs(5),
            durability: SqliteDurability::Normal,
        }
    }
}

impl SqliteRuntimeConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.queue_capacity == 0 {
            return Err("SQLite writer queue capacity must be positive");
        }
        if self.reader_workers == 0 {
            return Err("SQLite reader worker count must be positive");
        }
        if self.reader_queue_capacity == 0 {
            return Err("SQLite reader queue capacity must be positive");
        }
        Ok(())
    }
}

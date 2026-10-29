//! Database access. Functions here translate between rows and domain types and
//! contain no business rules; callers receive typed values, never SQLx rows.
//!
//! Rows read back from the database are parsed with the same domain
//! constructors used for input, so a corrupt row surfaces as an error instead
//! of a silently wrong number.

mod config;
mod connection;
mod error;
pub mod reference;
pub mod trades;

pub use config::PersistenceConfig;
pub(crate) use config::RawPersistenceConfig;
pub use connection::{connect, ping};
pub use error::{PersistenceConfigError, PersistenceError};

//! Strongly typed entity identifiers.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Macro to generate a strongly typed 64-bit unsigned entity identifier.
macro_rules! define_id {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[repr(transparent)]
        pub struct $name(pub u64);

        impl $name {
            /// Create a new identifier.
            #[inline]
            #[must_use]
            pub const fn new(val: u64) -> Self {
                Self(val)
            }

            /// Return the underlying raw numeric value.
            #[inline]
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<u64> for $name {
            #[inline]
            fn from(val: u64) -> Self {
                Self(val)
            }
        }

        impl From<$name> for u64 {
            #[inline]
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

define_id!(UserId, "Strongly-typed identifier for a user.");
define_id!(ProjectId, "Strongly-typed identifier for a project.");
define_id!(PageId, "Strongly-typed identifier for a page.");

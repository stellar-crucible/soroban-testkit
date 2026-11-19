#[cfg(feature = "proptest")]
pub mod errors;
#[cfg(feature = "proptest")]
pub mod strategies;
#[cfg(feature = "proptest")]
pub mod ttl;

#[cfg(feature = "proptest")]
pub use errors::{host_error_code_coverage, HostError};
#[cfg(feature = "proptest")]
pub use ttl::{
    entry_ttl, expired_ttl, ttl_extension_delta, ttl_extension_delta_up_to, EntryTtl,
    MAINNET_MAX_ENTRY_TTL,
};

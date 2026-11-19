#[cfg(feature = "proptest")]
pub mod errors;
#[cfg(feature = "proptest")]
pub mod multisig;
#[cfg(feature = "proptest")]
pub mod strategies;

#[cfg(feature = "proptest")]
pub use errors::{host_error_code_coverage, HostError};
#[cfg(feature = "proptest")]
pub use multisig::{multisig_policy, MultisigPolicy, Signer};

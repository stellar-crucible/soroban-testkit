#[cfg(feature = "proptest")]
pub mod errors;
#[cfg(feature = "proptest")]
pub mod strategies;
#[cfg(feature = "proptest")]
pub mod tuples;

#[cfg(feature = "proptest")]
pub use errors::{host_error_code_coverage, HostError};
#[cfg(feature = "proptest")]
pub use tuples::{
    byte_array, byte_array_32, byte_array_64, soroban_tuple_2, soroban_tuple_3, ByteArrayStrategy,
};

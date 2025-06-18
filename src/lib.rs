pub mod bindings;

// Re-export the interface to maintain compatibility
pub use bindings::*;

unsafe impl Send for fmpz_poly_struct {}

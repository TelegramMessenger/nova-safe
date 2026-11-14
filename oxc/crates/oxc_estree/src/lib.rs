#![forbid(unsafe_code)]

/// Marker trait for `#[generate_derive(ESTree)]`.
///
/// nova-safe does not vendor upstream's ESTree serializer (the `serialize` module): nova never
/// serializes an AST to JSON.
pub trait ESTree {}

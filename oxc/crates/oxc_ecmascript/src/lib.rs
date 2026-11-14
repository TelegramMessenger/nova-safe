//! Methods defined in the [ECMAScript Language Specification](https://tc39.es/ecma262).
//!
//! nova-safe keeps only the syntax-directed operations its parser, semantic checker and
//! compiler call; constant evaluation and side-effect analysis are not vendored.
#![forbid(unsafe_code)]

// [Syntax-Directed Operations](https://tc39.es/ecma262/#sec-syntax-directed-operations)
mod bound_names;
mod is_simple_parameter_list;
mod private_bound_identifiers;
mod prop_name;

pub use self::{
    bound_names::BoundNames, is_simple_parameter_list::IsSimpleParameterList,
    private_bound_identifiers::PrivateBoundIdentifiers, prop_name::PropName,
};

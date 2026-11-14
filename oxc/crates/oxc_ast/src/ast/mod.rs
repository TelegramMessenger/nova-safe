//! AST Definitions
//!
//! # Enum inheritance
//!
//! Some enum AST types inherit variants from other enums using the `inherit_variants!` macro.
//!
//! "Inherit" means: If `enum Y` inherits the variants of `enum X`,
//! then all `X`'s variants are duplicated as variants of `Y`.
//!
//! This is mainly an explanation of the consumer-facing API. For further details on implementation,
//! see comments in `src/ast/macros.rs`.
//!
//! ## Defining enum inheritance
//!
//! Instead of nested enums:
//!
//! ```ignore
//! pub enum Expression<'a> {
//!     BooleanLiteral(Box<'a, BooleanLiteral>),
//!     NullLiteral(Box<'a, NullLiteral>),
//!     // ...more variants
//!     MemberExpression(MemberExpression<'a>),
//! }
//!
//! pub enum MemberExpression<'a> {
//!     ComputedMemberExpression(Box<'a, ComputedMemberExpression<'a>>),
//!     StaticMemberExpression(Box<'a, StaticMemberExpression<'a>>),
//!     PrivateFieldExpression(Box<'a, PrivateFieldExpression<'a>>),
//! }
//! ```
//!
//! We define the types using `inherit_variants!` macro:
//!
//! ```ignore
//! inherit_variants! {
//! #[repr(C, u8)]
//! pub enum Expression<'a> {
//!     BooleanLiteral(Box<'a, BooleanLiteral>) = 0,
//!     NullLiteral(Box<'a, NullLiteral>) = 1,
//!     // ...more variants
//!     @inherit MemberExpression,
//! }
//! }
//!
//! #[repr(C, u8)]
//! pub enum MemberExpression<'a> {
//!     ComputedMemberExpression(Box<'a, ComputedMemberExpression<'a>>) = 48,
//!     StaticMemberExpression(Box<'a, StaticMemberExpression<'a>>) = 49,
//!     PrivateFieldExpression(Box<'a, PrivateFieldExpression<'a>>) = 50,
//! }
//! ```
//!
//! `inherit_variants!` macro expands `Expression` to:
//!
//! ```ignore
//! #[repr(C, u8)]
//! pub enum Expression<'a> {
//!     BooleanLiteral(Box<'a, BooleanLiteral>) = 0,
//!     NullLiteral(Box<'a, NullLiteral>) = 1,
//!     // ...more variants
//!
//!     // Inherited from `MemberExpression`
//!     ComputedMemberExpression(Box<'a, ComputedMemberExpression<'a>>) = 48,
//!     StaticMemberExpression(Box<'a, StaticMemberExpression<'a>>) = 49,
//!     PrivateFieldExpression(Box<'a, PrivateFieldExpression<'a>>) = 50,
//! }
//!
//! shared_enum_variants!(
//!     Expression, MemberExpression,
//!     is_member_expression,
//!     into_member_expression,
//!     as_member_expression, as_member_expression_mut,
//!     to_member_expression, to_member_expression_mut,
//!     [ComputedMemberExpression, StaticMemberExpression, PrivateFieldExpression]
//! )
//! ```
//!
//! See `src/ast/macros.rs` for what `shared_enum_variants!` macro expands to.
//! It provides the APIs listed below.
//!
//! ## Using inherited variants
//!
//! #### Creation
//!
//! ```ignore
//! // Old
//! let expr = Expression::MemberExpression(
//!   MemberExpression::ComputedMemberExpression(computed_member_expr)
//! );
//!
//! // New
//! let expr = Expression::ComputedMemberExpression(computed_member_expr);
//! ```
//!
//! #### Conversion
//!
//! ```ignore
//! // Old
//! let expr = Expression::MemberExpression(member_expr);
//!
//! // New
//! let expr = Expression::from(member_expr);
//! ```
//!
//! ```ignore
//! // Old
//! let maybe_member_expr = match expr {
//!     Expression::MemberExpression(member_expr) => Some(member_expr),
//!     _ => None,
//! };
//!
//! // New
//! let maybe_member_expr = MemberExpression::try_from(expr).ok();
//! ```
//!
//! #### Testing
//!
//! ```ignore
//! // Old
//! if matches!(expr, Expression::MemberExpression(_)) { }
//!
//! // New
//! if expr.is_member_expression() { }
//! // or
//! if matches!(expr, match_member_expression!(Expression)) { }
//! ```
//!
//! #### Branching
//!
//! ```ignore
//! // Old
//! if let Expression::MemberExpression(member_expr) = &expr { }
//!
//! // New (a borrowed view of the member expression variants)
//! if let Some(member_expr) = expr.as_member_expression() { }
//! ```
//!
//! #### Matching
//!
//! ```ignore
//! // Old
//! match get_expression() {
//!     Expression::MemberExpression(member_expr) => visitor.visit(member_expr),
//! }
//!
//! // New (exhaustive match on the inherited variants)
//! match get_expression() {
//!     Expression::ComputedMemberExpression(e) => visitor.visit_computed(e),
//!     Expression::StaticMemberExpression(e) => visitor.visit_static(e),
//!     Expression::PrivateFieldExpression(e) => visitor.visit_private_field(e),
//! }
//! ```
//!
//! ## No casts between the enums
//!
//! Upstream oxc converts `&Expression` to `&MemberExpression` (and the like) with a pointer
//! cast that relies on `#[repr(C, u8)]` and equal discriminants. nova-safe has no `unsafe`, so
//! those `as_*`/`to_*` conversions are gone: owned values convert with `From`/`TryFrom`, and
//! borrowed ones are matched variant by variant (`MemberExpressionRef` borrows the member
//! expression variants).

// Re-export AST types from other crates
pub use oxc_span::{Language, LanguageVariant, ModuleKind, SourceType, Span, Str};
pub use oxc_syntax::{
    number::{BigintBase, NumberBase},
    operator::{
        AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator, UpdateOperator,
    },
};

pub(crate) mod comment;
pub(crate) mod js;
pub(crate) mod jsx;
pub(crate) mod literal;
pub(crate) mod macros;
pub(crate) mod ts;

pub use comment::*;
pub use js::*;
pub use jsx::*;
pub use literal::*;
pub use ts::*;

use macros::inherit_variants;

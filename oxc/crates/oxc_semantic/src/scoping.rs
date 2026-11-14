use std::{
    borrow::Borrow,
    collections::hash_map::Entry,
    fmt,
    hash::{Hash, Hasher},
    mem,
};

use rustc_hash::{FxHashMap, FxHashSet};

use oxc_allocator::{
    Allocator, CloneIn, HashMap as ArenaHashMap, IdentBuildHasher, Vec as ArenaVec,
};
use oxc_index::IndexVec;
use oxc_span::{Ident, Span};
use oxc_syntax::{
    node::NodeId,
    reference::{Reference, ReferenceId},
    scope::{ScopeFlags, ScopeId},
    symbol::{SymbolFlags, SymbolId},
};

use crate::multi_index_vec::multi_index_vec;

pub type Bindings<'a> = ArenaHashMap<'a, ScopingName, SymbolId, IdentBuildHasher>;
pub type UnresolvedReferences<'a> =
    ArenaHashMap<'a, ScopingName, ArenaVec<'a, ReferenceId>, IdentBuildHasher>;

/// A name that [`Scoping`] owns: the string of an [`Ident`].
///
/// It hashes and compares like its `str` (and so like an `Ident` of it), so the maps of [`Scoping`]
/// are searched with a `&str` through `Borrow<str>`. (Upstream oxc kept `Ident`s copied into an
/// arena owned by `Scoping`, which needed a self-referential struct.)
#[derive(Clone)]
pub struct ScopingName {
    name: Box<str>,
}

impl ScopingName {
    fn new(name: &str) -> Self {
        Self { name: name.into() }
    }

    /// The name as a `&str`.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.name
    }

    /// The name as an [`Ident`] borrowing it.
    #[inline]
    pub fn as_ident(&self) -> Ident<'_> {
        Ident::from(&*self.name)
    }
}

impl PartialEq for ScopingName {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for ScopingName {}

impl Hash for ScopingName {
    /// Same as `str`'s and `Ident`'s `Hash`, as `Borrow<str>` requires.
    #[inline]
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.as_str().hash(hasher);
    }
}

impl Borrow<str> for ScopingName {
    #[inline]
    fn borrow(&self) -> &str {
        &self.name
    }
}

impl fmt::Debug for ScopingName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.name, f)
    }
}

impl fmt::Display for ScopingName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&*self.name, f)
    }
}

fn new_bindings() -> Bindings<'static> {
    ArenaHashMap::default()
}

#[derive(Clone, Debug)]
pub struct Redeclaration {
    pub span: Span,
    pub declaration: NodeId,
    pub flags: SymbolFlags,
}

impl CloneIn<'_> for Redeclaration {
    type Cloned = Self;

    #[inline]
    fn clone_in(&self, _allocator: &Allocator) -> Self::Cloned {
        Self { span: self.span, declaration: NodeId::DUMMY, flags: self.flags }
    }

    #[inline]
    fn clone_in_with_semantic_ids(&self, _allocator: &Allocator) -> Self::Cloned {
        self.clone()
    }
}

multi_index_vec! {
    /// Scope tree stored as struct-of-arrays in a single allocation.
    ///
    /// Contains parent IDs, node IDs, and flags for all scopes. Using a single
    /// allocation with one `len`/`cap` instead of 3 separate `IndexVec`s saves
    /// memory (no redundant len/cap) and CPU (one bounds check, one capacity
    /// check on push).
    struct ScopeTable<ScopeId> {
        parent_ids => parent_ids_mut: Option<ScopeId>,
        node_ids => node_ids_mut: NodeId,
        flags => flags_mut: ScopeFlags,
    }
}

multi_index_vec! {
    /// Symbol table stored as struct-of-arrays in a single allocation.
    ///
    /// Contains spans, flags, scope IDs, and declaration node IDs for all symbols.
    /// Using a single allocation with one `len`/`cap` instead of 4 separate `IndexVec`s
    /// saves memory and CPU.
    struct SymbolTable<SymbolId> {
        symbol_spans => symbol_spans_mut: Span,
        symbol_flags => symbol_flags_mut: SymbolFlags,
        symbol_scope_ids => symbol_scope_ids_mut: ScopeId,
        symbol_declarations => symbol_declarations_mut: NodeId,
    }
}

/// # Symbol Table and Scope Tree
///
/// ## Symbol Table
///
/// `SoA` (Struct of Arrays) for memory efficiency.
///
/// Most symbols won't have redeclarations, so instead of storing `Vec<Span>` directly in
/// `redeclare_variables` (32 bytes per symbol), store `Option<RedeclarationId>` (4 bytes).
/// That ID indexes into `redeclarations` where the actual `Vec<Span>` is stored.
///
/// ## Scope Tree
///
/// The scope tree stores lexical scopes created by a program, and all the
/// variable bindings each scope creates.
///
/// - All scopes have a parent scope, except the root scope.
/// - Scopes can have 0 or more child scopes.
/// - Nodes that create a scope store the [`ScopeId`] of the scope they create.
pub struct Scoping {
    /* Symbol Table - single allocation for all symbol-indexed flat fields */
    symbol_table: SymbolTable,

    pub(crate) references: IndexVec<ReferenceId, Reference>,

    /// Function or Variable Symbol IDs that are marked with `@__NO_SIDE_EFFECTS__`.
    pub(crate) no_side_effects: FxHashSet<SymbolId>,

    /* Scope Tree - single allocation for all scope-indexed flat fields */
    scope_table: ScopeTable,

    pub(crate) cell: ScopingInner,
}

impl fmt::Debug for Scoping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("Scoping").finish()
    }
}

impl Default for Scoping {
    fn default() -> Self {
        Self {
            symbol_table: SymbolTable::new(),
            references: IndexVec::new(),
            no_side_effects: FxHashSet::default(),
            scope_table: ScopeTable::new(),
            cell: ScopingInner {
                symbol_names: Vec::new(),
                resolved_references: Vec::new(),
                symbol_redeclarations: FxHashMap::default(),
                bindings: IndexVec::new(),
                root_unresolved_references: ArenaHashMap::default(),
            },
        }
    }
}


/// The 2-dimensional parts of [`Scoping`] (`Vec<Vec<T>>`, `HashMap<Vec<T>>`, `Vec<HashMap<T>>`).
pub struct ScopingInner {
    /* Symbol Table Fields */
    symbol_names: Vec<ScopingName>,
    resolved_references: Vec<ArenaVec<'static, ReferenceId>>,
    /// Redeclarations of a symbol.
    ///
    /// NOTE:
    /// Once a symbol is redeclared, there are at least two entries here. The first
    /// entry is the original symbol information, and the rest are redeclarations.
    /// i.e. `symbol_redeclarations[symbol_id].len() >= 2` always.
    symbol_redeclarations: FxHashMap<SymbolId, ArenaVec<'static, Redeclaration>>,
    /* Scope Tree Fields */
    /// Symbol bindings in a scope.
    ///
    /// A binding is a mapping from an identifier name to its [`SymbolId`]
    pub(crate) bindings: IndexVec<ScopeId, Bindings<'static>>,

    pub(crate) root_unresolved_references: UnresolvedReferences<'static>,
}

// Symbol Table Methods
impl Scoping {
    /// Returns the number of symbols in this table.
    #[inline]
    pub fn symbols_len(&self) -> usize {
        self.symbol_table.len()
    }

    /// Returns `true` if this table contains no symbols.
    #[inline]
    pub fn symbols_is_empty(&self) -> bool {
        self.symbol_table.is_empty()
    }

    /// Iterate all symbol names in insertion order.
    pub fn symbol_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.cell.symbol_names.iter().map(ScopingName::as_str)
    }

    /// Iterate resolved reference ID lists for each symbol.
    pub fn resolved_references(&self) -> impl Iterator<Item = &ArenaVec<'_, ReferenceId>> + '_ {
        self.cell.resolved_references.iter()
    }

    /// Iterate over all symbol IDs in this table.
    ///
    /// Use [`ScopeTree::iter_bindings_in`] to only iterate over symbols declared in a specific
    /// scope.
    ///
    /// [`ScopeTree::iter_bindings_in`]: crate::scoping::Scoping::iter_bindings_in
    pub fn symbol_ids(&self) -> impl Iterator<Item = SymbolId> + '_ {
        self.symbol_table.iter_ids()
    }

    /// Get the [`Span`] of the [`AstNode`] declaring a symbol.
    ///
    /// [`AstNode`]: crate::node::AstNode
    #[inline]
    pub fn symbol_span(&self, symbol_id: SymbolId) -> Span {
        *self.symbol_table.symbol_spans(symbol_id)
    }

    /// Get the identifier name a symbol is bound to.
    #[inline]
    pub fn symbol_name(&self, symbol_id: SymbolId) -> &str {
        self.cell.symbol_names[symbol_id.index()].as_str()
    }

    /// Get the [`Ident`] for the identifier name a symbol is bound to.
    #[inline]
    pub fn symbol_ident(&self, symbol_id: SymbolId) -> Ident<'_> {
        self.cell.symbol_names[symbol_id.index()].as_ident()
    }

    /// Rename a symbol.
    ///
    /// Returns the old name.
    #[inline]
    pub fn set_symbol_name(&mut self, symbol_id: SymbolId, name: Ident<'_>) {
        self.cell.symbol_names[symbol_id.index()] = ScopingName::new(name.as_str());
    }

    /// Get the [`SymbolFlags`] for a symbol, which describe how the symbol is declared.
    ///
    /// To find how a symbol is used, use [`Scoping::get_resolved_references`].
    #[inline]
    pub fn symbol_flags(&self, symbol_id: SymbolId) -> SymbolFlags {
        *self.symbol_table.symbol_flags(symbol_id)
    }

    /// Get a mutable reference to a symbol's [flags](SymbolFlags).
    #[inline]
    pub fn symbol_flags_mut(&mut self, symbol_id: SymbolId) -> &mut SymbolFlags {
        self.symbol_table.symbol_flags_mut(symbol_id)
    }

    #[inline]
    /// Get all redeclarations associated with a symbol.
    pub fn symbol_redeclarations(&self, symbol_id: SymbolId) -> &[Redeclaration] {
        self.cell.symbol_redeclarations.get(&symbol_id).map_or_else(
            || {
                static EMPTY: &[Redeclaration] = &[];
                EMPTY
            },
            |v| v.as_slice(),
        )
    }

    #[inline]
    /// Union additional flags into an existing symbol.
    pub fn union_symbol_flag(&mut self, symbol_id: SymbolId, includes: SymbolFlags) {
        *self.symbol_table.symbol_flags_mut(symbol_id) |= includes;
    }

    #[inline]
    /// Set the scope that owns `symbol_id`.
    pub fn set_symbol_scope_id(&mut self, symbol_id: SymbolId, scope_id: ScopeId) {
        *self.symbol_table.symbol_scope_ids_mut(symbol_id) = scope_id;
    }

    #[inline]
    /// Get the scope that owns `symbol_id`.
    pub fn symbol_scope_id(&self, symbol_id: SymbolId) -> ScopeId {
        *self.symbol_table.symbol_scope_ids(symbol_id)
    }

    /// Get the ID of the AST node declaring a symbol.
    ///
    /// This node will be a [`VariableDeclaration`], [`Function`], or some other AST node
    /// that _has_ a [`BindingIdentifier`] or a [`BindingPattern`]. It will not point to the
    /// binding pattern or identifier node itself.
    ///
    /// [`VariableDeclaration`]: oxc_ast::ast::VariableDeclaration
    /// [`Function`]: oxc_ast::ast::Function
    /// [`BindingIdentifier`]: oxc_ast::ast::BindingIdentifier
    /// [`BindingPattern`]: oxc_ast::ast::BindingPattern
    #[inline]
    pub fn symbol_declaration(&self, symbol_id: SymbolId) -> NodeId {
        *self.symbol_table.symbol_declarations(symbol_id)
    }

    /// Get all declaration node IDs for a symbol.
    ///
    /// Most symbols have a single declaration, but TypeScript allows merged type/value
    /// declarations (e.g. `interface Foo {}` + `const Foo = ...`).
    /// This iterator yields all declaration nodes in those cases.
    #[inline]
    pub fn symbol_declarations(&self, symbol_id: SymbolId) -> impl Iterator<Item = NodeId> + '_ {
        let redeclarations = self.symbol_redeclarations(symbol_id);
        std::iter::once(self.symbol_declaration(symbol_id))
            .filter(move |_| redeclarations.is_empty())
            .chain(redeclarations.iter().map(|redeclaration| redeclaration.declaration))
    }

    /// Create a new symbol and append symbol metadata to the symbol table.
    pub fn create_symbol(
        &mut self,
        span: Span,
        name: Ident<'_>,
        flags: SymbolFlags,
        scope_id: ScopeId,
        node_id: NodeId,
    ) -> SymbolId {
        self.cell.symbol_names.push(ScopingName::new(name.as_str()));
        self.cell.resolved_references.push(ArenaVec::from_std(Vec::new()));
        self.symbol_table.push(span, flags, scope_id, node_id)
    }

    /// Record a redeclaration for an existing symbol.
    pub fn add_symbol_redeclaration(
        &mut self,
        symbol_id: SymbolId,
        flags: SymbolFlags,
        declaration: NodeId,
        span: Span,
    ) {
        let is_first_redeclared =
            !self.cell.symbol_redeclarations.contains_key(&symbol_id);
        let first_declaration = is_first_redeclared.then(|| Redeclaration {
            span: self.symbol_span(symbol_id),
            declaration: self.symbol_declaration(symbol_id),
            flags: self.symbol_flags(symbol_id),
        });

        {
            let redeclaration = Redeclaration { span, declaration, flags };
            match self.cell.symbol_redeclarations.entry(symbol_id) {
                Entry::Occupied(occupied) => {
                    occupied.into_mut().push(redeclaration);
                }
                Entry::Vacant(vacant) => {
                    let first_declaration = first_declaration.unwrap_or_else(|| {
                        unreachable!(
                            "The above step has already been checked, and it was first declared."
                        )
                    });
                    let v = ArenaVec::from_std(vec![first_declaration, redeclaration]);
                    vacant.insert(v);
                }
            }
        }
    }

    #[inline]
    /// Create and store a reference entry.
    pub fn create_reference(&mut self, reference: Reference) -> ReferenceId {
        self.references.push(reference)
    }

    /// Get a resolved or unresolved reference.
    ///
    /// [`ReferenceId`]s can be found in [oxc_ast::ast::IdentifierReference] and similar nodes.
    #[inline]
    pub fn get_reference(&self, reference_id: ReferenceId) -> &Reference {
        &self.references[reference_id]
    }

    #[inline]
    /// Get mutable access to a reference.
    pub fn get_reference_mut(&mut self, reference_id: ReferenceId) -> &mut Reference {
        &mut self.references[reference_id]
    }

    /// Get the name of the symbol a reference is resolved to. Returns `None` if the reference is
    /// not resolved.
    #[inline]
    pub fn get_reference_name(&self, reference_id: ReferenceId) -> Option<&str> {
        self.symbol_name(self.references[reference_id].symbol_id()?).into()
    }

    /// Returns `true` if the corresponding [`Reference`] is resolved to a symbol.
    ///
    /// When `false`, this could either be a reference to a global value or an identifier that does
    /// not exist.
    #[inline]
    pub fn has_binding(&self, reference_id: ReferenceId) -> bool {
        self.references[reference_id].symbol_id().is_some()
    }

    /// Find [`Reference`] ids resolved to a symbol.
    ///
    /// If you want direct access to a symbol's [`Reference`]s, use [`Scoping::get_resolved_references`].
    #[inline]
    pub fn get_resolved_reference_ids(&self, symbol_id: SymbolId) -> &[ReferenceId] {
        &self.cell.resolved_references[symbol_id.index()]
    }

    /// Find [`Reference`]s resolved to a symbol.
    pub fn get_resolved_references(
        &self,
        symbol_id: SymbolId,
    ) -> impl DoubleEndedIterator<Item = &Reference> + '_ {
        self.get_resolved_reference_ids(symbol_id)
            .iter()
            .map(|&reference_id| &self.references[reference_id])
    }

    /// Get whether a symbol is mutated (i.e. assigned to).
    ///
    /// If symbol is `const`, always returns `false`.
    /// Otherwise, returns `true` if the symbol is assigned to somewhere in AST.
    pub fn symbol_is_mutated(&self, symbol_id: SymbolId) -> bool {
        if self.symbol_table.symbol_flags(symbol_id).contains(SymbolFlags::ConstVariable) {
            false
        } else {
            self.get_resolved_references(symbol_id).any(Reference::is_write)
        }
    }

    /// Get whether a symbol is unused (i.e. not read or written after declaration).
    pub fn symbol_is_unused(&self, symbol_id: SymbolId) -> bool {
        self.get_resolved_reference_ids(symbol_id).is_empty()
    }

    /// Add a reference to a symbol.
    pub fn add_resolved_reference(&mut self, symbol_id: SymbolId, reference_id: ReferenceId) {
        self.cell.resolved_references[symbol_id.index()].push(reference_id);
    }

    /// Delete a reference.
    pub fn delete_reference(&mut self, reference_id: ReferenceId) {
        let Some(symbol_id) = self.get_reference(reference_id).symbol_id() else { return };
        self.delete_resolved_reference(symbol_id, reference_id);
    }

    /// Delete a reference to a symbol.
    ///
    /// # Panics
    /// Panics if provided `reference_id` is not a resolved reference for `symbol_id`.
    pub fn delete_resolved_reference(&mut self, symbol_id: SymbolId, reference_id: ReferenceId) {
        let reference_ids = &mut self.cell.resolved_references[symbol_id.index()];
        let index = reference_ids.iter().position(|&id| id == reference_id).unwrap();
        reference_ids.swap_remove(index);
    }

    /// Reserve additional capacity for symbols, references, and scopes.
    pub fn reserve(
        &mut self,
        additional_symbols: usize,
        additional_references: usize,
        additional_scopes: usize,
    ) {
        self.symbol_table.reserve(additional_symbols);
        self.cell.symbol_names.reserve_exact(additional_symbols);
        self.cell.resolved_references.reserve_exact(additional_symbols);
        self.references.reserve(additional_references);

        self.scope_table.reserve(additional_scopes);
        self.cell.bindings.reserve(additional_scopes);
    }

    /// Symbols marked with `@__NO_SIDE_EFFECTS__`.
    pub fn no_side_effects(&self) -> &FxHashSet<SymbolId> {
        &self.no_side_effects
    }
}

/// Scope Tree Methods
impl Scoping {
    const ROOT_SCOPE_ID: ScopeId = ScopeId::new(0);

    /// Returns the number of scopes found in the program. Includes the root
    /// program scope.
    #[inline]
    pub fn scopes_len(&self) -> usize {
        self.scope_table.len()
    }

    /// Returns `true` if there are no scopes in the program.
    ///
    /// This will always return `false` when semantic analysis has completed
    /// since there is a root scope.
    #[inline]
    pub fn scopes_is_empty(&self) -> bool {
        self.scope_table.is_empty()
    }

    /// Iterate over the scopes that contain a scope.
    ///
    /// The first element of this iterator will be the scope itself. This
    /// guarantees the iterator will have at least 1 element.
    pub fn scope_ancestors(&self, scope_id: ScopeId) -> impl Iterator<Item = ScopeId> + '_ {
        std::iter::successors(Some(scope_id), |&scope_id| *self.scope_table.parent_ids(scope_id))
    }

    /// Iterate all scope IDs from the root scope through all descendants.
    pub fn scope_descendants_from_root(&self) -> impl Iterator<Item = ScopeId> + '_ {
        self.scope_table.iter_ids()
    }

    /// Get the root [`Program`] scope id.
    ///
    /// [`Program`]: oxc_ast::ast::Program
    #[expect(clippy::unused_self)]
    #[inline]
    pub const fn root_scope_id(&self) -> ScopeId {
        Self::ROOT_SCOPE_ID
    }

    /// Get the flags for the root scope.
    ///
    /// This is a shorthand for `scope.get_flags(scope.root_scope_id())`.
    #[inline]
    pub fn root_scope_flags(&self) -> ScopeFlags {
        *self.scope_table.flags(self.root_scope_id())
    }

    #[inline]
    /// Unresolved references recorded in the root scope.
    pub fn root_unresolved_references(&self) -> &UnresolvedReferences<'_> {
        &self.cell.root_unresolved_references
    }

    /// Iterate unresolved reference IDs grouped by unresolved identifier name.
    pub fn root_unresolved_references_ids(
        &self,
    ) -> impl Iterator<Item = impl Iterator<Item = ReferenceId> + '_> + '_ {
        self.cell.root_unresolved_references.values().map(|v| v.iter().copied())
    }

    /// Delete an unresolved reference.
    ///
    /// If the `ReferenceId` provided is only reference remaining for this unresolved reference
    /// (i.e. this `x` was last `x` in the AST), deletes the key from `root_unresolved_references`.
    ///
    /// # Panics
    /// Panics if there is no unresolved reference for provided `name` and `reference_id`.
    #[inline]
    pub fn delete_root_unresolved_reference(&mut self, name: Ident<'_>, reference_id: ReferenceId) {
        let cell = &mut self.cell;
        let reference_ids = cell.root_unresolved_references.get_mut(name.as_str()).unwrap();
        if reference_ids.len() == 1 {
            assert_eq!(reference_ids[0], reference_id);
            cell.root_unresolved_references.remove(name.as_str());
        } else {
            let index = reference_ids.iter().position(|&id| id == reference_id).unwrap();
            reference_ids.swap_remove(index);
        }
    }

    #[inline]
    /// Get scope flags for `scope_id`.
    pub fn scope_flags(&self, scope_id: ScopeId) -> ScopeFlags {
        *self.scope_table.flags(scope_id)
    }

    #[inline]
    /// Get mutable scope flags for `scope_id`.
    pub fn scope_flags_mut(&mut self, scope_id: ScopeId) -> &mut ScopeFlags {
        self.scope_table.flags_mut(scope_id)
    }

    /// Get [`ScopeFlags`] for a new child scope under `parent_scope_id`.
    pub fn get_new_scope_flags(&self, flags: ScopeFlags, parent_scope_id: ScopeId) -> ScopeFlags {
        // https://tc39.es/ecma262/#sec-strict-mode-code
        flags | self.scope_flags(parent_scope_id) & ScopeFlags::StrictMode
    }

    #[inline]
    /// Get the parent scope ID, if any.
    pub fn scope_parent_id(&self, scope_id: ScopeId) -> Option<ScopeId> {
        *self.scope_table.parent_ids(scope_id)
    }

    /// Set the parent scope ID.
    pub fn set_scope_parent_id(&mut self, scope_id: ScopeId, parent_id: Option<ScopeId>) {
        *self.scope_table.parent_ids_mut(scope_id) = parent_id;
    }

    /// Change the parent scope of a scope.
    ///
    /// This will also remove the scope from the child list of the old parent and add it to the new parent.
    pub fn change_scope_parent_id(&mut self, scope_id: ScopeId, new_parent_id: Option<ScopeId>) {
        *self.scope_table.parent_ids_mut(scope_id) = new_parent_id;
    }

    /// Get a variable binding by name that was declared in the top-level scope
    #[inline]
    pub fn get_root_binding(&self, name: Ident<'_>) -> Option<SymbolId> {
        self.get_binding(self.root_scope_id(), name)
    }

    /// Add an unresolved reference to the root scope map.
    pub fn add_root_unresolved_reference(&mut self, name: Ident<'_>, reference_id: ReferenceId) {
        let roots = &mut self.cell.root_unresolved_references;
        if let Some(reference_ids) = roots.get_mut(name.as_str()) {
            reference_ids.push(reference_id);
        } else {
            roots.insert(
                ScopingName::new(name.as_str()),
                ArenaVec::from_std(vec![reference_id]),
            );
        }
    }

    /// Check if a symbol is declared in a certain scope.
    pub fn scope_has_binding(&self, scope_id: ScopeId, name: Ident<'_>) -> bool {
        self.cell.bindings[scope_id].contains_key(name.as_str())
    }

    /// Get the symbol bound to an identifier name in a scope.
    ///
    /// Returns [`None`] if that name is not bound in the scope. This could be
    /// because the symbol is not declared within this tree, but it could also
    /// be because its declaration is in a parent scope. If you want to find a
    /// binding that might be declared in a parent scope, use [`find_binding`].
    ///
    /// [`find_binding`]: Scoping::find_binding
    pub fn get_binding(&self, scope_id: ScopeId, name: Ident<'_>) -> Option<SymbolId> {
        self.cell.bindings[scope_id].get(name.as_str()).copied()
    }

    /// Find a binding by name in a scope or its ancestors.
    ///
    /// Bindings are resolved by walking up the scope tree until a binding is
    /// found. If no binding is found, [`None`] is returned.
    pub fn find_binding(&self, mut scope_id: ScopeId, name: Ident<'_>) -> Option<SymbolId> {
        let cell = &self.cell;
        loop {
            if let Some(symbol_id) = cell.bindings[scope_id].get(name.as_str()) {
                return Some(*symbol_id);
            }
            let parent_scope_id = (*self.scope_table.parent_ids(scope_id))?;
            scope_id = parent_scope_id;
        }
    }

    /// Get all bound identifiers in a scope.
    #[inline]
    pub fn get_bindings(&self, scope_id: ScopeId) -> &Bindings<'_> {
        &self.cell.bindings[scope_id]
    }

    /// Get the ID of the [`AstNode`] that created a scope.
    ///
    /// [`AstNode`]: crate::AstNode
    #[inline]
    pub fn get_node_id(&self, scope_id: ScopeId) -> NodeId {
        *self.scope_table.node_ids(scope_id)
    }

    /// Iterate over all bindings declared in the entire program.
    ///
    /// If you only want bindings in a specific scope, use [`iter_bindings_in`].
    ///
    /// [`iter_bindings_in`]: Scoping::iter_bindings_in
    pub fn iter_bindings(&self) -> impl Iterator<Item = (ScopeId, &Bindings<'_>)> + '_ {
        self.cell.bindings.iter_enumerated()
    }

    /// Iterate over bindings declared inside a scope.
    #[inline]
    pub fn iter_bindings_in(&self, scope_id: ScopeId) -> impl Iterator<Item = SymbolId> + '_ {
        self.cell.bindings[scope_id].values().copied()
    }

    #[inline]
    pub(crate) fn insert_binding(
        &mut self,
        scope_id: ScopeId,
        name: Ident<'_>,
        symbol_id: SymbolId,
    ) {
        self.cell.bindings[scope_id].insert(ScopingName::new(name.as_str()), symbol_id);
    }

    /// Create a scope.
    #[inline]
    pub fn add_scope(
        &mut self,
        parent_id: Option<ScopeId>,
        node_id: NodeId,
        flags: ScopeFlags,
    ) -> ScopeId {
        let scope_id = self.scope_table.push(parent_id, node_id, flags);
        self.cell.bindings.push(new_bindings());
        scope_id
    }

    /// Add a binding to a scope.
    pub fn add_binding(&mut self, scope_id: ScopeId, name: Ident<'_>, symbol_id: SymbolId) {
        self.cell.bindings[scope_id].insert(ScopingName::new(name.as_str()), symbol_id);
    }

    /// Remove an existing binding from a scope.
    pub fn remove_binding(&mut self, scope_id: ScopeId, name: Ident<'_>) {
        self.cell.bindings[scope_id].remove(name.as_str());
    }

    /// Move a binding from one scope to another.
    pub fn move_binding(&mut self, from: ScopeId, to: ScopeId, name: Ident<'_>) {
        let bindings = &mut self.cell.bindings;
        if let Some((name, symbol_id)) = bindings[from].remove_entry(name.as_str()) {
            bindings[to].insert(name, symbol_id);
        }
    }

    /// Rename a binding to a new name.
    ///
    /// The following must be true for successful operation:
    /// * Binding exists in specified scope for `old_name`.
    /// * Existing binding is for specified `symbol_id`.
    /// * No binding already exists for `new_name`.
    ///
    /// Panics in debug mode if any of the above are not true.
    pub fn rename_binding(
        &mut self,
        scope_id: ScopeId,
        symbol_id: SymbolId,
        old_name: Ident<'_>,
        new_name: Ident<'_>,
    ) {
        let bindings = &mut self.cell.bindings[scope_id];
        let old_symbol_id = bindings.remove(old_name.as_str());
        debug_assert_eq!(old_symbol_id, Some(symbol_id));
        let existing_symbol_id = bindings.insert(ScopingName::new(new_name.as_str()), symbol_id);
        debug_assert!(existing_symbol_id.is_none());
    }

    /// Rename symbol.
    ///
    /// The following must be true for successful operation:
    /// * Binding exists in specified scope for `symbol_id`.
    /// * No binding already exists in scope for `new_name`.
    ///
    /// Panics in debug mode if either of the above are not satisfied.
    pub fn rename_symbol(&mut self, symbol_id: SymbolId, scope_id: ScopeId, new_name: Ident<'_>) {
        let cell = &mut self.cell;
        let new_name = ScopingName::new(new_name.as_str());
        let old_name = mem::replace(&mut cell.symbol_names[symbol_id.index()], new_name.clone());

        let bindings = &mut cell.bindings[scope_id];
        let old_symbol_id = bindings.remove(old_name.as_str());
        debug_assert_eq!(old_symbol_id, Some(symbol_id));
        let existing_symbol_id = bindings.insert(new_name, symbol_id);
        debug_assert!(existing_symbol_id.is_none());
    }

    /// Remove bindings that exist only in TypeScript type space.
    pub fn delete_typescript_bindings(&mut self) {
        let symbol_table = &self.symbol_table;
        for bindings in &mut self.cell.bindings {
            bindings.retain(|_name, symbol_id| {
                let flags = *symbol_table.symbol_flags(*symbol_id);
                !flags.intersects(
                    SymbolFlags::TypeAlias | SymbolFlags::Interface | SymbolFlags::TypeParameter,
                )
            });
        }
    }

    /// Move the bindings of a catch clause's parameter scope to its body scope, so that resolving
    /// references and checking redeclarations sees them there. Returns the moved symbols, or
    /// `None` if there were none.
    pub(crate) fn move_catch_param_bindings(
        &mut self,
        param_scope_id: ScopeId,
        body_scope_id: ScopeId,
    ) -> Option<Vec<SymbolId>> {
        let bindings = &mut self.cell.bindings;
        if bindings[param_scope_id].is_empty() {
            return None;
        }
        let param_bindings = mem::replace(&mut bindings[param_scope_id], new_bindings());
        let symbol_ids = param_bindings.values().copied().collect::<Vec<_>>();
        bindings[body_scope_id] = param_bindings;
        Some(symbol_ids)
    }
}

impl Scoping {
    /// Clone all semantic data. Used in `Rolldown`.
    #[must_use]
    pub fn clone_in_with_semantic_ids_with_another_arena(&self) -> Self {
        let cell = &self.cell;
        Self {
            symbol_table: self.symbol_table.clone(),
            references: self.references.clone(),
            no_side_effects: self.no_side_effects.clone(),
            scope_table: self.scope_table.clone(),
            cell: ScopingInner {
                symbol_names: cell.symbol_names.clone(),
                resolved_references: cell
                    .resolved_references
                    .iter()
                    .map(|ids| ArenaVec::from_std(ids.to_vec()))
                    .collect(),
                symbol_redeclarations: cell
                    .symbol_redeclarations
                    .iter()
                    .map(|(k, v)| (*k, ArenaVec::from_std(v.to_vec())))
                    .collect(),
                bindings: cell.bindings.iter().map(|map| clone_map(map, |&id| id)).collect(),
                root_unresolved_references: clone_map(&cell.root_unresolved_references, |ids| {
                    ArenaVec::from_std(ids.to_vec())
                }),
            },
        }
    }
}

fn clone_map<V>(
    map: &ArenaHashMap<'static, ScopingName, V, IdentBuildHasher>,
    clone_value: impl Fn(&V) -> V,
) -> ArenaHashMap<'static, ScopingName, V, IdentBuildHasher> {
    let mut cloned = ArenaHashMap::default();
    cloned.reserve(map.len());
    for (name, value) in map.iter() {
        cloned.insert(name.clone(), clone_value(value));
    }
    cloned
}

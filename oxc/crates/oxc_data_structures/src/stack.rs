//! Contains the following FILO data structures, all on top of [`Vec`]:
//!
//! * [`Stack`]: A growable stack (push/pop).
//! * [`NonEmptyStack`]: A growable stack that can never be empty, so `last` / `last_mut` need no `Option`.
//! * [`SparseStack`]: A growable stack of `Option`s, storing only the filled entries.
//!
//! The `*_unchecked` methods keep their names but check: they panic where the old ones were
//! undefined behaviour.

use std::{
    error::Error,
    fmt::{self, Debug, Display},
    ops::{Deref, DerefMut},
};

const fn max_capacity<T>() -> usize {
    if size_of::<T>() == 0 { usize::MAX } else { isize::MAX as usize / size_of::<T>() }
}

/// A growable stack, equivalent to [`Vec`].
#[derive(Debug, Clone)]
pub struct Stack<T> {
    vec: Vec<T>,
}

impl<T> Default for Stack<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Stack<T> {
    /// Maximum capacity.
    pub const MAX_CAPACITY: usize = max_capacity::<T>();

    /// Create new empty [`Stack`].
    #[inline]
    pub const fn new() -> Self {
        Self { vec: Vec::new() }
    }

    /// Create new [`Stack`] with pre-allocated capacity.
    ///
    /// # Panics
    /// Panics if `capacity` exceeds [`Self::MAX_CAPACITY`].
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity <= Self::MAX_CAPACITY, "`capacity` cannot exceed `Self::MAX_CAPACITY`");
        Self { vec: Vec::with_capacity(capacity) }
    }

    /// Same as [`Stack::with_capacity`].
    #[inline]
    pub fn with_capacity_unchecked(capacity: usize) -> Self {
        Self::with_capacity(capacity)
    }

    /// Get reference to last value on stack.
    #[inline]
    pub fn last(&self) -> Option<&T> {
        self.vec.last()
    }

    /// Get reference to last value on stack.
    ///
    /// # Panics
    /// Panics if the stack is empty.
    #[inline]
    pub fn last_unchecked(&self) -> &T {
        self.vec.last().expect("stack is not empty")
    }

    /// Get mutable reference to last value on stack.
    #[inline]
    pub fn last_mut(&mut self) -> Option<&mut T> {
        self.vec.last_mut()
    }

    /// Get mutable reference to last value on stack.
    ///
    /// # Panics
    /// Panics if the stack is empty.
    #[inline]
    pub fn last_unchecked_mut(&mut self) -> &mut T {
        self.vec.last_mut().expect("stack is not empty")
    }

    /// Push value to stack.
    #[inline]
    pub fn push(&mut self, value: T) {
        self.vec.push(value);
    }

    /// Pop value from stack.
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        self.vec.pop()
    }

    /// Pop value from stack.
    ///
    /// # Panics
    /// Panics if the stack is empty.
    #[inline]
    pub fn pop_unchecked(&mut self) -> T {
        self.vec.pop().expect("stack is not empty")
    }

    /// Clear contents of stack.
    #[inline]
    pub fn clear(&mut self) {
        self.vec.clear();
    }

    /// Get number of entries on stack.
    #[inline]
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    /// Get if stack is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }

    /// Get capacity.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.vec.capacity()
    }

    /// Get contents of stack as a slice `&[T]`.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.vec
    }

    /// Get contents of stack as a mutable slice `&mut [T]`.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.vec
    }

    /// Convert into a [`Vec`].
    #[inline]
    pub fn into_vec(self) -> Vec<T> {
        self.vec
    }
}

impl<T> Deref for Stack<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        &self.vec
    }
}

impl<T> DerefMut for Stack<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.vec
    }
}

impl<T> From<Vec<T>> for Stack<T> {
    #[inline]
    fn from(vec: Vec<T>) -> Self {
        Self { vec }
    }
}

impl<T> From<Stack<T>> for Vec<T> {
    #[inline]
    fn from(stack: Stack<T>) -> Self {
        stack.vec
    }
}

impl<T> FromIterator<T> for Stack<T> {
    #[inline]
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self { vec: Vec::from_iter(iter) }
    }
}

/// A growable stack which always holds at least one entry.
///
/// It is created with an initial entry, which cannot be popped off.
#[derive(Debug, Clone)]
pub struct NonEmptyStack<T> {
    vec: Vec<T>,
}

impl<T> NonEmptyStack<T> {
    /// Maximum capacity.
    pub const MAX_CAPACITY: usize = max_capacity::<T>();

    /// Create new [`NonEmptyStack`] holding `initial_value`.
    #[inline]
    pub fn new(initial_value: T) -> Self {
        Self { vec: vec![initial_value] }
    }

    /// Create new [`NonEmptyStack`] with pre-allocated capacity, holding `initial_value`.
    ///
    /// # Panics
    /// Panics if `capacity` is 0 or exceeds [`Self::MAX_CAPACITY`].
    #[inline]
    pub fn with_capacity(capacity: usize, initial_value: T) -> Self {
        assert!(capacity != 0, "`capacity` cannot be zero");
        assert!(capacity <= Self::MAX_CAPACITY, "`capacity` cannot exceed `Self::MAX_CAPACITY`");
        let mut vec = Vec::with_capacity(capacity);
        vec.push(initial_value);
        Self { vec }
    }

    /// Same as [`NonEmptyStack::with_capacity`].
    #[inline]
    pub fn with_capacity_unchecked(capacity: usize, initial_value: T) -> Self {
        Self::with_capacity(capacity, initial_value)
    }

    /// Get reference to first value on stack.
    #[inline]
    pub fn first(&self) -> &T {
        self.vec.first().expect("NonEmptyStack is never empty")
    }

    /// Get mutable reference to first value on stack.
    #[inline]
    pub fn first_mut(&mut self) -> &mut T {
        self.vec.first_mut().expect("NonEmptyStack is never empty")
    }

    /// Get reference to last value on stack.
    #[inline]
    pub fn last(&self) -> &T {
        self.vec.last().expect("NonEmptyStack is never empty")
    }

    /// Get mutable reference to last value on stack.
    #[inline]
    pub fn last_mut(&mut self) -> &mut T {
        self.vec.last_mut().expect("NonEmptyStack is never empty")
    }

    /// Push value to stack.
    #[inline]
    pub fn push(&mut self, value: T) {
        self.vec.push(value);
    }

    /// Pop value from stack.
    ///
    /// # Panics
    /// Panics if the stack has only 1 entry on it.
    #[inline]
    pub fn pop(&mut self) -> T {
        assert!(self.vec.len() > 1, "Cannot pop all entries");
        self.vec.pop().expect("NonEmptyStack is never empty")
    }

    /// Same as [`NonEmptyStack::pop`].
    ///
    /// # Panics
    /// Panics if the stack has only 1 entry on it.
    #[inline]
    pub fn pop_unchecked(&mut self) -> T {
        self.pop()
    }

    /// Get number of values on stack. Always at least 1.
    #[inline]
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    /// Get if stack is empty.
    ///
    /// This method is pointless, as the stack is never empty.
    /// Only present to override the default method from `[T]::is_empty` which is inherited via `Deref`.
    /// Using it is a compile-time error. Use [`is_exhausted`] instead.
    ///
    /// [`is_exhausted`]: Self::is_exhausted
    #[expect(clippy::unused_self)]
    #[inline]
    pub fn is_empty(&self) -> bool {
        const {
            panic!("`NonEmptyStack` is never empty. Use `is_exhausted` instead.");
        }
    }

    /// Get if stack is back in its initial state (only the initial entry is left).
    #[inline]
    pub fn is_exhausted(&self) -> bool {
        self.vec.len() == 1
    }

    /// Get capacity.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.vec.capacity()
    }

    /// Get contents of stack as a slice `&[T]`.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.vec
    }

    /// Get contents of stack as a mutable slice `&mut [T]`.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.vec
    }

    /// Convert into a [`Vec`].
    #[inline]
    pub fn into_vec(self) -> Vec<T> {
        self.vec
    }
}

impl<T> Deref for NonEmptyStack<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        &self.vec
    }
}

impl<T> DerefMut for NonEmptyStack<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.vec
    }
}

impl<T: Default> Default for NonEmptyStack<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// Error returned when converting an empty [`Vec`] into a [`NonEmptyStack`].
pub struct TryFromVecError;

impl Error for TryFromVecError {}

impl Display for TryFromVecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Empty Vec")
    }
}

impl Debug for TryFromVecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TryFromVecError: Empty Vec")
    }
}

impl<T> TryFrom<Vec<T>> for NonEmptyStack<T> {
    type Error = TryFromVecError;

    #[inline]
    fn try_from(vec: Vec<T>) -> Result<Self, Self::Error> {
        if vec.is_empty() { Err(TryFromVecError) } else { Ok(Self { vec }) }
    }
}

impl<T> From<NonEmptyStack<T>> for Vec<T> {
    #[inline]
    fn from(stack: NonEmptyStack<T>) -> Self {
        stack.vec
    }
}

impl<T> FromIterator<T> for NonEmptyStack<T> {
    /// # Panics
    /// Panics if the iterator is empty.
    #[inline]
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::try_from(Vec::from_iter(iter)).expect("Cannot create `NonEmptyStack` from empty iterator")
    }
}

/// Stack which is sparsely filled: a [`NonEmptyStack`] of `Option<T>` that stores only the
/// filled entries.
///
/// Stack is initialized with a single empty entry which can never be popped off.
pub struct SparseStack<T> {
    has_values: NonEmptyStack<bool>,
    values: Stack<T>,
}

impl<T> Default for SparseStack<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> SparseStack<T> {
    /// Maximum capacity for filled entries (`Some`).
    pub const MAX_FILLED_CAPACITY: usize = Stack::<T>::MAX_CAPACITY;

    /// Maximum capacity for entries (either `Some` or `None`).
    pub const MAX_TOTAL_CAPACITY: usize = NonEmptyStack::<bool>::MAX_CAPACITY;

    /// Create new `SparseStack`.
    pub fn new() -> Self {
        Self { has_values: NonEmptyStack::new(false), values: Stack::new() }
    }

    /// Create new `SparseStack` with pre-allocated capacity.
    ///
    /// # Panics
    /// Panics if `total_capacity` is 0 or a capacity exceeds its maximum.
    #[inline]
    pub fn with_capacity(total_capacity: usize, filled_capacity: usize) -> Self {
        Self {
            has_values: NonEmptyStack::with_capacity(total_capacity, false),
            values: Stack::with_capacity(filled_capacity),
        }
    }

    /// Push an entry to the stack.
    #[inline]
    pub fn push(&mut self, value: Option<T>) {
        let has_value = match value {
            Some(value) => {
                self.values.push(value);
                true
            }
            None => false,
        };
        self.has_values.push(has_value);
    }

    /// Pop last entry from the stack.
    ///
    /// # Panics
    /// Panics if the stack has only 1 entry on it.
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.has_values.pop() { Some(self.values.pop_unchecked()) } else { None }
    }

    /// Get reference to value of first entry on the stack.
    #[inline]
    pub fn first(&self) -> Option<&T> {
        if *self.has_values.first() { self.values.first() } else { None }
    }

    /// Get mutable reference to value of first entry on the stack.
    #[inline]
    pub fn first_mut(&mut self) -> Option<&mut T> {
        if *self.has_values.first() { self.values.first_mut() } else { None }
    }

    /// Get reference to value of last entry on the stack.
    #[inline]
    pub fn last(&self) -> Option<&T> {
        if *self.has_values.last() { Some(self.values.last_unchecked()) } else { None }
    }

    /// Get mutable reference to value of last entry on the stack.
    #[inline]
    pub fn last_mut(&mut self) -> Option<&mut T> {
        if *self.has_values.last() { Some(self.values.last_unchecked_mut()) } else { None }
    }

    /// Take value from last entry on the stack, leaving last entry empty.
    #[inline]
    pub fn take_last(&mut self) -> Option<T> {
        let has_value = self.has_values.last_mut();
        if *has_value {
            *has_value = false;
            Some(self.values.pop_unchecked())
        } else {
            None
        }
    }

    /// Initialize the value for last entry on the stack, if it has no value already.
    /// Return reference to value.
    #[inline]
    pub fn last_or_init<I: FnOnce() -> T>(&mut self, init: I) -> &T {
        self.last_mut_or_init(init)
    }

    /// Initialize the value for last entry on the stack, if it has no value already.
    /// Return mutable reference to value.
    #[inline]
    pub fn last_mut_or_init<I: FnOnce() -> T>(&mut self, init: I) -> &mut T {
        let has_value = self.has_values.last_mut();
        if !*has_value {
            *has_value = true;
            self.values.push(init());
        }
        self.values.last_unchecked_mut()
    }

    /// Get number of entries on the stack. Always at least 1.
    #[inline]
    pub fn len(&self) -> usize {
        self.has_values.len()
    }

    /// Get if stack is back in its initial state (only the initial entry is left).
    #[inline]
    pub fn is_exhausted(&self) -> bool {
        self.has_values.is_exhausted()
    }

    /// Get number of filled entries on the stack.
    #[inline]
    pub fn filled_len(&self) -> usize {
        self.values.len()
    }

    /// Get capacity of stack for any entries (either `Some` or `None`).
    #[inline]
    pub fn total_capacity(&self) -> usize {
        self.has_values.capacity()
    }

    /// Get capacity of stack for filled entries (`Some`).
    #[inline]
    pub fn filled_capacity(&self) -> usize {
        self.values.capacity()
    }

    /// Get filled entries of stack as a slice `&[T]`.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        self.values.as_slice()
    }

    /// Get filled entries of stack as a mutable slice `&mut [T]`.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.values.as_mut_slice()
    }
}

#[cfg(test)]
mod test {
    use super::{NonEmptyStack, SparseStack, Stack};

    #[test]
    fn stack_push_pop() {
        let mut stack = Stack::new();
        assert!(stack.is_empty());
        stack.push(1);
        stack.push(2);
        assert_eq!(stack.last(), Some(&2));
        *stack.last_unchecked_mut() = 3;
        assert_eq!(stack.as_slice(), &[1, 3]);
        assert_eq!(stack.pop(), Some(3));
        assert_eq!(stack.pop_unchecked(), 1);
        assert_eq!(stack.pop(), None);
        assert_eq!(Stack::from(vec![4, 5]).into_vec(), vec![4, 5]);
    }

    #[test]
    fn non_empty_stack() {
        let mut stack = NonEmptyStack::new(0);
        assert!(stack.is_exhausted());
        stack.push(1);
        stack.push(2);
        assert_eq!(*stack.first(), 0);
        assert_eq!(*stack.last(), 2);
        *stack.last_mut() = 3;
        assert_eq!(stack.pop(), 3);
        assert_eq!(stack.pop_unchecked(), 1);
        assert!(stack.is_exhausted());
        assert_eq!(stack.len(), 1);
        assert!(NonEmptyStack::try_from(Vec::<u8>::new()).is_err());
    }

    #[test]
    #[should_panic(expected = "Cannot pop all entries")]
    fn non_empty_stack_pop_panic() {
        let mut stack = NonEmptyStack::new(0);
        stack.pop();
    }

    #[test]
    #[should_panic(expected = "`capacity` cannot be zero")]
    fn non_empty_stack_with_capacity_zero() {
        NonEmptyStack::with_capacity(0, 10u64);
    }

    #[test]
    fn sparse_stack() {
        let mut stack = SparseStack::new();
        assert_eq!(stack.last(), None);
        stack.push(Some(1));
        stack.push(None);
        assert_eq!(stack.last(), None);
        assert_eq!(*stack.last_or_init(|| 2), 2);
        assert_eq!(stack.filled_len(), 2);
        assert_eq!(stack.take_last(), Some(2));
        assert_eq!(stack.pop(), None);
        assert_eq!(stack.last_mut(), Some(&mut 1));
        assert_eq!(stack.pop(), Some(1));
        assert!(stack.is_exhausted());
        assert_eq!(stack.first(), None);
        assert_eq!(stack.len(), 1);
    }
}

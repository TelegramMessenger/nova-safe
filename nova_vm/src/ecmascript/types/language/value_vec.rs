// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::marker::PhantomData;

use crate::{
    ecmascript::{Agent, Value},
    engine::{Bindable, HeapRootCollection, NoGcScope, ScopableCollection, ScopedCollection},
};

impl ScopableCollection for Vec<Value<'_>> {
    fn scope<'scope>(
        self,
        agent: &Agent,
        gc: NoGcScope<'_, 'scope>,
    ) -> ScopedCollection<'scope, Self::Of<'static>> {
        ScopedCollection::new(agent, self.unbind(), gc)
    }
}

impl ScopedCollection<'_, Vec<Value<'static>>> {
    fn with_cb<R>(&self, agent: &Agent, f: impl FnOnce(&Vec<Value<'static>>) -> R) -> R {
        let stack_ref_collections = agent.stack_ref_collections.borrow();
        let Some(stack_slot) = stack_ref_collections.get(self.inner as usize) else {
            unreachable!();
        };
        let HeapRootCollection::ValueVec(value_vec) = stack_slot else {
            unreachable!()
        };
        f(value_vec)
    }

    fn with_cb_mut<R>(
        &mut self,
        agent: &Agent,
        f: impl FnOnce(&mut Vec<Value<'static>>) -> R,
    ) -> R {
        let mut stack_ref_collections = agent.stack_ref_collections.borrow_mut();
        let Some(stack_slot) = stack_ref_collections.get_mut(self.inner as usize) else {
            unreachable!();
        };
        let HeapRootCollection::ValueVec(value_vec) = stack_slot else {
            unreachable!()
        };
        f(value_vec)
    }

    /// Push a Value into the scoped Vec.
    pub fn push(&mut self, agent: &Agent, value: Value) {
        self.with_cb_mut(agent, |value_vec| value_vec.push(value.unbind()));
    }

    /// Pop a Value from the scoped Vec.
    pub fn pop<'a>(&mut self, agent: &Agent, gc: NoGcScope<'a, '_>) -> Option<Value<'a>> {
        self.with_cb_mut(agent, |value_vec| value_vec.pop().bind(gc))
    }

    /// Copy the last Value from the scoped Vec if not empty.
    pub fn last<'a>(&self, agent: &Agent, gc: NoGcScope<'a, '_>) -> Option<Value<'a>> {
        self.with_cb(agent, |value_vec| {
            value_vec.last().map(|value| value.bind(gc))
        })
    }

    /// Returns `true` if the scoped Vec contains a Value.
    pub fn contains(&self, agent: &Agent, value: Value) -> bool {
        self.with_cb(agent, |value_vec| value_vec.contains(&value.unbind()))
    }

    pub(crate) fn iter(&self, agent: &mut Agent) -> ScopedValuesIterator<'_> {
        self.with_cb(agent, |value_vec| {
            ScopedValuesIterator::new(self.inner, 0, value_vec.len())
        })
    }

    /// Returns `true` if the scoped Vec is empty.
    pub fn is_empty(&self, agent: &Agent) -> bool {
        self.with_cb(agent, |value_vec| value_vec.is_empty())
    }

    /// Returns the scoped Vec's length.
    pub fn len(&self, agent: &Agent) -> usize {
        self.with_cb(agent, |value_vec| value_vec.len())
    }
}

impl<'scope> Bindable for ScopedCollection<'scope, Vec<Value<'static>>> {
    type Of<'a> = ScopedCollection<'scope, Vec<Value<'static>>>;

    #[inline(always)]
    fn unbind(self) -> Self::Of<'static> {
        self
    }

    #[inline(always)]
    fn bind<'a>(self, _: NoGcScope<'a, '_>) -> Self::Of<'a> {
        self
    }
}

/// Iterator over a scoped Value list ([`Vec<Value>`] or an arguments list)
/// that stays on the heap while it is iterated.
///
/// The items are positions in the list; each read goes through the Agent, so
/// garbage collection between reads is fine.
pub(crate) struct ScopedValuesIterator<'a> {
    collection: u32,
    position: usize,
    end: usize,
    marker: PhantomData<&'a [Value<'static>]>,
}

impl ScopedValuesIterator<'_> {
    pub(crate) fn new(collection: u32, start: usize, end: usize) -> Self {
        Self {
            collection,
            position: start,
            end,
            marker: PhantomData,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ScopedValue<'a> {
    collection: u32,
    index: usize,
    marker: PhantomData<&'a [Value<'static>]>,
}

impl ScopedValue<'_> {
    pub fn get<'a>(&self, agent: &Agent, gc: NoGcScope<'a, '_>) -> Value<'a> {
        let stack_ref_collections = agent.stack_ref_collections.borrow();
        let value = match stack_ref_collections.get(self.collection as usize) {
            Some(HeapRootCollection::ValueVec(values)) => values.get(self.index),
            Some(HeapRootCollection::ArgumentsList(data)) => data.values.get(self.index),
            _ => None,
        };
        value
            .copied()
            .expect("ScopedValue read after its collection was dropped")
            .bind(gc)
    }
}

impl<'a> Iterator for ScopedValuesIterator<'a> {
    type Item = ScopedValue<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.end {
            return None;
        }
        let item = ScopedValue {
            collection: self.collection,
            index: self.position,
            marker: PhantomData,
        };
        self.position += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.end.saturating_sub(self.position);
        (len, Some(len))
    }
}

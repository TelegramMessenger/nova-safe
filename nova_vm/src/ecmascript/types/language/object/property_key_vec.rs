// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::marker::PhantomData;

use crate::{
    ecmascript::Agent,
    engine::{Bindable, HeapRootCollection, NoGcScope, ScopableCollection, ScopedCollection},
};

use super::PropertyKey;

impl ScopableCollection for Vec<PropertyKey<'_>> {
    fn scope<'scope>(
        self,
        agent: &Agent,
        gc: NoGcScope<'_, 'scope>,
    ) -> ScopedCollection<'scope, Self::Of<'static>> {
        ScopedCollection::new(agent, self.unbind(), gc)
    }
}

impl ScopedCollection<'_, Vec<PropertyKey<'static>>> {
    fn with_cb<R>(&self, agent: &Agent, f: impl FnOnce(&Vec<PropertyKey<'static>>) -> R) -> R {
        let stack_ref_collections = agent.stack_ref_collections.borrow();
        let Some(stack_slot) = stack_ref_collections.get(self.inner as usize) else {
            unreachable!();
        };
        let HeapRootCollection::PropertyKeyVec(property_key_vec) = stack_slot else {
            unreachable!()
        };
        f(property_key_vec)
    }

    fn with_cb_mut<R>(
        &mut self,
        agent: &Agent,
        f: impl FnOnce(&mut Vec<PropertyKey<'static>>) -> R,
    ) -> R {
        let mut stack_ref_collections = agent.stack_ref_collections.borrow_mut();
        let Some(stack_slot) = stack_ref_collections.get_mut(self.inner as usize) else {
            unreachable!();
        };
        let HeapRootCollection::PropertyKeyVec(property_key_vec) = stack_slot else {
            unreachable!()
        };
        f(property_key_vec)
    }

    /// Push a PropertyKey into the scoped vec.
    pub fn push(&mut self, agent: &Agent, value: PropertyKey) {
        self.with_cb_mut(agent, |property_key_vec| {
            property_key_vec.push(value.unbind())
        });
    }

    /// Returns `true` if the scoped vec contains a PropertyKey.
    pub fn contains(&self, agent: &Agent, value: PropertyKey) -> bool {
        self.with_cb(agent, |property_key_vec| {
            property_key_vec.contains(&value.unbind())
        })
    }

    pub(crate) fn iter(&self, agent: &mut Agent) -> ScopedPropertyKeysIterator<'_> {
        self.with_cb(agent, |property_key_vec| ScopedPropertyKeysIterator {
            collection: self.inner,
            position: 0,
            end: property_key_vec.len(),
            marker: PhantomData,
        })
    }

    /// Returns `true` if the scoped vec is empty.
    pub fn is_empty(&self, agent: &Agent) -> bool {
        self.with_cb(agent, |property_key_vec| property_key_vec.is_empty())
    }

    /// Returns the length of the scoped vec.
    pub fn len(&self, agent: &Agent) -> usize {
        self.with_cb(agent, |property_key_vec| property_key_vec.len())
    }
}

impl<'scope> Bindable for ScopedCollection<'scope, Vec<PropertyKey<'static>>> {
    type Of<'a> = ScopedCollection<'scope, Vec<PropertyKey<'static>>>;

    #[inline(always)]
    fn unbind(self) -> Self::Of<'static> {
        self
    }

    #[inline(always)]
    fn bind<'a>(self, _: NoGcScope<'a, '_>) -> Self::Of<'a> {
        self
    }
}

/// Iterator over a scoped [`Vec<PropertyKey>`] that stays on the heap while
/// it is iterated.
///
/// The items are positions in the Vec; each read goes through the Agent, so
/// garbage collection between reads is fine.
pub(crate) struct ScopedPropertyKeysIterator<'a> {
    collection: u32,
    position: usize,
    end: usize,
    marker: PhantomData<&'a mut ScopedCollection<'a, Vec<PropertyKey<'static>>>>,
}

#[derive(Clone, Copy)]
pub(crate) struct ScopedPropertyKey<'a> {
    collection: u32,
    index: usize,
    marker: PhantomData<&'a mut ScopedCollection<'a, Vec<PropertyKey<'static>>>>,
}

impl ScopedPropertyKey<'_> {
    /// Get the referenced PropertyKey.
    pub fn get<'a>(self, agent: &Agent, gc: NoGcScope<'a, '_>) -> PropertyKey<'a> {
        let stack_ref_collections = agent.stack_ref_collections.borrow();
        let key = match stack_ref_collections.get(self.collection as usize) {
            Some(HeapRootCollection::PropertyKeyVec(keys)) => keys.get(self.index),
            _ => None,
        };
        key.copied()
            .expect("ScopedPropertyKey read after its collection was dropped")
            .bind(gc)
    }
}

impl<'a> Iterator for ScopedPropertyKeysIterator<'a> {
    type Item = ScopedPropertyKey<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.end {
            return None;
        }
        let item = ScopedPropertyKey {
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

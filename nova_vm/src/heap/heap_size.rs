// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An estimate of the bytes the heap holds, for the heap limit.

use core::mem::size_of;

use super::Heap;
use crate::ecmascript::{PropertyKey, Value};

fn vec_bytes<T>(v: &[T]) -> usize {
    core::mem::size_of_val(v)
}

impl Heap {
    /// Bytes held by the heap's records, element storage, string data and
    /// ArrayBuffer data. Called after a garbage collection, when the vectors
    /// only hold live data.
    pub(crate) fn estimate_live_bytes(&self) -> usize {
        let mut total = 0usize;
        total += self.arrays.len() as usize * 16;
        total += vec_bytes(&self.array_iterators);
        total += vec_bytes(&self.async_generators);
        total += vec_bytes(&self.await_reactions);
        total += vec_bytes(&self.bigints);
        total += vec_bytes(&self.bound_functions);
        total += vec_bytes(&self.builtin_constructors);
        total += vec_bytes(&self.builtin_functions);
        total += vec_bytes(&self.ecmascript_functions);
        total += vec_bytes(&self.errors);
        total += vec_bytes(&self.executables);
        total += vec_bytes(&self.generators);
        total += self.maps.len() as usize * 64;
        total += crate::ecmascript::map_entries_bytes(&self.maps);
        total += vec_bytes(&self.map_iterators);
        total += vec_bytes(&self.numbers);
        total += vec_bytes(&self.object_shapes);
        total += vec_bytes(&self.objects);
        total += vec_bytes(&self.primitive_objects);
        total += vec_bytes(&self.promise_reaction_records);
        total += vec_bytes(&self.promise_resolving_functions);
        total += vec_bytes(&self.promise_finally_functions);
        total += vec_bytes(&self.promises);
        total += vec_bytes(&self.proxies);
        total += vec_bytes(&self.symbols);
        total += vec_bytes(&self.string_iterators);
        #[cfg(feature = "set")]
        {
            total += self.sets.len() as usize * 64;
            for index in 0..self.sets.len() {
                if let Some(set) = self.sets.get(index) {
                    total += set.values.capacity() * (size_of::<Option<Value<'static>>>() + 8);
                }
            }
        }
        #[cfg(feature = "array-buffer")]
        {
            total += vec_bytes(&self.array_buffers);
            total += vec_bytes(&self.typed_arrays);
            total += vec_bytes(&self.data_views);
            for buffer in self.array_buffers.iter() {
                total += buffer.byte_length();
            }
        }
        total += vec_bytes(&self.strings);
        for string in self.strings.iter() {
            total += string.len();
        }
        let value_size = size_of::<Option<Value<'static>>>();
        let key_size = size_of::<PropertyKey<'static>>();
        let e = &self.elements;
        macro_rules! storage {
            ($($values:ident, $keys:ident, $n:expr;)*) => {
                $(
                    total += e.$values.values.len().saturating_mul($n).saturating_mul(value_size);
                    total += e.$keys.keys.len().saturating_mul($n).saturating_mul(key_size);
                )*
            };
        }
        storage! {
            e2pow1, k2pow1, 1usize << 1;
            e2pow2, k2pow2, 1usize << 2;
            e2pow3, k2pow3, 1usize << 3;
            e2pow4, k2pow4, 1usize << 4;
            e2pow6, k2pow6, 1usize << 6;
            e2pow8, k2pow8, 1usize << 8;
            e2pow10, k2pow10, 1usize << 10;
            e2pow12, k2pow12, 1usize << 12;
            e2pow16, k2pow16, 1usize << 16;
            e2pow24, k2pow24, 1usize << 24;
            e2pow32, k2pow32, 1usize << 32;
        }
        total
    }
}

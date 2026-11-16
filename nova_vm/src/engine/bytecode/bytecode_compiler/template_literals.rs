// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! ### [13.2.8 Template Literals](https://tc39.es/ecma262/#sec-template-literals)

use oxc_ast::ast;

use super::parsed_string_to_wtf8;
use crate::{
    ecmascript::{
        Agent, Array, ArrayHeapData, BUILTIN_STRING_MEMORY, InternalMethods, InternalSlots,
        OrdinaryObject, String, Value, unwrap_try,
    },
    engine::{Bindable, NoGcScope},
    heap::{
        CreateHeapData, ElementDescriptor, ElementDescriptorMap, ObjectEntry,
        ObjectEntryPropertyDescriptor,
    },
};

/// ArrayCreate(count) for a template object. Unlike `array_create`, a large
/// count is not checked against the heap limit up front: the compiler has no
/// way to throw that error, and the parts are bounded by the template's source
/// text, which is already in memory.
fn create_template_array<'a>(agent: &mut Agent, len: usize, gc: NoGcScope<'a, '_>) -> Array<'a> {
    let mut elements = agent
        .heap
        .elements
        .allocate_elements_with_length(len)
        .expect("template object elements");
    elements.len = u32::try_from(len).expect("a template has fewer than 2^32 parts");
    agent.heap.create(ArrayHeapData {
        object_index: None,
        elements: elements.bind(gc),
    })
}

/// ### [13.2.8.4 GetTemplateObject ( templateLiteral )](https://tc39.es/ecma262/#sec-gettemplateobject)
///
/// The abstract operation GetTemplateObject takes argument templateLiteral (a
/// Parse Node) and returns an Array.
pub(super) fn get_template_object<'a>(
    agent: &mut Agent,
    template_literal: &ast::TemplateLiteral,
    gc: NoGcScope<'a, '_>,
) -> Array<'a> {
    // 1. Let realm be the current Realm Record.
    // 2. Let templateRegistry be realm.[[TemplateMap]].
    // 3. For each element e of templateRegistry, do
    // a. If e.[[Site]] is the same Parse Node as templateLiteral, then
    // i. Return e.[[Array]].
    // 4. Let rawStrings be the TemplateStrings of templateLiteral with argument true.
    // 5. Assert: rawStrings is a List of Strings.
    // 6. Let cookedStrings be the TemplateStrings of templateLiteral with argument false.
    // 7. Let count be the number of elements in the List cookedStrings.
    // 8. Assert: count ≤ 2**32 - 1.
    let len = template_literal.quasis.len();
    debug_assert!((len as u64) < 2u64.pow(32));
    // 11. Let index be 0.
    // 12. Repeat, while index < count,

    let mut cooked_values = Vec::with_capacity(len);
    let mut raw_values = Vec::with_capacity(len);
    let mut lone_surrogate_values = Vec::new();
    {
        let strings = &mut agent.heap.strings;
        let string_lookup_table = &mut agent.heap.string_lookup_table;
        let string_hasher = &mut agent.heap.string_hasher;
        let alloc_counter = &mut agent.heap.alloc_counter;
        for quasi in template_literal.quasis.iter() {
            // a. Let prop be ! ToString(𝔽(index)).
            // b. Let cookedValue be cookedStrings[index].
            let cooked_value = quasi.value.cooked.map_or(Value::Undefined, |cooked_value| {
                if quasi.lone_surrogates {
                    lone_surrogate_values.push((cooked_values.len(), cooked_value.as_str()));
                    return Value::Undefined;
                }
                String::from_str_direct(
                    strings,
                    string_lookup_table,
                    string_hasher,
                    alloc_counter,
                    cooked_value.as_str(),
                    gc,
                )
                .into()
            });
            // d. Let rawValue be the String value rawStrings[index].
            let raw_value = String::from_str_direct(
                strings,
                string_lookup_table,
                string_hasher,
                alloc_counter,
                quasi.value.raw.as_str(),
                gc,
            );
            cooked_values.push(cooked_value.unbind());
            raw_values.push(Value::from(raw_value.unbind()));
            // f. Set index to index + 1.
        }
    }
    for (index, cooked_value) in lone_surrogate_values {
        cooked_values[index] = parsed_string_to_wtf8(agent, cooked_value, true, gc)
            .unbind()
            .into();
    }
    create_template_object(agent, cooked_values, raw_values, gc)
}

/// The rest of GetTemplateObject: the template object of the given cooked
/// (strings or undefined) and raw strings.
pub(crate) fn create_template_object<'a>(
    agent: &mut Agent,
    cooked_values: Vec<Value<'static>>,
    raw_values: Vec<Value<'static>>,
    gc: NoGcScope<'a, '_>,
) -> Array<'a> {
    let len = cooked_values.len();
    debug_assert_eq!(len, raw_values.len());
    // 9. Let template be ! ArrayCreate(count).
    let template = create_template_array(agent, len, gc);
    // 10. Let rawObj be ! ArrayCreate(count).
    let raw_obj = create_template_array(agent, len, gc);
    // c. Perform ! DefinePropertyOrThrow(template, prop,
    //    PropertyDescriptor {
    //        [[Value]]: cookedValue, [[Writable]]: false,
    //        [[Enumerable]]: true, [[Configurable]]: false }).
    define_read_only_elements(agent, template, cooked_values);
    // e. Perform ! DefinePropertyOrThrow(rawObj, prop,
    //    PropertyDescriptor {
    //        [[Value]]: rawValue, [[Writable]]: false,
    //        [[Enumerable]]: true, [[Configurable]]: false }).
    define_read_only_elements(agent, raw_obj, raw_values);
    // 13. Perform ! SetIntegrityLevel(rawObj, frozen).
    unwrap_try(raw_obj.try_prevent_extensions(agent, gc));
    let prototype = template.internal_prototype(agent).unwrap();
    // 14. Perform ! DefinePropertyOrThrow(template,
    let template_backing_object = OrdinaryObject::create_object(
        agent,
        Some(prototype),
        &[ObjectEntry {
            // "raw",
            key: BUILTIN_STRING_MEMORY.raw.to_property_key(),
            // PropertyDescriptor {
            value: ObjectEntryPropertyDescriptor::Data {
                // [[Value]]: rawObj,
                value: raw_obj.into(),
                // [[Writable]]: false,
                writable: false,
                // [[Enumerable]]: false,
                enumerable: false,
                // [[Configurable]]: false
                configurable: false,
            },
            // }).
        }],
    )
    .expect("Should perform GC here");
    template.set_backing_object(agent, template_backing_object.unbind());
    // 15. Perform ! SetIntegrityLevel(template, frozen).
    unwrap_try(template.try_prevent_extensions(agent, gc));
    // 16. Append the Record { [[Site]]: templateLiteral, [[Array]]: template }
    //     to realm.[[TemplateMap]].
    // 17. Return template.
    template
}

/// Writes `values` into the elements of a new array of the same length as
/// read-only, enumerable, non-configurable data properties.
fn define_read_only_elements(agent: &mut Agent, array: Array, values: Vec<Value<'static>>) {
    let storage = array.get_storage_mut(agent);
    let elements = storage.values;
    let descriptors = storage
        .descriptors
        .insert_entry(ElementDescriptorMap::with_capacity(values.len()))
        .into_mut();
    for (prop, value) in values.into_iter().enumerate() {
        elements[prop] = Some(value);
        descriptors.insert(
            prop as u32,
            ElementDescriptor::ReadOnlyEnumerableUnconfigurableData,
        );
    }
}

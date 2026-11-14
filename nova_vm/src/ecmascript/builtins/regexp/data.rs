// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use oxc_ast::ast::RegExpFlags;
use regress::{ExecBudget, Flags, Match, MatchError, Regex};
use wtf8::{Wtf8Buf, Wtf8Str};

use crate::{
    ecmascript::{OrdinaryObject, PropertyDescriptor, String, Value, execution::Agent},
    engine::Bindable,
    heap::{CompactionLists, HeapMarkAndSweep, StringHeapAccess, WorkQueues},
};

/// ## Optimistic storage for the RegExp "lastIndex" property
///
/// The property can take any JavaScript Value, but under any reasonable use it
/// is an index into a JavaScript String which then means it is a 32-bit
/// unsigned integer since strings of 4 GiB length or larger are rare.
///
/// As such, the optimistic storage is a 32-bit unsigned integer with the
/// `u32::MAX` value used as a sentinel to signify that the backing object
/// should be asked for the actual value. If the backing object doesn't exist,
/// then the property value is `undefined`.
///
/// ### Writability of the "lastIndex" property
///
/// The `lastIndex` property can be made read-only using
/// `Object.defineProperty` or `Object.freeze`. These are rare operations and
/// as such, the writable bit of the "lastIndex" property is not stored in the
/// RegExp data at all. This means that if the backing object does not exists,
/// then the writable bit is guaranteed to be `true`. If the backing object
/// does exist, the writable bit must be asked from the backing object in all
/// cases.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub(crate) struct RegExpLastIndex(u32);

impl Default for RegExpLastIndex {
    fn default() -> Self {
        Self(u32::MAX)
    }
}

impl RegExpLastIndex {
    pub(crate) const ZERO: Self = Self(0);
    const INVALID: Self = Self(u32::MAX);

    /// Create a RegExpLastIndex from a JavaScript Value.
    pub(super) fn from_value(value: Value) -> Self {
        let Value::Integer(value) = value else {
            return Self::INVALID;
        };
        let value = value.into_i64();
        if let Ok(value) = u32::try_from(value) {
            if value == u32::MAX {
                Self::INVALID
            } else {
                Self(value)
            }
        } else {
            Self::INVALID
        }
    }

    /// Check if the RegExpLastIndex is an accurate JavaScript value.
    ///
    /// Note: If backing object does not exist, `undefined` is also accurately
    /// represented by an invalid RegExpLastIndex.
    #[inline]
    pub(super) fn is_valid(self) -> bool {
        self.0 != u32::MAX
    }

    /// Get the current lastIndex property index value.
    ///
    /// If the current value is not a 32-bit unsigned integer, then None is
    /// returned.
    pub(super) fn get_value(self) -> Option<u32> {
        if self.0 == u32::MAX {
            None
        } else {
            Some(self.0)
        }
    }

    /// Get the lastIndex property descriptor value.
    ///
    /// This property descriptor is only valid if the backing object does not
    /// exist. If it does exist, then this both the value and the writable bit
    /// of this descriptor may differ from the real values.
    pub(super) fn into_property_descriptor(self) -> PropertyDescriptor<'static> {
        PropertyDescriptor {
            value: Some(self.get_value().map_or(Value::Undefined, |i| i.into())),
            writable: Some(true),
            get: None,
            set: None,
            enumerable: Some(false),
            configurable: Some(false),
        }
    }
}

impl From<usize> for RegExpLastIndex {
    fn from(value: usize) -> Self {
        if let Ok(value) = u32::try_from(value) {
            if value == u32::MAX {
                Self::INVALID
            } else {
                Self(value)
            }
        } else {
            Self::INVALID
        }
    }
}

/// The most steps one search may take: a pattern that backtracks
/// exponentially (`/(a+)+$/` on a long run of "a") ends with an error instead
/// of running for years.
const MAX_STEPS: u64 = 200_000_000;
/// How many steps go between two host interrupt polls.
const POLL_INTERVAL: u64 = crate::ecmascript::POLL_INTERVAL as u64;
/// The most entries of the backtracking stack (each a few machine words).
const MAX_BACKTRACK_ENTRIES: usize = 4 << 20;

/// Append the letters of `flags` in the order the `flags` getter writes
/// them (RegExp.prototype.flags: "dgimsuvy").
pub(crate) fn push_flag_letters(buf: &mut Wtf8Buf, flags: RegExpFlags) {
    for (flag, letter) in [
        (RegExpFlags::D, 'd'),
        (RegExpFlags::G, 'g'),
        (RegExpFlags::I, 'i'),
        (RegExpFlags::M, 'm'),
        (RegExpFlags::S, 's'),
        (RegExpFlags::U, 'u'),
        (RegExpFlags::V, 'v'),
        (RegExpFlags::Y, 'y'),
    ] {
        if flags.contains(flag) {
            buf.push_char(letter);
        }
    }
}

/// The deepest nesting of groups and classes a pattern may have: deeper
/// patterns are a SyntaxError before any parser recurses into them.
pub(crate) const MAX_PATTERN_NESTING: usize = 256;

/// Why a search ended without an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegExpExecError {
    /// The step or stack budget ran out.
    TooComplex,
    /// The host asked to stop.
    Interrupted,
}

impl RegExpExecError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::TooComplex => "regular expression is too complex to match",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, crate::engine::Bindable)]
pub(crate) struct RegExpHeapData<'a> {
    pub(super) object_index: Option<OrdinaryObject<'a>>,
    pub(super) reg_exp_matcher: Result<Regex, std::string::String>,
    pub(super) original_source: String<'a>,
    pub(super) original_flags: RegExpFlags,
    pub(super) last_index: RegExpLastIndex,
    /// The last non-ASCII string searched and its UTF-16 code units, so that
    /// a global search over it converts it once.
    utf16_input_key: Option<String<'a>>,
    utf16_input: Vec<u16>,
}

impl<'a> RegExpHeapData<'a> {
    /// Compiles a pattern given as a JavaScript string. Without the `u` or
    /// `v` flag the pattern is a list of code units, with them a list of code
    /// points (lone surrogates included).
    pub(crate) fn compile_pattern(
        pattern: Wtf8Str<'_>,
        flags: RegExpFlags,
    ) -> Result<Regex, std::string::String> {
        if pattern_nesting(pattern) > MAX_PATTERN_NESTING {
            return Err("regular expression is nested too deeply".into());
        }
        let regress_flags = Flags {
            icase: flags.contains(RegExpFlags::I),
            multiline: flags.contains(RegExpFlags::M),
            dot_all: flags.contains(RegExpFlags::S),
            no_opt: false,
            unicode: flags.intersects(RegExpFlags::U | RegExpFlags::V),
            unicode_sets: flags.contains(RegExpFlags::V),
        };
        let result = if flags.intersects(RegExpFlags::U | RegExpFlags::V) {
            Regex::from_unicode(pattern.code_points().map(|c| c.to_u32()), regress_flags)
        } else {
            Regex::from_unicode(pattern.to_ill_formed_utf16().map(u32::from), regress_flags)
        };
        result.map_err(|err| err.to_string())
    }

    pub(crate) fn new(agent: &Agent, source: String<'a>, flags: RegExpFlags) -> Self {
        let regex = Self::compile_pattern(source.as_wtf8_(agent), flags);
        Self {
            object_index: None,
            reg_exp_matcher: regex,
            original_source: source,
            original_flags: flags,
            last_index: RegExpLastIndex::ZERO,
            utf16_input_key: None,
            utf16_input: Vec::new(),
        }
    }

    /// Searches `s` from UTF-16 index `start` (only at `start` if `sticky`).
    /// The match and its captures are UTF-16 ranges. `poll` is called every
    /// few thousand steps; returning true stops the search with
    /// [`RegExpExecError::Interrupted`].
    ///
    /// ## Panics
    ///
    /// If the pattern did not compile.
    pub(super) fn search(
        &mut self,
        strings: &impl StringHeapAccess,
        s: String<'_>,
        start: usize,
        sticky: bool,
        poll: &mut dyn FnMut() -> bool,
    ) -> Result<Option<Match>, RegExpExecError> {
        let bytes = s.as_bytes_(strings);
        let utf16_len = s.utf16_len_(strings);
        let budget = ExecBudget {
            max_steps: MAX_STEPS,
            poll_interval: POLL_INTERVAL,
            max_backtrack_entries: MAX_BACKTRACK_ENTRIES,
            poll: Some(poll),
        };
        let full_unicode = self
            .original_flags
            .intersects(RegExpFlags::U | RegExpFlags::V);
        let result = if utf16_len == bytes.len() {
            let matcher = self.reg_exp_matcher.as_ref().expect("a compiled pattern");
            matcher.exec_ascii(bytes, start, sticky, budget)
        } else {
            if self.utf16_input_key != Some(s.unbind()) {
                self.utf16_input.clear();
                self.utf16_input
                    .extend(s.as_wtf8_(strings).to_ill_formed_utf16());
                self.utf16_input_key = match s {
                    String::String(_) => Some(s.unbind()),
                    String::SmallString(_) => None,
                };
            }
            let units = &self.utf16_input;
            let start = if full_unicode
                && start > 0
                && start < units.len()
                && (0xDC00..0xE000).contains(&units[start])
                && (0xD800..0xDC00).contains(&units[start - 1])
            {
                start - 1
            } else {
                start
            };
            let matcher = self.reg_exp_matcher.as_ref().expect("a compiled pattern");
            matcher.exec_utf16(units, start, sticky, budget)
        };
        result.map_err(|err| match err {
            MatchError::Interrupted => RegExpExecError::Interrupted,
            MatchError::StepLimit | MatchError::StackLimit => RegExpExecError::TooComplex,
        })
    }
}

/// The deepest nesting of groups and (`v` flag) classes in a pattern. A
/// cheap scan that over-counts escaped or in-class parentheses never, and
/// may count brackets that a non-`v` class takes literally, which only makes
/// it stricter.
pub(crate) fn pattern_nesting(pattern: Wtf8Str<'_>) -> usize {
    let bytes = pattern.as_bytes();
    let mut depth = 0usize;
    let mut max_depth = 0usize;
    let mut class_depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'[' => {
                class_depth += 1;
                depth += 1;
            }
            b']' if class_depth > 0 => {
                class_depth -= 1;
                depth = depth.saturating_sub(1);
            }
            b'(' if class_depth == 0 => depth += 1,
            b')' if class_depth == 0 => depth = depth.saturating_sub(1),
            _ => {}
        }
        max_depth = max_depth.max(depth);
        i += 1;
    }
    max_depth
}

impl Default for RegExpHeapData<'_> {
    fn default() -> Self {
        Self {
            object_index: Default::default(),
            reg_exp_matcher: Err(std::string::String::new()),
            original_source: String::EMPTY_STRING,
            original_flags: RegExpFlags::empty(),
            last_index: Default::default(),
            utf16_input_key: None,
            utf16_input: Vec::new(),
        }
    }
}

impl HeapMarkAndSweep for RegExpHeapData<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            object_index,
            reg_exp_matcher: _,
            original_source,
            original_flags: _,
            last_index: _,
            utf16_input_key,
            utf16_input: _,
        } = self;
        object_index.mark_values(queues);
        original_source.mark_values(queues);
        utf16_input_key.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            object_index,
            reg_exp_matcher: _,
            original_source,
            original_flags: _,
            last_index: _,
            utf16_input_key,
            utf16_input: _,
        } = self;
        object_index.sweep_values(compactions);
        original_source.sweep_values(compactions);
        utf16_input_key.sweep_values(compactions);
    }
}

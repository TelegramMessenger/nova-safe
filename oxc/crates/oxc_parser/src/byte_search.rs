#![forbid(unsafe_code)]

#[inline]
pub fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

#[inline]
pub fn memchr_iter(needle: u8, haystack: &[u8]) -> impl Iterator<Item = usize> + '_ {
    haystack.iter().enumerate().filter_map(move |(i, &b)| (b == needle).then_some(i))
}

pub mod memmem {
    pub struct Finder<'n> {
        needle: &'n [u8],
    }

    impl<'n> Finder<'n> {
        pub fn new<N: ?Sized + AsRef<[u8]>>(needle: &'n N) -> Self {
            Self { needle: needle.as_ref() }
        }

        pub fn find(&self, haystack: &[u8]) -> Option<usize> {
            let Some((&first, rest)) = self.needle.split_first() else {
                return Some(0);
            };
            let last_start = haystack.len().checked_sub(self.needle.len())?;
            let mut start = 0;
            while start <= last_start {
                let offset = super::memchr(first, &haystack[start..=last_start])?;
                let candidate = start + offset;
                if haystack[candidate + 1..candidate + self.needle.len()] == *rest {
                    return Some(candidate);
                }
                start = candidate + 1;
            }
            None
        }
    }
}

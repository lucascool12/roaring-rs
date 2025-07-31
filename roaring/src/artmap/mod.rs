use blart::{TreeMap, map::{self, PrefixEntry, PrefixOccupied}};
use container::Container;
use key::HighKey;
use core::{iter::Peekable, ops::RangeInclusive};

use crate::{bitmap, RoaringBitmap};

mod container;
mod key;

const PREFIX_LEN: usize = key::BYTES_AMOUNT as usize;

#[derive(Debug)]
pub struct RoaringArtmap {
    map: TreeMap<HighKey, Container, { PREFIX_LEN }>,
}

impl Default for RoaringArtmap {
    fn default() -> Self {
        Self::new()
    }
}

fn split_key(value: u64) -> (u32, u32) {
    (((value >> u64::BITS / 2) as u32), ((value & u64::from(u32::MAX)) as u32))
}

fn assemble_key_from_parts(high: u32, low: u32) -> u64 {
    ((high as u64) << u64::BITS/2) | low as u64
}

impl RoaringArtmap {
    pub fn new() -> Self {
        Self {
            map: Default::default(),
        }
    }

    pub fn insert(&mut self, value: u64) -> bool {
        let (high, low) = split_key(value);

        match self.map.prefix_entry(HighKey::from(high)) {
            // We have reached an inner node, this is only possible with Full containers, as such
            // this value already exists.
            PrefixEntry::Occupied(PrefixOccupied::Inner(_)) => false,
            PrefixEntry::Occupied(PrefixOccupied::Leaf(mut occupied)) => {
                match occupied.get_mut() {
                    Container::Full => false,
                    Container::Bitmap(bitmap) => {
                        let ret = bitmap.insert(low);
                        if ret && bitmap.is_full() {
                            *occupied.get_mut() = Container::Full;
                        }
                        ret
                    }
                }
            },
            PrefixEntry::Vacant(value) => {
                value.insert(
                    Container::Bitmap(RoaringBitmap::from_iter([low]))
                );
                true
            }
        }
    }

    pub fn remove(&mut self, value: u64) -> bool {
        let (high, low) = split_key(value);
        let key = match self.map.prefix_entry(HighKey::from(high)) {
            PrefixEntry::Occupied(PrefixOccupied::Inner(inner)) => {
                let inner_key = *inner.get_key().unwrap();
                let mut new = RoaringBitmap::full();
                new.remove(low);
                inner.insert(Container::Bitmap(new));
                inner_key
            }
            PrefixEntry::Occupied(PrefixOccupied::Leaf(mut occupied)) => {
                match occupied.get_mut() {
                    Container::Full => {
                        let mut new = RoaringBitmap::full();
                        new.remove(low);
                        *occupied.get_mut() = Container::Bitmap(new);
                        return true;
                    }
                    Container::Bitmap(bitmap) => {
                        let ret = bitmap.remove(low);
                        if ret {
                            bitmap.is_empty();
                            occupied.remove();
                        }
                        return ret;
                    },
                }
            },
            PrefixEntry::Vacant(_) => return false,
        };
        true
    }

    fn cmp_iter<'a>(&'a self, other: &'a Self) -> CompareIter<'a> {
        CompareIter {
            left: self.map.iter().peekable(),
            right: other.map.iter().peekable(),
        }
    }
}

impl PartialEq for RoaringArtmap {
    fn eq(&self, other: &Self) -> bool {
        self.cmp_iter(other).all(|value| match value {
            EitherOrBoth::Both(left, right) => left.1 == right.1,
            _ => false,
        })
    }
}

pub struct Iter<'a> {
    art_iter: blart::map::Iter<'a, HighKey, Container, { PREFIX_LEN }>,
    cur_container: Option<(HighKey, container::Iter<'a>)>,
}

impl<'a> Iter<'a> {
    fn new(art_map: &'a RoaringArtmap) -> Self {
        Self {
            art_iter: art_map.map.iter(),
            cur_container: None,
        }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        let (high_key, cur_iter) = match &mut self.cur_container {
            Some(value) => {
                value
            }
            value @ None => {
                let (next_key, next_container) = self.art_iter.next()?;
                *value = Some((*next_key, next_container.iter()));
                value.as_mut().unwrap()
            }
        };
        let low_key = cur_iter.next()?;
        Some(assemble_key_from_parts(high_key.value.get(), low_key))
    }
}

struct CompareIter<'a> {
    left: Peekable<map::Iter<'a, HighKey, Container, { PREFIX_LEN }>>,
    right: Peekable<map::Iter<'a, HighKey, Container, { PREFIX_LEN }>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EitherOrBoth<L, R> {
    Left(L),
    Right(R),
    Both(L, R),
}

impl<'a> Iterator for CompareIter<'a> {
    type Item = EitherOrBoth<(&'a HighKey, &'a Container), (&'a HighKey, &'a Container)>;

    fn next(&mut self) -> Option<Self::Item> {
        use core::cmp::Ordering;
        match (self.left.peek(), self.right.peek()) {
            (Some(&left), Some(&right)) => {
                match left.0.cmp(right.0) {
                    Ordering::Equal => {
                        self.left.next();
                        self.right.next();
                        return Some(EitherOrBoth::Both(left, right));
                    }
                    Ordering::Less => {
                        // left key is smaller than right key, this can mean two things:
                        // - right key starts with left key, i.e. right contains left
                        // - right key has no overlap with left key
                        if right.0.starts_with(left.0) {
                            // since right starts with left, we always advance right
                            self.right.next();
                            // we should advance left if the next right key no longer starts
                            // with the left key
                            if self.right.peek().map(|f| !f.0.starts_with(left.0)).unwrap_or(false) {
                                self.left.next();
                            }
                            return Some(EitherOrBoth::Both(left, right))
                        } else {
                            self.left.next();
                            return Some(EitherOrBoth::Left(left));
                        }
                    }
                    Ordering::Greater => {
                        // same two cases as Ordering::Less case except with left and right
                        // swapped.
                        if left.0.starts_with(right.0) {
                            // since left starts with right, we always advance left
                            self.left.next();
                            // we should advance right if the next left key no longer starts
                            // with the right key
                            if self.left.peek().map(|f| !f.0.starts_with(right.0)).unwrap_or(false) {
                                self.right.next();
                            }
                            return Some(EitherOrBoth::Both(left, right))
                        } else {
                            self.right.next();
                            return Some(EitherOrBoth::Right(right));
                        }
                    }
                }
            }
            (Some(&left), None) => {
                self.left.next();
                return Some(EitherOrBoth::Left(left));
            }
            (None, Some(&right)) => {
                self.right.next();
                return Some(EitherOrBoth::Right(right));
            }
            (None, None) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::artmap::{container::Container, EitherOrBoth, split_key};

    use super::{key::HighKey, CompareIter, RoaringArtmap};

    #[test]
    fn iter_test() {
        let mut left = RoaringArtmap::new();
        let mut right = RoaringArtmap::new();
        left.map.force_insert(HighKey::new(0xAAAAAA00, 3).unwrap(), Container::Full);
        left.map.force_insert(HighKey::new(0xBBBBBB00, 3).unwrap(), Container::Full);
        left.map.force_insert(HighKey::new(0xFFFFFF00, 4).unwrap(), Container::Full);
        left.map.force_insert(HighKey::new(0xFFFFFF01, 4).unwrap(), Container::Full);
        left.map.force_insert(HighKey::new(0xFFFFFF02, 4).unwrap(), Container::Full);
        right.map.force_insert(HighKey::new(0xAAAAAA00, 2).unwrap(), Container::Full);
        right.map.force_insert(HighKey::new(0xCCCCCC00, 2).unwrap(), Container::Full);
        right.map.force_insert(HighKey::new(0xFFFFFF00, 2).unwrap(), Container::Full);
        let mut iter = CompareIter {
            left: left.map.iter().peekable(),
            right: right.map.iter().peekable(),
        };
        assert_eq!(
            iter.next(),
            Some(
                EitherOrBoth::Both(
                    (&HighKey::new(0xAAAAAA00, 3).unwrap(), &Container::Full),
                    (&HighKey::new(0xAAAAAA00, 2).unwrap(), &Container::Full),
                )
            )
        );
        assert_eq!(
            iter.next(),
            Some(
                EitherOrBoth::Left(
                    (&HighKey::new(0xBBBBBB00, 3).unwrap(), &Container::Full),
                )
            )
        );
        assert_eq!(
            iter.next(),
            Some(
                EitherOrBoth::Right(
                    (&HighKey::new(0xCCCCCC00, 2).unwrap(), &Container::Full),
                )
            )
        );
        assert_eq!(
            iter.next(),
            Some(
                EitherOrBoth::Both(
                    (&HighKey::new(0xFFFFFF00, 4).unwrap(), &Container::Full),
                    (&HighKey::new(0xFFFFFF00, 2).unwrap(), &Container::Full),
                )
            )
        );
        assert_eq!(
            iter.next(),
            Some(
                EitherOrBoth::Both(
                    (&HighKey::new(0xFFFFFF01, 4).unwrap(), &Container::Full),
                    (&HighKey::new(0xFFFFFF00, 2).unwrap(), &Container::Full),
                )
            )
        );
        assert_eq!(
            iter.next(),
            Some(
                EitherOrBoth::Both(
                    (&HighKey::new(0xFFFFFF02, 4).unwrap(), &Container::Full),
                    (&HighKey::new(0xFFFFFF00, 2).unwrap(), &Container::Full),
                )
            )
        );
    }

    #[test]
    fn test_split_key() {
        assert_eq!(
            split_key(0x0123456789ABCDEF),
            (0x01234567, 0x89ABCDEF)
        );
    }
}

use core::ops::Div;

use blart::{AsBytes, Mapped, ToUBE};

pub const BYTES_AMOUNT: u8 = (u32::BITS / 8) as u8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighKey {
    pub value: Mapped<ToUBE, u32>,
    pub length: u8,
}

impl HighKey {
    pub fn new(value: u32, length: u8) -> Result<Self, ()> {
        if length > BYTES_AMOUNT {
            Err(())
        } else {
            Ok(Self { value: Mapped::new(value), length })
        }
    }

    pub fn starts_with(&self, other: &Self) -> bool {
        self.as_bytes().starts_with(other.as_bytes())
    }

    pub fn next_step(self) -> Option<Self> {
        1u32.checked_shl((self.length * 8) as u32)
            .and_then(|to_add| self.value.get().checked_add(to_add))
            .map(|next_value| Self { value: Mapped::new(next_value), length: self.length })
    }

    pub fn next(self) -> Option<Self> {
        1u32.checked_shl((self.length * 8) as u32)
            .and_then(|to_add| self.value.get().checked_add(to_add))
            .map(|next_value| Self { value: Mapped::new(next_value), length: BYTES_AMOUNT })
    }

    pub fn prev_step(self) -> Option<Self> {
        1u32.checked_shl((self.length * 8) as u32)
            .and_then(|to_add| self.value.get().checked_sub(to_add))
            .map(|next_value| Self { value: Mapped::new(next_value), length: self.length })
    }

    pub fn prev(self) -> Option<Self> {
        1u32.checked_shl((self.length * 8) as u32)
            .and_then(|to_add| self.value.get().checked_sub(to_add))
            .map(|next_value| Self { value: Mapped::new(next_value), length: BYTES_AMOUNT })
    }

    pub fn iter_inclusive(self, other: HighKey) {}
}

impl From<u32> for HighKey {
    fn from(value: u32) -> Self {
        HighKey { value: Mapped::new(value), length: 4 }
    }
}

impl PartialOrd for HighKey {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        self.as_bytes().iter().partial_cmp(other.as_bytes().iter())
    }
}

impl Ord for HighKey {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_bytes().iter().cmp(other.as_bytes().iter())
    }
}

impl blart::AsBytes for HighKey {
    fn as_bytes(&self) -> &[u8] {
        &blart::AsBytes::as_bytes(&self.value)[..self.length.into()]
    }
}

pub struct HighKeyIter {
    cur: HighKey,
    end: u32,
    exhausted: bool
}

impl Iterator for HighKeyIter {
    type Item = HighKey;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }
        if self.cur < HighKey::from(self.end) {
            let this_length_step = ((self.end - self.cur.value.get()).ilog2() / 8) as u8;
            self.cur = HighKey {
                value: self.cur.value,
                length: this_length_step,
            };
            let ret = self.cur;
            if let Some(next_cur) = self.cur.next_step() {
                self.cur = next_cur;
            } else {
                self.exhausted = true;
            }
            Some(ret)
        } else {
            self.exhausted = true;
            Some(self.cur)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HighKey;

    #[test]
    fn high_key_cmp() {
        let a = HighKey::new(0xFF000000, 2).unwrap();
        let b = HighKey::new(0xFF00AA00, 3).unwrap();
        assert!(b.starts_with(&a));
        assert!(a < b);
    }
}

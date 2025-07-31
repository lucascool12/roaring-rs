use crate::{bitmap, RoaringBitmap};

#[derive(Debug, Clone)]
pub enum Container {
    Full,
    Bitmap(RoaringBitmap),
}

impl Container {
    pub fn iter(&self) -> Iter {
        Iter::new(self)
    }
}

impl PartialEq for Container {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Full, Self::Full) => true,
            (Self::Full, Self::Bitmap(other)) => other.is_full(),
            (Self::Bitmap(this), Self::Full) => this.is_full(),
            (Self::Bitmap(this), Self::Bitmap(other)) => this.eq(other),
        }
    }
}

impl Eq for Container {}

pub enum Iter<'a> {
    Full {
        cur: u32,
        ended: bool,
    },
    Bitmap(bitmap::Iter<'a>)
}

impl<'a> Iter<'a> {
    fn new(container: &'a Container) -> Self {
        match container {
            Container::Full => Self::Full { cur: 0, ended: false },
            Container::Bitmap(bitmap) => Self::Bitmap(bitmap.iter()),
        }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Full { ended, .. } if *ended => {
                None
            }
            Self::Full { cur, ended } => {
                let res = cur.overflowing_add(1);
                *ended = res.1;
                let ret = *cur;
                *cur = res.0;
                Some(ret)
            }
            Self::Bitmap(iter) => iter.next(),
        }
    }
}

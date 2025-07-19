use crate::RoaringBitmap;

#[derive(Debug, Clone)]
pub enum Container {
    Full,
    Bitmap(RoaringBitmap),
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


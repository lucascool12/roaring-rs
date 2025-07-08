use blart::TreeMap;
use crate::RoaringBitmap;

pub struct RoaringArtmap {
    map: TreeMap<u32, RoaringBitmap>,
}

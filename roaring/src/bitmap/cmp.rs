use core::borrow::Borrow;
use core::cmp::Ordering;
use core::iter::FusedIterator;
use core::marker::PhantomData;

use rayon::iter::plumbing::{bridge_unindexed, Producer, UnindexedProducer, UnindexedConsumer};
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};
use rayon::vec::{SliceDrain, DrainProducer, Drain};
use rayon::slice::{IterProducer, IterMutProducer};

use super::container::Container;
use super::store::SliceIterator;
use crate::RoaringBitmap;

impl RoaringBitmap {
    /// Returns true if the set has no elements in common with other. This is equivalent to
    /// checking for an empty intersection.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use roaring::RoaringBitmap;
    ///
    /// let mut rb1 = RoaringBitmap::new();
    /// let mut rb2 = RoaringBitmap::new();
    ///
    /// rb1.insert(1);
    ///
    /// assert_eq!(rb1.is_disjoint(&rb2), true);
    ///
    /// rb2.insert(1);
    ///
    /// assert_eq!(rb1.is_disjoint(&rb2), false);
    ///
    /// ```
    pub fn is_disjoint(&self, other: &Self) -> bool {
        Pairs::new(&self.containers, &other.containers)
            .filter_map(|(c1, c2)| c1.zip(c2))
            .all(|(c1, c2)| c1.is_disjoint(c2))
    }

    /// Returns `true` if this set is a subset of `other`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use roaring::RoaringBitmap;
    ///
    /// let mut rb1 = RoaringBitmap::new();
    /// let mut rb2 = RoaringBitmap::new();
    ///
    /// rb1.insert(1);
    ///
    /// assert_eq!(rb1.is_subset(&rb2), false);
    ///
    /// rb2.insert(1);
    ///
    /// assert_eq!(rb1.is_subset(&rb2), true);
    ///
    /// rb1.insert(2);
    ///
    /// assert_eq!(rb1.is_subset(&rb2), false);
    /// ```
    pub fn is_subset(&self, other: &Self) -> bool {
        for pair in Pairs::new(&self.containers, &other.containers) {
            match pair {
                (None, _) => (),
                (_, None) => return false,
                (Some(c1), Some(c2)) => {
                    if !c1.is_subset(c2) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Returns `true` if this set is a superset of `other`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use roaring::RoaringBitmap;
    ///
    /// let mut rb1 = RoaringBitmap::new();
    /// let mut rb2 = RoaringBitmap::new();
    ///
    /// rb1.insert(1);
    ///
    /// assert_eq!(rb2.is_superset(&rb1), false);
    ///
    /// rb2.insert(1);
    ///
    /// assert_eq!(rb2.is_superset(&rb1), true);
    ///
    /// rb1.insert(2);
    ///
    /// assert_eq!(rb2.is_superset(&rb1), false);
    /// ```
    pub fn is_superset(&self, other: &Self) -> bool {
        other.is_subset(self)
    }
}

/// An helping Iterator over pairs of containers.
///
/// Returns the smallest container according to its key
/// or both if the key is the same. It is useful when you need
/// to iterate over two containers to do operations on them.
pub(crate) struct Pairs<I, J, L, R>
where
    I: SliceIterator<Container, Item = L>,
    J: SliceIterator<Container, Item = R>,
    L: Borrow<Container>,
    R: Borrow<Container>,
{
    left: I,
    right: J,
}

impl<I, J, L, R> Pairs<I, J, L, R>
where
    I: SliceIterator<Container, Item = L>,
    J: SliceIterator<Container, Item = R>,
    L: Borrow<Container>,
    R: Borrow<Container>,
{
    pub fn new<A, B>(left: A, right: B) -> Pairs<I, J, L, R>
    where
        A: IntoIterator<Item = L, IntoIter = I>,
        B: IntoIterator<Item = R, IntoIter = J>,
    {
        Pairs { left: left.into_iter(), right: right.into_iter() }
    }
}

impl<I, J, L, R> Iterator for Pairs<I, J, L, R>
where
    I: SliceIterator<Container, Item = L>,
    J: SliceIterator<Container, Item = R>,
    L: Borrow<Container>,
    R: Borrow<Container>,
{
    type Item = (Option<L>, Option<R>);

    fn next(&mut self) -> Option<Self::Item> {
        match (self.left.as_slice().first(), self.right.as_slice().first()) {
            (None, None) => None,
            (Some(_), None) => Some((self.left.next(), None)),
            (None, Some(_)) => Some((None, self.right.next())),
            (Some(c1), Some(c2)) => match c1.borrow().key.cmp(&c2.borrow().key) {
                Ordering::Equal => Some((self.left.next(), self.right.next())),
                Ordering::Less => Some((self.left.next(), None)),
                Ordering::Greater => Some((None, self.right.next())),
            },
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let size = self.left.len().max(self.right.len());
        (size, Some(self.left.len() + self.right.len()))
    }
}

impl<I, J, L, R> FusedIterator for Pairs<I, J, L, R>
where
    I: SliceIterator<Container, Item = L>,
    J: SliceIterator<Container, Item = R>,
    L: Borrow<Container>,
    R: Borrow<Container>,
{}

/// An iterator of pairs of [Container]s, like [Pairs] but splitable.
///
/// This iterator will iterate until both iterators are exhausted or **both** have passed their guard
/// value.
///
/// This allows us to split the iterator using the following method.
/// First choose a splitting point (middle point of each sub iterator, is an obvious choice).
/// In the first splitted iterator adjust the guards to the given split point.
/// In the second splitted iterator we take the guards from the original iterator but move up the
/// iterators appropriately.
/// After moving up the inner iterators, we query for the last largest value before the guards in the
/// first iterator and note down which of the two inner iterators contains this largest value.
/// Then in the other inner iterator (or neither if both have the same value before the guard) we move
/// this iterator up until we find the next largest value then the value we found in the first
/// iterator.
/// After this the splitted pairs produce the same pairs as the unsplitted one.
///
/// For example, say we have the following two arrays of containers (only container keys will be written down),
/// `[0, 1, 3, 5, 7, 8]` and `[3, 5, 8, 9]`.
/// Using [Pairs] or [SplitPairs] without splitting produces the following values:
/// `(Some(0), None)`, `(Some(1), None)`, `(Some(3), Some(3))`, `(Some(5), Some(5))`,
/// `(Some(7), None)`, `(Some(8), Some(8))`, `(None, Some(9))`
///
/// If we were to split a [SplitPairs] iterator of both we would get the following.
/// First we choose a split point, here the middle of each iterator, i.e. `[0, 1, 3, | 5, 7, 8]` and
/// `[3, 5, | 8, 9]`, `|` denotes the split point and guards for each iterator.
/// After which we look for the biggest value before the guards in both, we find that the biggest
/// value is 5 in the second inner iterator. We then look for the position of the value larger
/// than 5 in the first inner iterator.
/// We find that at position 4 the value is larger than 5.
/// Now we can construct the splitted iterators, the first iterator will contains `[0, 1, 3, 5]`
/// and `[3, 5]`, the second iterator will contains `[7, 8`] and `[8, 9]`.
/// The values produced by the first iterator is:
/// `(Some(0), None)`, `(Some(1), None)`, `(Some(3), Some(3))`, `(Some(5), Some(5))`,
/// While the values produced by the second iterator is:
/// `(Some(7), None)`, `(Some(8), Some(8))`, `(None, Some(9))`
pub struct ParallelPairs<I, J, L, R>
where
    I: ParallelSliceIterator<Container, Item = L>,
    J: ParallelSliceIterator<Container, Item = R>,
    L: Borrow<Container> + Send,
    R: Borrow<Container> + Send,
{
    left: I,
    right: J,
}

impl<I, J, L, R> ParallelPairs<I, J, L, R>
where
    I: ParallelSliceIterator<Container, Item = L>,
    J: ParallelSliceIterator<Container, Item = R>,
    L: Borrow<Container> + Send,
    R: Borrow<Container> + Send,
{
    pub fn new<A, B>(left: A, right: B) -> Self
    where
        A: IntoParallelIterator<Item = L, Iter = I>,
        B: IntoParallelIterator<Item = R, Iter = J>,
    {
        Self { left: left.into_par_iter(), right: right.into_par_iter() }
    }
}

impl<I, J, L, R> ParallelIterator for ParallelPairs<I, J, L, R>
where
    I: ParallelSliceIterator<Container, Item = L>,
    J: ParallelSliceIterator<Container, Item = R>,
    L: Borrow<Container> + Send,
    R: Borrow<Container> + Send,
{
    type Item = (Option<L>, Option<R>);

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
        where
            C: rayon::iter::plumbing::UnindexedConsumer<Self::Item>
    {
        let left = self.left.with_slice_producer(CallbackLeft::<_, R, _> {
            right: self.right,
            consumer,
            marker: PhantomData
        });
        return left;

        struct CallbackLeft<S, R, C> {
            right: S,
            consumer: C,
            marker: PhantomData<R>,
        }

        impl<C, S, I, R> SliceProducerCallback<Container, I> for CallbackLeft<S, R, C>
        where
            S: ParallelSliceIterator<Container, Item = R>,
            C: UnindexedConsumer<(Option<I>, Option<R>)>,
            R: Borrow<Container> + Send,
            I: Borrow<Container> + Send,
        {
            type Output = C::Result;
            fn callback<P>(self, producer: P) -> C::Result
            where
                P: SliceProducer<Container, Item = I>,
            {
                self.right.with_slice_producer(CallbackRight {
                    left_producer: producer,
                    consumer: self.consumer,
                    marker: PhantomData,
                })
            }
        }

        struct CallbackRight<LP, C, L> {
            left_producer: LP,
            consumer: C,
            marker: PhantomData<L>,
        }

        impl<LP, C, I, L> SliceProducerCallback<Container, I> for CallbackRight<LP, C, L>
        where
            LP: SliceProducer<Container, Item = L>,
            C: UnindexedConsumer<(Option<L>, Option<I>)>,
            L: Borrow<Container> + Send,
            I: Borrow<Container> + Send,
        {
            type Output = C::Result;
            fn callback<P>(self, right: P) -> C::Result
            where
                P: SliceProducer<Container, Item = I>,
            {
                bridge_unindexed(
                    ParPairsProducer {
                        left: self.left_producer,
                        right,
                    },
                    self.consumer
                )
            }
        }
    }
}

pub struct ParPairsProducer<I, J, L, R>
where
    I: SliceProducer<Container, Item = L>,
    J: SliceProducer<Container, Item = R>,
    L: Borrow<Container> + Send,
    R: Borrow<Container> + Send,
{
    left: I,
    right: J,
}

const MAX_SPLIT_SIZE: usize = 512;

impl<I, J, L, R> UnindexedProducer for ParPairsProducer<I, J, L, R>
where
    I: SliceProducer<Container, Item = L>,
    J: SliceProducer<Container, Item = R>,
    L: Borrow<Container> + Send,
    R: Borrow<Container> + Send,
{
    type Item = (Option<L>, Option<R>);

    fn split(self) -> (Self, Option<Self>) {
        let left_slice = self.left.as_slice();
        let right_slice = self.right.as_slice();
        if left_slice.len()/2 <= MAX_SPLIT_SIZE && right_slice.len()/2 <= MAX_SPLIT_SIZE {
            return (Self {
                left: self.left,
                right: self.right,
            }, None);
        }
        let (split_on_left, split_point) = if right_slice.len() > left_slice.len() {
            (false, right_slice.len()/2)
        } else {
            (true, left_slice.len()/2)
        };
        if split_on_left {
            let biggest = left_slice[split_point - 1].key;
            let right_split_point = right_slice.partition_point(|f| f.key <= biggest);
            if right_split_point >= right_slice.len() {
                // There is no overlap left, don't even bother splitting
                return (Self {
                    left: self.left,
                    right: self.right,
                }, None);
            }
            let (left_left, left_right) = self.left.split_at(split_point);
            let (right_left, right_right) = self.right.split_at(right_split_point);
            (Self {
                left: left_left,
                right: right_left,
            }, Some(Self {
                left: left_right,
                right: right_right,
            }))
        } else {
            let biggest = right_slice[split_point - 1].key;
            let left_split_point = left_slice.partition_point(|f| f.key <= biggest);
            if left_split_point >= left_slice.len() {
                // There is no overlap left, don't even bother splitting
                return (Self {
                    left: self.left,
                    right: self.right,
                }, None);
            }
            let (right_left, right_right) = self.right.split_at(split_point);
            let (left_left, left_right) = self.left.split_at(left_split_point);
            (Self {
                left: left_left,
                right: right_left,
            }, Some(Self {
                left: left_right,
                right: right_right,
            }))
        }
    }

    fn fold_with<F>(self, folder: F) -> F
        where
            F: rayon::iter::plumbing::Folder<Self::Item>
    {
        folder.consume_iter(Pairs::new(self.left.into_iter(), self.right.into_iter()))
    }
}

impl<'a, I> SliceIterator<I> for SliceDrain<'a, I> {
    fn as_slice(&self) -> &[I] {
        self.as_slice()
    }
}

pub trait SliceProducer<T>: Producer<IntoIter: SliceIterator<T>> {
    fn as_slice(&self) -> &[T];
}

pub trait SliceProducerCallback<D, T> {
    /// The type of value returned by this callback. Analogous to
    /// [`Output` from the `FnOnce` trait][FnOnce::Output].
    type Output;

    /// Invokes the callback with the given producer as argument. The
    /// key point of this trait is that this method is generic over
    /// `P`, and hence implementors must be defined for any producer.
    fn callback<P>(self, producer: P) -> Self::Output
    where
        P: SliceProducer<D, Item = T>;
}

pub trait ParallelSliceIterator<I>: IndexedParallelIterator {
    fn with_slice_producer<CB: SliceProducerCallback<I, Self::Item>>(self, callback: CB) -> CB::Output;
}

impl<T: Send> ParallelSliceIterator<T> for rayon::vec::IntoIter<T> {
    fn with_slice_producer<CB: SliceProducerCallback<T, Self::Item>>(self, callback: CB) -> CB::Output {
        self.with_concrete_producer(move |producer| callback.callback(producer))
    }
}

impl<'data, T: Send> ParallelSliceIterator<T> for Drain<'data, T> {
    fn with_slice_producer<CB: SliceProducerCallback<T, Self::Item>>(self, callback: CB) -> CB::Output {
        self.with_concrete_producer(move |producer| callback.callback(producer))
    }
}

impl<'data, T: Sync> ParallelSliceIterator<T> for rayon::slice::Iter<'data, T> {
    fn with_slice_producer<CB: SliceProducerCallback<T, Self::Item>>(self, callback: CB) -> CB::Output {
        self.with_concrete_producer(move |producer| callback.callback(producer))
    }
}

impl<'data, T: Send> ParallelSliceIterator<T> for rayon::slice::IterMut<'data, T> {
    fn with_slice_producer<CB: SliceProducerCallback<T, Self::Item>>(self, callback: CB) -> CB::Output {
        self.with_concrete_producer(move |producer| callback.callback(producer))
    }
}

impl<'data, T: Send> SliceProducer<T> for DrainProducer<'data, T> {
    fn as_slice(&self) -> &[T] {
        self.as_slice()
    }
}

impl<'data, T: Sync> SliceProducer<T> for IterProducer<'data, T> {
    fn as_slice(&self) -> &[T] {
        self.as_slice()
    }
}

impl<'data, T: Send> SliceProducer<T> for IterMutProducer<'data, T> {
    fn as_slice(&self) -> &[T] {
        self.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitmap::container::Container;

    #[test]
    fn test_split_2() {
        let left = Vec::from_iter([
            Container::new(3),
            Container::new(5),
            Container::new(8),
            Container::new(9),
        ]);
        let right = Vec::from_iter([
            Container::new(0),
            Container::new(1),
            Container::new(3),
            Container::new(5),
            Container::new(7),
            Container::new(8),
        ]);
        let parallel_pairs = ParallelPairs::new(left.clone(), right.clone());
        let pairs = Pairs::new(left.clone(), right.clone());
        let par_pars_vec: Vec<_> = parallel_pairs.collect();
        let pairs_vec: Vec<_> = pairs.collect();
        assert_eq!(par_pars_vec, pairs_vec);
    }
}

use std::{
    borrow::Borrow,
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem::ManuallyDrop,
    ops::Deref,
    ptr::{slice_from_raw_parts, NonNull},
    sync::Arc,
};

#[derive(Clone, Copy)]
enum Kind {
    Owned,
    Borrowed,
    Shared,
}

/// A clone-on-write smart pointer with an optimized memory layout, based on `beef`.
///
/// # Strings, strings everywhere
///
/// In `metrics`, strings are arguably the most common data type used despite the fact that metrics
/// are measuring numerical values. Both the name of a metric, and its labels, are strings: emitting
/// a metric may involve one string, or 10 strings. Many of these strings tend to be used over and
/// over during the life of the process, as well.
///
/// In order to achieve and maintain a high level of performance, we use a "clone-on-write" smart
/// pointer to handle passing these strings around. Doing so allows us to potentially avoid having
/// to allocate entire copies of a string, instead using a lightweight smart pointer that can live
/// on the stack.
///
/// # Why not `std::borrow::Cow`?
///
/// The standard library already provides a clone-on-write smart pointer, `std::borrow::Cow`, which
/// works well in many cases. However, `metrics` strives to provide minimal overhead where possible,
/// and so `std::borrow::Cow` falls down in one particular way: it uses an enum representation which
/// consumes an additional word of storage.
///
/// As an example, let's look at strings. A string in `std::borrow::Cow` implies that `T` is `str`,
/// and the owned version of `str` is simply `String`. Thus, for `std::borrow::Cow`, the in-memory
/// layout looks like this:
///
/// ```text
///                                                                       Padding
///                                                                          |
///                                                                          v
///                       +--------------+-------------+--------------+--------------+
/// stdlib Cow::Borrowed: |   Enum Tag   |   Pointer   |    Length    |   XXXXXXXX   |
///                       +--------------+-------------+--------------+--------------+
///                       +--------------+-------------+--------------+--------------+
/// stdlib Cow::Owned:    |   Enum Tag   |   Pointer   |    Length    |   Capacity   |
///                       +--------------+-------------+--------------+--------------+
/// ```
///
/// As you can see, you pay a memory size penalty to be able to wrap an owned string. This
/// additional word adds minimal overhead, but we can easily avoid it with some clever logic around
/// the values of the length and capacity fields.
///
/// There is an existing crate that does just that: `beef`. Instead of using an enum, it is simply a
/// struct that encodes the discriminant values in the length and capacity fields directly. If we're
/// wrapping a borrowed value, we can infer that the "capacity" will always be zero, as we only need
/// to track the capacity when we're wrapping an owned value, in order to be able to recreate the
/// underlying storage when consuming the smart pointer, or dropping it. Instead of the above
/// layout, `beef` looks like this:
///
/// ```text
///                        +-------------+--------------+----------------+
/// `beef` Cow (borrowed): |   Pointer   |  Length (N)  |  Capacity (0)  |
///                        +-------------+--------------+----------------+
///                        +-------------+--------------+----------------+
/// `beef` Cow (owned):    |   Pointer   |  Length (N)  |  Capacity (M)  |
///                        +-------------+--------------+----------------+
/// ```
///
/// # Why not `beef`?
///
/// Up until this point, it might not be clear why we didn't just use `beef`. In truth, our design
/// is fundamentally based on `beef`. Crucially, however, `beef` did not/still does not support
/// `const` construction for generic slices. Remember how we mentioned labels? The labels of a
/// metric are `[Label]` under-the-hood, and so without a way to construct them in a `const`
/// fashion, our previous work to allow entirely static keys would not be possible.
///
/// Thus, we forked `beef` and copied into directly into `metrics` so that we could write a
/// specialized `const` constructor for `[Label]`.
///
/// This is why we have our own `Cow` bundled into `metrics` directly, which is based on `beef`. In
/// doing so, we can experiment with more interesting optimizations, and, as mentioned above, we can
/// add const methods to support all of the types involved in statically building metrics keys.
///
/// # What we do that `beef` doesn't do
///
/// It was already enough to use our own implementation for the specialized `const` capabilities,
/// but we've taken things even further in a key way: support for `Arc`-wrapped values.
///
/// ## `Arc`-wrapped values
///
/// For many strings, there is still a desire to share them cheaply even when they are constructed
/// at run-time.  Remember, cloning a `Cow` of an owned value means cloning the value itself, so we
/// need another level of indirection to allow the cheap sharing, which is where `Arc<T>` can
/// provide value.
///
/// Users can construct a `Arc<T>`, where `T` is lined up with the `T` of `metrics::Cow`, and use
/// that as the initial value instead. When `Cow` is cloned, we end up cloning the underlying
/// `Arc<T>` instead, avoiding a new allocation.  `Arc<T>` still handles all of the normal logic
/// necessary to know when the wrapped value must be dropped, and how many live references to the
/// value that there are, and so on.
///
/// We handle this by relying on an invariant of `Vec<T>`: it never allocates more than `isize::MAX`
/// [1]. This lets us derive the following truth table of the valid combinations of length/capacity:
///
/// ```text
///                         Length (N)     Capacity (M)
///                     +---------------+----------------+
/// Borrowed (&T):      |       N       |        0       |
///                     +---------------+----------------+
/// Owned (T::ToOwned): |       N       | M < usize::MAX |
///                     +---------------+----------------+
/// Shared (Arc<T>):    |       N       |   usize::MAX   |
///                     +---------------+----------------+
/// ```
///
/// As we only implement `Cow` for types where their owned variants are either explicitly or
/// implicitly backed by `Vec<_>`, we know that our capacity will never be `usize::MAX`, as it is
/// limited to `isize::MAX`, and thus we can safely encode our "shared" state within the capacity
/// field.
///
/// # Notes
///
/// [1] - technically, `Vec<T>` can have a capacity greater than `isize::MAX` when storing
/// zero-sized types, but we don't do that here, so we always enforce that an owned version's
/// capacity cannot be `usize::MAX` when constructing `Cow`.
pub struct Cow<'a, T: Cowable + ?Sized + 'a> {
    ptr: NonNull<T::Pointer>,
    metadata: Metadata,
    _lifetime: PhantomData<&'a T>,
}

impl<T> Cow<'_, T>
where
    T: Cowable + ?Sized,
{
    fn from_parts(ptr: NonNull<T::Pointer>, metadata: Metadata) -> Self {
        Self { ptr, metadata, _lifetime: PhantomData }
    }

    /// Creates a pointer to an owned value, consuming it.
    pub fn from_owned(owned: T::Owned) -> Self {
        let (ptr, metadata) = T::owned_into_parts(owned);

        // This check is partially to guard against the semantics of `Vec<T>` changing in the
        // future, and partially to ensure that we don't somehow implement `Cowable` for a type
        // where its owned version is backed by a vector of ZSTs, where the capacity could
        // _legitimately_ be `usize::MAX`.
        if metadata.capacity() == usize::MAX {
            panic!("Invalid capacity of `usize::MAX` for owned value.");
        }

        Self::from_parts(ptr, metadata)
    }

    /// Creates a pointer to a shared value.
    pub fn from_shared(arc: Arc<T>) -> Self {
        let (ptr, metadata) = T::shared_into_parts(arc);
        Self::from_parts(ptr, metadata)
    }

    /// Extracts the owned data.
    ///
    /// Clones the data if it is not already owned.
    pub fn into_owned(self) -> <T as ToOwned>::Owned {
        // We need to ensure that our own `Drop` impl does _not_ run because we're simply
        // transferring ownership of the value back to the caller. For borrowed values, this is
        // naturally a no-op because there's nothing to drop, but for owned values, like `String` or
        // `Arc<T>`, we wouldn't want to double drop.
        let cow = ManuallyDrop::new(self);

        T::owned_from_parts(cow.ptr, &cow.metadata)
    }
}

impl<'a, T> Cow<'a, T>
where
    T: Cowable + ?Sized,
{
    /// Creates a pointer to a borrowed value.
    pub fn from_borrowed(borrowed: &'a T) -> Self {
        let (ptr, metadata) = T::borrowed_into_parts(borrowed);

        Self::from_parts(ptr, metadata)
    }
}

impl<'a, T> Cow<'a, [T]>
where
    T: Clone,
{
    pub const fn const_slice(val: &'a [T]) -> Cow<'a, [T]> {
        // SAFETY: We can never create a null pointer by casting a reference to a pointer.
        let ptr = unsafe { NonNull::new_unchecked(val.as_ptr() as *mut _) };
        let metadata = Metadata::borrowed(val.len());

        Self { ptr, metadata, _lifetime: PhantomData }
    }
}

impl<T> Cow<'static, [T]>
where
    T: Clone,
{
    pub(crate) fn to_retained(&self) -> Self {
        match self.metadata.kind() {
            Kind::Borrowed | Kind::Shared => self.clone(),
            Kind::Owned => Cow::from_shared(Arc::<[T]>::from(self.deref())),
        }
    }
}

impl<'a> Cow<'a, str> {
    pub const fn const_str(val: &'a str) -> Self {
        // SAFETY: We can never create a null pointer by casting a reference to a pointer.
        let ptr = unsafe { NonNull::new_unchecked(val.as_ptr() as *mut _) };
        let metadata = Metadata::borrowed(val.len());

        Self { ptr, metadata, _lifetime: PhantomData }
    }
}

impl Cow<'static, str> {
    pub(crate) fn to_retained(&self) -> Self {
        match self.metadata.kind() {
            Kind::Borrowed | Kind::Shared => self.clone(),
            Kind::Owned => Cow::from_shared(Arc::<str>::from(self.deref())),
        }
    }
}

impl<T> Deref for Cow<'_, T>
where
    T: Cowable + ?Sized,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        let borrowed_ptr = T::borrowed_from_parts(self.ptr, &self.metadata);

        // SAFETY: We only ever hold a pointer to a borrowed value of at least the lifetime of
        // `Self`, or an owned value which we have ownership of (albeit indirectly when using
        // `Arc<T>`), so our pointer is always valid and live for dereferencing.
        unsafe { borrowed_ptr.as_ref().unwrap() }
    }
}

impl<T> Clone for Cow<'_, T>
where
    T: Cowable + ?Sized,
{
    fn clone(&self) -> Self {
        let (ptr, metadata) = T::clone_from_parts(self.ptr, &self.metadata);
        Self { ptr, metadata, _lifetime: PhantomData }
    }
}

impl<T> Drop for Cow<'_, T>
where
    T: Cowable + ?Sized,
{
    fn drop(&mut self) {
        T::drop_from_parts(self.ptr, &self.metadata);
    }
}

impl<T> Hash for Cow<'_, T>
where
    T: Hash + Cowable + ?Sized,
{
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.deref().hash(state)
    }
}

impl<'a, T> Default for Cow<'a, T>
where
    T: Cowable + ?Sized,
    &'a T: Default,
{
    #[inline]
    fn default() -> Self {
        Cow::from_borrowed(Default::default())
    }
}

impl<T> Eq for Cow<'_, T> where T: Eq + Cowable + ?Sized {}

impl<A, B> PartialOrd<Cow<'_, B>> for Cow<'_, A>
where
    A: Cowable + ?Sized + PartialOrd<B>,
    B: Cowable + ?Sized,
{
    #[inline]
    fn partial_cmp(&self, other: &Cow<'_, B>) -> Option<Ordering> {
        PartialOrd::partial_cmp(self.deref(), other.deref())
    }
}

impl<T> Ord for Cow<'_, T>
where
    T: Ord + Cowable + ?Sized,
{
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        Ord::cmp(self.deref(), other.deref())
    }
}

impl<'a, T> From<&'a T> for Cow<'a, T>
where
    T: Cowable + ?Sized,
{
    #[inline]
    fn from(val: &'a T) -> Self {
        Cow::from_borrowed(val)
    }
}

impl<'a, T> From<Arc<T>> for Cow<'a, T>
where
    T: Cowable + ?Sized,
{
    #[inline]
    fn from(val: Arc<T>) -> Self {
        Cow::from_shared(val)
    }
}

impl<'a> From<std::borrow::Cow<'a, str>> for Cow<'a, str> {
    #[inline]
    fn from(s: std::borrow::Cow<'a, str>) -> Self {
        match s {
            std::borrow::Cow::Borrowed(bs) => Cow::from_borrowed(bs),
            std::borrow::Cow::Owned(os) => Cow::from_owned(os),
        }
    }
}

impl<'a, T: Cowable + ?Sized> From<Cow<'a, T>> for std::borrow::Cow<'a, T> {
    #[inline]
    fn from(value: Cow<'a, T>) -> Self {
        match value.metadata.kind() {
            Kind::Owned | Kind::Shared => Self::Owned(value.into_owned()),
            Kind::Borrowed => {
                // SAFETY: We know the contained data is borrowed from 'a, we're simply
                // restoring the original immutable reference and returning a copy of it.
                Self::Borrowed(unsafe { &*T::borrowed_from_parts(value.ptr, &value.metadata) })
            }
        }
    }
}

impl From<String> for Cow<'_, str> {
    #[inline]
    fn from(s: String) -> Self {
        Cow::from_owned(s)
    }
}

impl<T> From<Vec<T>> for Cow<'_, [T]>
where
    T: Clone,
{
    #[inline]
    fn from(v: Vec<T>) -> Self {
        Cow::from_owned(v)
    }
}

impl<T> AsRef<T> for Cow<'_, T>
where
    T: Cowable + ?Sized,
{
    #[inline]
    fn as_ref(&self) -> &T {
        self.borrow()
    }
}

impl<T> Borrow<T> for Cow<'_, T>
where
    T: Cowable + ?Sized,
{
    #[inline]
    fn borrow(&self) -> &T {
        self.deref()
    }
}

impl<A, B> PartialEq<Cow<'_, B>> for Cow<'_, A>
where
    A: Cowable + ?Sized,
    B: Cowable + ?Sized,
    A: PartialEq<B>,
{
    fn eq(&self, other: &Cow<B>) -> bool {
        self.deref() == other.deref()
    }
}

impl<T> fmt::Debug for Cow<'_, T>
where
    T: Cowable + fmt::Debug + ?Sized,
{
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.deref().fmt(f)
    }
}

impl<T> fmt::Display for Cow<'_, T>
where
    T: Cowable + fmt::Display + ?Sized,
{
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.deref().fmt(f)
    }
}

// SAFETY: `NonNull<T>` is not `Send` or `Sync` by default, but we're asserting that `Cow` is so
// long as the underlying `T` is.
unsafe impl<T: Cowable + Sync + ?Sized> Sync for Cow<'_, T> {}
unsafe impl<T: Cowable + Send + ?Sized> Send for Cow<'_, T> {}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata(usize, usize);

impl Metadata {
    #[inline]
    const fn len(&self) -> usize {
        self.0
    }

    #[inline]
    const fn capacity(&self) -> usize {
        self.1
    }

    #[inline]
    const fn kind(&self) -> Kind {
        match (self.0, self.1) {
            (_, usize::MAX) => Kind::Shared,
            (_, 0) => Kind::Borrowed,
            _ => Kind::Owned,
        }
    }

    #[inline]
    const fn shared(len: usize) -> Metadata {
        Metadata(len, usize::MAX)
    }

    #[inline]
    const fn borrowed(len: usize) -> Metadata {
        Metadata(len, 0)
    }

    #[inline]
    const fn owned(len: usize, capacity: usize) -> Metadata {
        Metadata(len, capacity)
    }
}

pub trait Cowable: ToOwned {
    type Pointer;

    fn borrowed_into_parts(&self) -> (NonNull<Self::Pointer>, Metadata);
    fn owned_into_parts(owned: <Self as ToOwned>::Owned) -> (NonNull<Self::Pointer>, Metadata);
    fn shared_into_parts(arc: Arc<Self>) -> (NonNull<Self::Pointer>, Metadata);

    fn borrowed_from_parts(ptr: NonNull<Self::Pointer>, metadata: &Metadata) -> *const Self;
    fn owned_from_parts(
        ptr: NonNull<Self::Pointer>,
        metadata: &Metadata,
    ) -> <Self as ToOwned>::Owned;
    fn clone_from_parts(
        ptr: NonNull<Self::Pointer>,
        metadata: &Metadata,
    ) -> (NonNull<Self::Pointer>, Metadata);
    fn drop_from_parts(ptr: NonNull<Self::Pointer>, metadata: &Metadata);
}

impl Cowable for str {
    type Pointer = u8;

    #[inline]
    fn borrowed_into_parts(&self) -> (NonNull<Self::Pointer>, Metadata) {
        // SAFETY: We know that it's safe to take and hold a pointer to a reference to `Self` since
        // `Cow` can only live as long as the input reference does, and an invalid pointer cannot
        // be taken from a live reference.
        let ptr = unsafe { NonNull::new_unchecked(self.as_ptr() as *mut _) };
        let metadata = Metadata::borrowed(self.len());
        (ptr, metadata)
    }

    #[inline]
    fn owned_into_parts(owned: Self::Owned) -> (NonNull<Self::Pointer>, Metadata) {
        // SAFETY: We know that it's safe to take and hold a pointer to a reference to `owned` since
        // we own the allocation by virtue of consuming it here without dropping it.
        let mut owned = ManuallyDrop::new(owned.into_bytes());
        let ptr = unsafe { NonNull::new_unchecked(owned.as_mut_ptr()) };
        let metadata = Metadata::owned(owned.len(), owned.capacity());
        (ptr, metadata)
    }

    #[inline]
    fn shared_into_parts(arc: Arc<Self>) -> (NonNull<Self::Pointer>, Metadata) {
        let metadata = Metadata::shared(arc.len());
        // SAFETY: We know that the pointer given back by `Arc::into_raw` is valid.
        let ptr = unsafe { NonNull::new_unchecked(Arc::into_raw(arc) as *mut _) };
        (ptr, metadata)
    }

    #[inline]
    fn borrowed_from_parts(ptr: NonNull<Self::Pointer>, metadata: &Metadata) -> *const Self {
        slice_from_raw_parts(ptr.as_ptr(), metadata.len()) as *const _
    }

    #[inline]
    fn owned_from_parts(
        ptr: NonNull<Self::Pointer>,
        metadata: &Metadata,
    ) -> <Self as ToOwned>::Owned {
        match metadata.kind() {
            Kind::Borrowed => {
                // SAFETY: We know that it's safe to take and hold a pointer to a reference to
                // `Self` since `Cow` can only live as long as the input reference does, and an
                // invalid pointer cannot be taken from a live reference.
                let s = unsafe { &*Self::borrowed_from_parts(ptr, metadata) };
                s.to_owned()
            }

            // SAFETY: We know that the pointer is valid because it could have only been constructed
            // from a valid `String` handed to `Cow::from_owned`, which we assumed ownership of.
            Kind::Owned => unsafe {
                String::from_raw_parts(ptr.as_ptr(), metadata.len(), metadata.capacity())
            },
            Kind::Shared => {
                // SAFETY: We know that the pointer is valid because it could have only been
                // constructed from a valid `Arc<str>` handed to `Cow::from_shared`, which we
                // assumed ownership of, also ensuring that the strong count is at least one.
                let s = unsafe { Arc::from_raw(Self::borrowed_from_parts(ptr, metadata)) };
                s.to_string()
            }
        }
    }

    #[inline]
    fn clone_from_parts(
        ptr: NonNull<Self::Pointer>,
        metadata: &Metadata,
    ) -> (NonNull<Self::Pointer>, Metadata) {
        match metadata.kind() {
            Kind::Borrowed => (ptr, *metadata),
            Kind::Owned => {
                // SAFETY: We know that the pointer is valid because it could have only been constructed
                // from a valid `String` handed to `Cow::from_owned`, which we assumed ownership of.
                let s = unsafe { &*Self::borrowed_from_parts(ptr, metadata) };

                Self::owned_into_parts(s.to_string())
            }
            Kind::Shared => clone_shared::<Self>(ptr, metadata),
        }
    }

    #[inline]
    fn drop_from_parts(ptr: NonNull<Self::Pointer>, metadata: &Metadata) {
        match metadata.kind() {
            Kind::Borrowed => {}

            // SAFETY: We know that the pointer is valid because it could have only been constructed
            // from a valid `String` handed to `Cow::from_owned`, which we assumed ownership of.
            Kind::Owned => unsafe {
                drop(Vec::from_raw_parts(ptr.as_ptr(), metadata.len(), metadata.capacity()));
            },

            // SAFETY: We know that the pointer is valid because it could have only been constructed
            // from a valid `Arc<str>` handed to `Cow::from_shared`, which we assumed ownership of,
            // also ensuring that the strong count is at least one.
            Kind::Shared => unsafe {
                drop(Arc::from_raw(Self::borrowed_from_parts(ptr, metadata)));
            },
        }
    }
}

impl<T> Cowable for [T]
where
    T: Clone,
{
    type Pointer = T;

    #[inline]
    fn borrowed_into_parts(&self) -> (NonNull<Self::Pointer>, Metadata) {
        // SAFETY: We know that it's safe to take and hold a pointer to a reference to `Self` since
        // `Cow` can only live as long as the input reference does, and an invalid pointer cannot
        // be taken from a live reference.
        let ptr = unsafe { NonNull::new_unchecked(self.as_ptr() as *mut _) };
        let metadata = Metadata::borrowed(self.len());
        (ptr, metadata)
    }

    #[inline]
    fn owned_into_parts(owned: <Self as ToOwned>::Owned) -> (NonNull<Self::Pointer>, Metadata) {
        let mut owned = ManuallyDrop::new(owned);

        // SAFETY: We know that it's safe to take and hold a pointer to a reference to `owned` since
        // we own the allocation by virtue of consuming it here without dropping it.
        let ptr = unsafe { NonNull::new_unchecked(owned.as_mut_ptr()) };
        let metadata = Metadata::owned(owned.len(), owned.capacity());
        (ptr, metadata)
    }

    #[inline]
    fn shared_into_parts(arc: Arc<Self>) -> (NonNull<Self::Pointer>, Metadata) {
        let metadata = Metadata::shared(arc.len());
        // SAFETY: We know that the pointer given back by `Arc::into_raw` is valid.
        let ptr = unsafe { NonNull::new_unchecked(Arc::into_raw(arc) as *mut _) };
        (ptr, metadata)
    }

    #[inline]
    fn borrowed_from_parts(ptr: NonNull<Self::Pointer>, metadata: &Metadata) -> *const Self {
        slice_from_raw_parts(ptr.as_ptr(), metadata.len()) as *const _
    }

    #[inline]
    fn owned_from_parts(
        ptr: NonNull<Self::Pointer>,
        metadata: &Metadata,
    ) -> <Self as ToOwned>::Owned {
        match metadata.kind() {
            Kind::Borrowed => {
                // SAFETY: We know that it's safe to take and hold a pointer to a reference to
                // `Self` since `Cow` can only live as long as the input reference does, and an
                // invalid pointer cannot be taken from a live reference.
                let data = unsafe { &*Self::borrowed_from_parts(ptr, metadata) };
                data.to_vec()
            }

            // SAFETY: We know that the pointer is valid because it could have only been
            // constructed from a valid `Vec<T>` handed to `Cow::from_owned`, which we
            // assumed ownership of.
            Kind::Owned => unsafe {
                Vec::from_raw_parts(ptr.as_ptr(), metadata.len(), metadata.capacity())
            },

            Kind::Shared => {
                // SAFETY: We know that the pointer is valid because it could have only been
                // constructed from a valid `Arc<[T]>` handed to `Cow::from_shared`, which we
                // assumed ownership of, also ensuring that the strong count is at least one.
                let arc = unsafe { Arc::from_raw(Self::borrowed_from_parts(ptr, metadata)) };
                arc.to_vec()
            }
        }
    }

    #[inline]
    fn clone_from_parts(
        ptr: NonNull<Self::Pointer>,
        metadata: &Metadata,
    ) -> (NonNull<Self::Pointer>, Metadata) {
        match metadata.kind() {
            Kind::Borrowed => (ptr, *metadata),
            Kind::Owned => {
                let vec_ptr = Self::borrowed_from_parts(ptr, metadata);

                // SAFETY: We know that the pointer is valid because it could have only been
                // constructed from a valid `Vec<T>` handed to `Cow::from_owned`, which we assumed
                // ownership of.
                let new_vec = unsafe { vec_ptr.as_ref().unwrap().to_vec() };

                Self::owned_into_parts(new_vec)
            }
            Kind::Shared => clone_shared::<Self>(ptr, metadata),
        }
    }

    #[inline]
    fn drop_from_parts(ptr: NonNull<Self::Pointer>, metadata: &Metadata) {
        match metadata.kind() {
            Kind::Borrowed => {}

            // SAFETY: We know that the pointer is valid because it could have only been constructed
            // from a valid `Vec<T>` handed to `Cow::from_owned`, which we assumed ownership of.
            Kind::Owned => unsafe {
                drop(Vec::from_raw_parts(ptr.as_ptr(), metadata.len(), metadata.capacity()));
            },

            // SAFETY: We know that the pointer is valid because it could have only been constructed
            // from a valid `Arc<[T]>` handed to `Cow::from_shared`, which we assumed ownership of,
            // also ensuring that the strong count is at least one.
            Kind::Shared => unsafe {
                drop(Arc::from_raw(Self::borrowed_from_parts(ptr, metadata)));
            },
        }
    }
}

fn clone_shared<T: Cowable + ?Sized>(
    ptr: NonNull<T::Pointer>,
    metadata: &Metadata,
) -> (NonNull<T::Pointer>, Metadata) {
    let arc_ptr = T::borrowed_from_parts(ptr, metadata);

    // SAFETY: We know that the pointer is valid because it could have only been
    // constructed from a valid `Arc<T>` handed to `Cow::from_shared`, which we assumed
    // ownership of, also ensuring that the strong count is at least one.
    unsafe {
        Arc::increment_strong_count(arc_ptr);
    }

    (ptr, *metadata)
}

/// These implementations should be identical to Deref, but enable dereferencing in a `const`
/// environment for `Key` hashing.
pub(crate) mod const_cow {
    use super::*;
    use crate::Label;
    use std::ptr::slice_from_raw_parts;

    impl Cow<'static, str> {
        pub(crate) const fn as_const_str(&self) -> &str {
            let borrowed_ptr =
                slice_from_raw_parts(self.ptr.as_ptr(), self.metadata.len()) as *const str;

            // SAFETY: We only ever hold a pointer to a borrowed value of at least the lifetime of
            // `Self`, or an owned value which we have ownership of (albeit indirectly when using
            // `Arc<T>`), so our pointer is always valid and live for dereferencing.
            //
            // self.ptr is also `NonNull<T>`, and so borrowed_ptr is guaranteed to not be null.
            unsafe { &*borrowed_ptr }
        }
    }

    impl Cow<'static, [Label]> {
        pub(crate) const fn as_const_slice(&self) -> &[Label] {
            let borrowed_ptr =
                slice_from_raw_parts(self.ptr.as_ptr(), self.metadata.len()) as *const [Label];

            // SAFETY: We only ever hold a pointer to a borrowed value of at least the lifetime of
            // `Self`, or an owned value which we have ownership of (albeit indirectly when using
            // `Arc<T>`), so our pointer is always valid and live for dereferencing.
            //
            // self.ptr is also `NonNull<T>`, and so borrowed_ptr is guaranteed to not be null.
            unsafe { &*borrowed_ptr }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::Arc;

        #[test]
        fn test_as_const_str() {
            // Borrowed
            let cow = Cow::const_str("hello");
            assert_eq!(cow.as_const_str(), "hello");

            // Owned
            let cow: Cow<'static, str> = Cow::from_owned(String::from("hello"));
            assert_eq!(cow.as_const_str(), "hello");

            // Shared
            let arc: Arc<str> = Arc::from("hello");
            let cow: Cow<'static, str> = Cow::from_shared(arc);
            assert_eq!(cow.as_const_str(), "hello");
        }

        #[test]
        fn test_as_const_slice() {
            // Borrowed
            {
                static LABELS: [Label; 2] = [
                    Label::from_static_parts("key1", "value1"),
                    Label::from_static_parts("key2", "value2"),
                ];
                let cow = Cow::const_slice(&LABELS);
                let slice = cow.as_const_slice();
                assert_eq!(slice.len(), 2);
                assert_eq!(slice[0].key(), "key1");
                assert_eq!(slice[0].value(), "value1");
                assert_eq!(slice[1].key(), "key2");
                assert_eq!(slice[1].value(), "value2");
            }

            // Owned
            {
                let labels = vec![
                    Label::from_static_parts("key1", "value1"),
                    Label::from_static_parts("key2", "value2"),
                ];
                let cow: Cow<'static, [Label]> = Cow::from_owned(labels);
                let slice = cow.as_const_slice();
                assert_eq!(slice.len(), 2);
                assert_eq!(slice[0].key(), "key1");
                assert_eq!(slice[0].value(), "value1");
                assert_eq!(slice[1].key(), "key2");
                assert_eq!(slice[1].value(), "value2");
            }

            // Shared
            {
                let labels: Arc<[Label]> = Arc::from([
                    Label::from_static_parts("key1", "value1"),
                    Label::from_static_parts("key2", "value2"),
                ]);
                let cow: Cow<'static, [Label]> = Cow::from_shared(labels);
                let slice = cow.as_const_slice();
                assert_eq!(slice.len(), 2);
                assert_eq!(slice[0].key(), "key1");
                assert_eq!(slice[0].value(), "value1");
                assert_eq!(slice[1].key(), "key2");
                assert_eq!(slice[1].value(), "value2");
            }
        }

        #[test]
        fn test_to_retained_str() {
            let borrowed = Cow::const_str("hello").to_retained();
            assert!(matches!(borrowed.metadata.kind(), Kind::Borrowed));

            let owned: Cow<'static, str> = Cow::from_owned(String::from("hello"));
            let retained = owned.to_retained();
            assert!(matches!(retained.metadata.kind(), Kind::Shared));

            let shared = Cow::from_shared(Arc::<str>::from("hello")).to_retained();
            assert!(matches!(shared.metadata.kind(), Kind::Shared));
        }

        #[test]
        fn test_to_retained_slice() {
            static LABELS: [Label; 2] = [
                Label::from_static_parts("key1", "value1"),
                Label::from_static_parts("key2", "value2"),
            ];

            let borrowed = Cow::const_slice(&LABELS).to_retained();
            assert!(matches!(borrowed.metadata.kind(), Kind::Borrowed));

            let owned: Cow<'static, [Label]> = Cow::from_owned(LABELS.to_vec());
            let retained = owned.to_retained();
            assert!(matches!(retained.metadata.kind(), Kind::Shared));

            let shared = Cow::from_shared(Arc::<[Label]>::from(&LABELS[..])).to_retained();
            assert!(matches!(shared.metadata.kind(), Kind::Shared));
        }
    }
}

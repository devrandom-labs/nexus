#[derive(Debug)]
pub struct Product<H, T: HList>(pub H, pub T);

pub type One<T> = (T,);

#[inline]
pub fn one<T>(val: T) -> One<T> {
    (val,)
}

#[derive(Debug)]
pub enum Either<T, U> {
    A(T),
    B(U),
}

pub trait HList: Sized {
    type Tuple: Tuple<HList = Self>;

    fn flatten(self) -> Self::Tuple;
}

pub trait Tuple: Sized {
    type HList: HList<Tuple = Self>;

    fn hlist(self) -> Self::HList;

    #[inline]
    fn combine<T>(self, other: T) -> CombinedTuples<Self, T>
    where
        Self: Sized,
        T: Tuple,
        Self::HList: Combine<T::HList>,
    {
        self.hlist().combine(other.hlist()).flatten()
    }
}

pub type CombinedTuples<T, U> =
    <<<T as Tuple>::HList as Combine<<U as Tuple>::HList>>::Output as HList>::Tuple;

pub trait Combine<T: HList> {
    type Output: HList;

    fn combine(self, others: T) -> Self::Output;
}

pub trait Func<Args> {
    type Output;

    fn call(&self, args: Args) -> Self::Output;
}

// ===== Impl Combine ====
//

impl<T: HList> Combine<T> for () {
    type Output = T;

    #[inline]
    fn combine(self, other: T) -> Self::Output {
        other
    }
}

impl<H, T: HList, U: HList> Combine<U> for Product<H, T>
where
    T: Combine<U>,
    Product<H, <T as Combine<U>>::Output>: HList,
{
    type Output = Product<H, <T as Combine<U>>::Output>;

    #[inline]
    fn combine(self, other: U) -> Self::Output {
        Product(self.0, self.1.combine(other))
    }
}

impl HList for () {
    type Tuple = ();
    #[inline]
    fn flatten(self) -> Self::Tuple {}
}

impl Tuple for () {
    type HList = ();

    #[inline]
    fn hlist(self) -> Self::HList {}
}

impl<F, R> Func<()> for F
where
    F: Fn() -> R,
{
    type Output = R;

    #[inline]
    fn call(&self, _args: ()) -> Self::Output {
        (*self)()
    }
}

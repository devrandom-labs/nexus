mod and;
mod and_then;

use crate::generic::Tuple;

pub trait Handler: Sized + Send + Sync + 'static {
    type Extract: Tuple + Send;
    type Error: Send + Sync + 'static;
    type Future: Future<Output = Result<Self::Extract, Self::Error>> + Send;

    fn handle(self) -> Self::Future;

    fn and<F>(self, other: F) -> And<Self, F>
    where
        Self: Sized,
        <Self::Extract as Tuple>::HList: Combine<<F::Extract as Tuple>::HList>,
        F: Filter + Clone,
        F::Error: CombineRejection<Self::Error>,
    {
        And {
            first: self,
            second: other,
        }
    }
}

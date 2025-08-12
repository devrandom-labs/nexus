use crate::generic::Tuple;

pub trait Handler: Sized + Send + Sync + 'static {
    type Extract: Tuple + Send;
    type Error: Send + Sync + 'static;
    type Future: Future<Output = Result<Self::Extract, Self::Error>> + Send;

    fn handle(self) -> Self::Future;
}

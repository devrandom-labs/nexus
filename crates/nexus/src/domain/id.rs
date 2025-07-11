use std::str::FromStr;
use std::{
    fmt::{Debug, Display},
    hash::Hash,
};
pub trait Id:
    FromStr + Clone + Send + Sync + Debug + Hash + Eq + 'static + Display + AsRef<[u8]>
{
}

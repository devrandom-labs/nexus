use super::User;
use crate::command::{
    aggregate::{AggregateRoot, AggregateType},
    repository::{EventSourceRepository, RepositoryError},
};

#[derive(Debug, Clone)]
pub struct MockUserEventSourceRepsitory;

impl EventSourceRepository for MockUserEventSourceRepsitory {
    type AggregateType = User;

    fn load<'a>(
        &'a self,
        _id: &'a <Self::AggregateType as AggregateType>::Id,
    ) -> std::pin::Pin<
        Box<
            dyn Future<
                    Output = Result<
                        AggregateRoot<Self::AggregateType>,
                        RepositoryError<<Self::AggregateType as AggregateType>::Id>,
                    >,
                > + Send
                + 'a,
        >,
    > {
        todo!("load an aggregate")
    }

    fn save<'a>(
        &'a self,
        _aggregate: AggregateRoot<Self::AggregateType>,
    ) -> std::pin::Pin<
        Box<
            dyn Future<
                    Output = Result<
                        (),
                        RepositoryError<<Self::AggregateType as AggregateType>::Id>,
                    >,
                > + Send
                + 'a,
        >,
    > {
        todo!("save an aggregate")
    }
}

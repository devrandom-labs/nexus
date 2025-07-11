use common::{
    mock::{ErrorTypes, MockRepository},
    write_side_setup::{CreateUser, User, handlers::CreateUserHandler},
};
use nexus::{
    command::{EventSourceRepository, RepositoryError},
    domain::AggregateRoot,
    infra::NexusId,
};

mod common;

// #[tokio::test]
// async fn should_load_aggregate_when_correct_id_is_provided() {
//     let id = NexusId::default();

//     let repo = MockRepository::new(None);

//     let user_aggregate = repo.load(&id).await;
//     assert!(user_aggregate.is_ok());
// }

#[tokio::test]
async fn should_give_aggregate_not_found_error_when_invalid_id_is_provided() {
    let repo = MockRepository::new(None);
    let id = NexusId::default();
    let user_aggregate = repo.load(&id).await;
    assert!(user_aggregate.is_err());
    let error = user_aggregate.unwrap_err();
    match error {
        RepositoryError::AggregateNotFound(error_id) => {
            assert_eq!(error_id, id);
        }
        _ => panic!("expected AggregateNotFoundError"),
    }
}

#[tokio::test]
async fn should_save_aggregate_uncommited_events() {
    let aggregate_id = NexusId::default();
    let mut root = AggregateRoot::<User>::new(aggregate_id);
    let create_user = CreateUser {
        user_id: NexusId::default(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    let repo = MockRepository::new(None);
    let result = repo.save(root).await;
    assert!(result.is_ok());
}

// #[tokio::test]
// async fn should_give_conflict_error_when_version_mismatch_while_saving_aggregates() {
//     let aggregate_id = NexusId::default();
//     let mut root = AggregateRoot::<User>::new(aggregate_id);
//     let create_user = CreateUser {
//         user_id: NexusId::default(),
//         email: "joel@tixlys.com".to_string(),
//     };
//     let handler = CreateUserHandler;
//     let _ = root.execute(create_user, &handler, &()).await;
//     let repo = MockRepository::new(None);
//     let result = repo.save(root).await;
//     assert!(result.is_err());
//     let error = result.unwrap_err();
//     match error {
//         RepositoryError::Conflict {
//             aggregate_id,
//             expected_version,
//         } => {
//             assert_eq!(aggregate_id, aggregate_id);
//             assert_eq!(expected_version, 0);
//         }
//         _ => panic!("expected Conflict Error"),
//     }
// }

#[tokio::test]
async fn should_give_store_error_on_unreleated_database_error() {
    let repo = MockRepository::new_with_error(ErrorTypes::Store);
    let id = NexusId::default();
    let user_aggregate = repo.load(&id).await;
    assert!(user_aggregate.is_err());
    let error = user_aggregate.unwrap_err();
    match error {
        RepositoryError::StoreError { .. } => {}
        _ => panic!("expected StoreError"),
    }
}

#[tokio::test]
async fn should_return_deserialization_error_on_load() {
    let repo = MockRepository::new_with_error(ErrorTypes::Deserialization);
    let id = NexusId::default();
    let user_aggregate = repo.load(&id).await;
    assert!(user_aggregate.is_err());
    let error = user_aggregate.unwrap_err();
    match error {
        RepositoryError::DeserializationError { .. } => {}
        _ => panic!("expected AggregateNotFoundError"),
    }
}

#[tokio::test]
async fn should_return_serialization_error_on_save() {
    let aggregate_id = NexusId::default();
    let mut root = AggregateRoot::<User>::new(aggregate_id);
    let create_user = CreateUser {
        user_id: NexusId::default(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let _ = root.execute(create_user, &handler, &()).await;
    let repo = MockRepository::new_with_error(ErrorTypes::Serialization);
    let result = repo.save(root).await;
    assert!(result.is_err());
    let error = result.unwrap_err();
    match error {
        RepositoryError::SerializationError { .. } => {}
        _ => panic!("expected Serialization Error"),
    }
}

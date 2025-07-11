use chrono::Utc;
use common::write_side_setup::{
    ActivateUser, CreateUser, SomeService, User, UserDomainEvents, UserError, UserState,
    handlers::{ActivateUserHandler, CreateUserHandler, CreateUserHandlerWithService},
};
use nexus::{
    domain::{Aggregate, AggregateRoot, AggregateState},
    events,
    infra::NexusId,
};

mod common;

// user state tests
#[test]
fn should_initialize_user_state_with_default_values() {
    let user_state = UserState::default();
    assert_eq!(user_state.email, None);
    assert_eq!(user_state.created_at, None);
    assert!(!user_state.is_active);
}

#[test]
fn should_set_email_and_timestamp_when_user_created_event_applied() {
    let mut user_state = UserState::default();
    let timestamp = Utc::now();
    let user_created = UserDomainEvents::UserCreated {
        id: NexusId::default(),
        email: String::from("joel@tixlys.com"),
        timestamp,
    };
    user_state.apply(&user_created);
    assert_eq!(user_state.email, Some(String::from("joel@tixlys.com")));
    assert_eq!(user_state.created_at, Some(timestamp));
}

#[test]
fn should_set_active_when_user_activated_event_applied_after_creation() {
    let mut user_state = UserState::default();
    let timestamp = Utc::now();
    let id = NexusId::default();
    let events = events![
        UserDomainEvents::UserCreated {
            id,
            email: String::from("joel@tixlys.com"),
            timestamp
        },
        UserDomainEvents::UserActivated { id }
    ];
    for event in events {
        user_state.apply(&event);
    }
    assert!(user_state.is_active);
}

#[test]
fn should_not_activate_user_if_activated_event_applied_before_created() {
    let mut user_state = UserState::default();
    let timestamp = Utc::now();
    let id = NexusId::default();
    let events = events![
        UserDomainEvents::UserActivated { id },
        UserDomainEvents::UserCreated {
            id,
            email: String::from("joel@tixlys.com"),
            timestamp
        }
    ];
    for event in events {
        user_state.apply(&event);
    }
    assert_eq!(user_state.email, Some(String::from("joel@tixlys.com")));
    assert_eq!(user_state.created_at, Some(timestamp));
    assert!(!user_state.is_active);
}

#[test]
fn should_not_change_state_when_empty_event_list_applied() {
    let mut user_state = UserState::default();

    for event in [] {
        user_state.apply(event);
    }
    assert!(!user_state.is_active);
    assert_eq!(user_state.created_at, None);
    assert_eq!(user_state.email, None);
}

// create, modify should be idempotent, but its on the implementor,
// can I make it easier for them to adher to idempotency???
#[test]
fn should_be_idempotent_when_user_created_event_applied_multiple_times() {
    let mut user_state = UserState::default();
    let timestamp = Utc::now();
    let id = NexusId::default();
    let user_created = UserDomainEvents::UserCreated {
        id,
        email: String::from("joel@tixlys.com"),
        timestamp,
    };
    user_state.apply(&user_created);
    user_state.apply(&user_created);
    user_state.apply(&user_created);
    user_state.apply(&user_created);
    assert_eq!(user_state.email, Some(String::from("joel@tixlys.com")));
    assert_eq!(user_state.created_at, Some(timestamp));
    assert!(!user_state.is_active);
}

// aggregate root test
#[test]
fn should_initialize_aggregate_root_with_id_version_zero_and_default_state() {
    let aggregate_id = NexusId::default();
    let mut root = AggregateRoot::<User>::new(aggregate_id.clone());
    assert_eq!(root.id(), &aggregate_id);
    assert_eq!(root.version(), 0);
    assert_eq!(root.state(), &UserState::default());
    assert_eq!(root.current_version(), 0);
    assert_eq!(root.take_uncommitted_events().to_vec(), []);
}

#[test]
fn should_rehydrate_state_and_version_from_event_history() {
    let timestamp = Utc::now();
    let id = NexusId::default();
    let history = events![
        UserDomainEvents::UserCreated {
            id,
            email: String::from("joel@tixlys.com"),
            timestamp
        },
        UserDomainEvents::UserActivated { id }
    ];

    let aggregate_id = NexusId::default();
    let mut aggregate_root =
        AggregateRoot::<User>::load_from_history(aggregate_id.clone(), &history);
    assert_eq!(aggregate_root.id(), &aggregate_id);
    assert_eq!(aggregate_root.version(), 2);
    assert_eq!(
        aggregate_root.state(),
        &UserState::new(Some(String::from("joel@tixlys.com")), true, Some(timestamp))
    );
    assert_eq!(aggregate_root.current_version(), 2);
    assert_eq!(aggregate_root.take_uncommitted_events().to_vec(), []);
}

#[tokio::test]
async fn should_correctly_reflect_state_from_unordered_history_and_then_process_activate_command() {
    let timestamp = Utc::now();
    let id = NexusId::default();
    let history = events![
        UserDomainEvents::UserActivated { id },
        UserDomainEvents::UserCreated {
            id,
            email: String::from("joel@tixlys.com"),
            timestamp
        },
    ];
    let aggregate_id = NexusId::default();
    let mut root = AggregateRoot::<User>::load_from_history(aggregate_id, &history);
    assert_eq!(root.id(), &aggregate_id);
    assert!(!root.state().is_active);
    assert_eq!(root.state().created_at, Some(timestamp));
    assert_eq!(root.state().email, Some(String::from("joel@tixlys.com")));
    let handler = ActivateUserHandler;
    let result = root
        .execute(
            ActivateUser {
                user_id: id.clone(),
            },
            &handler,
            &(),
        )
        .await;

    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result, id.to_string());
}

#[tokio::test]
async fn should_start_with_default_state_from_empty_history_and_then_create_user() {
    let aggregate_id = NexusId::default();
    let mut aggregate_root = AggregateRoot::<User>::load_from_history(aggregate_id, &[]);

    assert_eq!(aggregate_root.id(), &aggregate_id);
    assert!(!aggregate_root.state().is_active);
    assert_eq!(aggregate_root.state().created_at, None);
    assert_eq!(aggregate_root.state().email, None);

    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id.clone(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = aggregate_root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result, id.to_string());
}

#[tokio::test]
async fn should_fail_to_activate_user_when_loaded_from_empty_history_and_not_created() {
    let id = NexusId::default();
    let mut root = AggregateRoot::<User>::load_from_history(id, &[]);
    assert_eq!(root.id(), &id);
    assert!(!root.state().is_active);
    assert_eq!(root.state().created_at, None);
    assert_eq!(root.state().email, None);
    let handler = ActivateUserHandler; // checking the state and executing
    let result = root
        .execute(
            ActivateUser {
                user_id: NexusId::default(),
            },
            &handler,
            &(),
        )
        .await;

    assert!(result.is_err());
    let error_type = result.unwrap_err();
    assert_eq!(error_type, UserError::FailedToActivate);
}

#[tokio::test]
async fn should_apply_events_and_return_result_on_successful_command_execution() {
    let aggregate_id = NexusId::default();
    let mut root = AggregateRoot::<User>::new(aggregate_id);
    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id.clone(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result, id.to_string());
}

#[tokio::test]
async fn should_propagate_error_and_not_apply_events_when_command_handler_fails() {
    let aggregate_id = NexusId::default();
    let mut root = AggregateRoot::<User>::new(aggregate_id.clone());
    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id,
        email: "error@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_err());
    let result = result.unwrap_err();
    assert_eq!(result, UserError::FailedToCreateUser);
}

// current version
#[tokio::test]
async fn should_calculate_current_version_correctly_after_execute_produces_events() {
    let aggregate_id = NexusId::default();
    let mut root = AggregateRoot::<User>::load_from_history(aggregate_id.clone(), &[]);
    assert_eq!(root.current_version(), 0);

    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id.clone(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    assert_eq!(root.current_version(), 1);
}

#[tokio::test]
async fn should_revert_current_version_to_persisted_version_after_taking_uncommitted_events() {
    let aggregate_id = NexusId::default();
    let mut root = AggregateRoot::<User>::load_from_history(aggregate_id.clone(), &[]);
    assert_eq!(root.current_version(), 0);

    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id.clone(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    assert_eq!(root.current_version(), 1);
    let _events = root.take_uncommitted_events();
    assert_eq!(root.current_version(), 0);
}

#[tokio::test]
async fn should_correctly_process_multiple_commands_sequentially() {
    let aggregate_id = NexusId::default();
    let mut root = AggregateRoot::<User>::load_from_history(aggregate_id, &[]);

    assert_eq!(root.current_version(), 0);

    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id.clone(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    assert_eq!(root.current_version(), 1);
    let handler = ActivateUserHandler; // checking the state and executing
    let result = root
        .execute(ActivateUser { user_id: id }, &handler, &())
        .await;

    assert!(result.is_ok());
    assert_eq!(root.current_version(), 2);
}

#[tokio::test]
async fn should_pass_services_correctly_to_handler_during_execute() {
    let aggregate_id = NexusId::default();
    let mut root = AggregateRoot::<User>::new(aggregate_id);

    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id,
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandlerWithService;
    let service = SomeService {
        name: "Service".to_string(),
    };
    let result = root.execute(create_user, &handler, &service).await;
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result, "Service");
}

#[tokio::test]
async fn should_return_uncommitted_events_and_clear_internal_list_then_return_empty_on_second_call()
{
    let aggregate_id = NexusId::default();
    let mut aggregate_root = AggregateRoot::<User>::load_from_history(aggregate_id, &[]);

    assert_eq!(aggregate_root.current_version(), 0);
    let create_user = CreateUser {
        user_id: NexusId::default(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = aggregate_root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    let events = aggregate_root.take_uncommitted_events();
    assert_eq!(events.len(), 1);
    let events = aggregate_root.take_uncommitted_events();
    assert_eq!(events.len(), 0);
}

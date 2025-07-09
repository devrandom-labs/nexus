mod common;

use chrono::Utc;
use common::{
    setup::{
        ActivateUser, ActivateUserHandler, CreateUser, CreateUserHandler,
        CreateUserHandlerWithService, SomeService, User, UserDomainEvents, UserError, UserState,
    },
    utils::{EventType, MockData},
};
use nexus::{
    domain::{Aggregate, AggregateLoadError, AggregateRoot, AggregateState},
    infra::events::Events,
};

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
        id: "id".to_string(),
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
    for events in MockData::new(Some(timestamp), EventType::Ordered).events {
        user_state.apply(&events);
    }
    assert!(user_state.is_active);
}

#[test]
fn should_not_activate_user_if_activated_event_applied_before_created() {
    let mut user_state = UserState::default();
    let timestamp = Utc::now();
    for events in MockData::new(Some(timestamp), EventType::UnOrdered).events {
        user_state.apply(&events);
    }
    assert_eq!(user_state.email, Some(String::from("joel@tixlys.com")));
    assert_eq!(user_state.created_at, Some(timestamp));
    assert!(!user_state.is_active);
}

#[test]
fn should_not_change_state_when_empty_event_list_applied() {
    let mut user_state = UserState::default();
    let timestamp = Utc::now();
    for events in MockData::new(Some(timestamp), EventType::Empty).events {
        user_state.apply(&events);
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
    let user_created = UserDomainEvents::UserCreated {
        id: "id".to_string(),
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
    let mut root = AggregateRoot::<User>::new(String::from("id"));
    assert_eq!(root.id(), "id");
    assert_eq!(root.version(), 0);
    assert_eq!(root.state(), &UserState::default());
    assert_eq!(root.current_version(), 0);
    assert_eq!(root.take_uncommitted_events(), Events::new());
}

#[test]
fn should_rehydrate_state_and_version_from_event_history() {
    let timestamp = Utc::now();
    let history = MockData::new(Some(timestamp), EventType::Ordered).events;
    let aggregate_root = AggregateRoot::<User>::load_from_history(String::from("id"), &history);
    assert!(aggregate_root.is_ok());
    let mut aggregate_root = aggregate_root.unwrap();
    assert_eq!(aggregate_root.id(), "id");
    assert_eq!(aggregate_root.version(), 2);
    assert_eq!(
        aggregate_root.state(),
        &UserState::new(Some(String::from("joel@tixlys.com")), true, Some(timestamp))
    );
    assert_eq!(aggregate_root.current_version(), 2);
    assert_eq!(aggregate_root.take_uncommitted_events(), Events::new());
}

#[test]
fn should_fail_to_load_when_event_aggregate_id_mismatches() {
    let timestamp = Utc::now();
    let history = MockData::new(Some(timestamp), EventType::Ordered).events;
    let aggregate_root =
        AggregateRoot::<User>::load_from_history(String::from("wrong_id"), &history);
    assert!(aggregate_root.is_err());
    let error = aggregate_root.unwrap_err();
    assert_eq!(
        error,
        AggregateLoadError::MismatchedAggregateId {
            expected_id: "wrong_id".to_string(),
            event_aggregate_id: "id".to_string()
        }
    );
}

#[tokio::test]
async fn should_correctly_reflect_state_from_unordered_history_and_then_process_activate_command() {
    let timestamp = Utc::now();
    let history = MockData::new(Some(timestamp), EventType::UnOrdered).events;
    let aggregate_root = AggregateRoot::<User>::load_from_history(String::from("id"), &history);
    assert!(aggregate_root.is_ok());
    let mut root = aggregate_root.unwrap();
    assert_eq!(root.id, "id");
    assert!(!root.state.is_active);
    assert_eq!(root.state.created_at, Some(timestamp));
    assert_eq!(root.state.email, Some(String::from("joel@tixlys.com")));
    let handler = ActivateUserHandler;
    let result = root
        .execute(
            ActivateUser {
                user_id: "id".to_string(),
            },
            &handler,
            &(),
        )
        .await;

    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result, "id");
}

#[tokio::test]
async fn should_start_with_default_state_from_empty_history_and_then_create_user() {
    let timestamp = Utc::now();
    let history = MockData::new(Some(timestamp), EventType::Empty).events;
    let aggregate_root = AggregateRoot::<User>::load_from_history(String::from("id"), &history);
    assert!(aggregate_root.is_ok());
    let mut root = aggregate_root.unwrap();
    assert_eq!(root.id, "id");
    assert!(!root.state.is_active);
    assert_eq!(root.state.created_at, None);
    assert_eq!(root.state.email, None);
    let create_user = CreateUser {
        user_id: "id".to_string(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result, "id");
}

#[tokio::test]
async fn should_fail_to_activate_user_when_loaded_from_empty_history_and_not_created() {
    let timestamp = Utc::now();
    let history = MockData::new(Some(timestamp), EventType::Empty).events;
    let aggregate_root = AggregateRoot::<User>::load_from_history(String::from("id"), &history);
    assert!(aggregate_root.is_ok());
    let mut root = aggregate_root.unwrap();
    assert_eq!(root.id, "id");
    assert!(!root.state.is_active);
    assert_eq!(root.state.created_at, None);
    assert_eq!(root.state.email, None);
    let handler = ActivateUserHandler; // checking the state and executing
    let result = root
        .execute(
            ActivateUser {
                user_id: "id".to_string(),
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
    let mut root = AggregateRoot::<User>::new(String::from("id"));
    let create_user = CreateUser {
        user_id: "id".to_string(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result, "id");
}

#[tokio::test]
async fn should_propagate_error_and_not_apply_events_when_command_handler_fails() {
    let mut root = AggregateRoot::<User>::new(String::from("id"));
    let create_user = CreateUser {
        user_id: "id".to_string(),
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
    let timestamp = Utc::now();
    let history = MockData::new(Some(timestamp), EventType::Empty).events;
    let aggregate_root = AggregateRoot::<User>::load_from_history(String::from("id"), &history);
    assert!(aggregate_root.is_ok());
    let mut root = aggregate_root.unwrap();
    assert_eq!(root.current_version(), 0);
    let create_user = CreateUser {
        user_id: "id".to_string(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    assert_eq!(root.current_version(), 1);
}

#[tokio::test]
async fn should_revert_current_version_to_persisted_version_after_taking_uncommitted_events() {
    let timestamp = Utc::now();
    let history = MockData::new(Some(timestamp), EventType::Empty).events;
    let aggregate_root = AggregateRoot::<User>::load_from_history(String::from("id"), &history);
    assert!(aggregate_root.is_ok());
    let mut root = aggregate_root.unwrap();
    assert_eq!(root.current_version(), 0);
    let create_user = CreateUser {
        user_id: "id".to_string(),
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
    let timestamp = Utc::now();
    let history = MockData::new(Some(timestamp), EventType::Empty).events;
    let aggregate_root = AggregateRoot::<User>::load_from_history(String::from("id"), &history);
    assert!(aggregate_root.is_ok());
    let mut root = aggregate_root.unwrap();
    assert_eq!(root.current_version(), 0);
    let create_user = CreateUser {
        user_id: "id".to_string(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    assert_eq!(root.current_version(), 1);
    let handler = ActivateUserHandler; // checking the state and executing
    let result = root
        .execute(
            ActivateUser {
                user_id: "id".to_string(),
            },
            &handler,
            &(),
        )
        .await;

    assert!(result.is_ok());
    assert_eq!(root.current_version(), 2);
}

#[tokio::test]
async fn should_pass_services_correctly_to_handler_during_execute() {
    let mut root = AggregateRoot::<User>::new(String::from("id"));
    let create_user = CreateUser {
        user_id: "id".to_string(),
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
    let timestamp = Utc::now();
    let history = MockData::new(Some(timestamp), EventType::Empty).events;
    let aggregate_root = AggregateRoot::<User>::load_from_history(String::from("id"), &history);
    assert!(aggregate_root.is_ok());
    let mut root = aggregate_root.unwrap();
    assert_eq!(root.current_version(), 0);
    let create_user = CreateUser {
        user_id: "id".to_string(),
        email: "joel@tixlys.com".to_string(),
    };
    let handler = CreateUserHandler;
    let result = root.execute(create_user, &handler, &()).await;
    assert!(result.is_ok());
    let events = root.take_uncommitted_events();
    assert_eq!(events.len(), 1);
    let events = root.take_uncommitted_events();
    assert_eq!(events.len(), 0);
}

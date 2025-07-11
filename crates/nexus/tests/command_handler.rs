use common::write_side_setup::{
    ActivateUser, CreateUser, DynTestService, MockDynTestService, SomeService, UserDomainEvents,
    UserError, UserState,
    handlers::{
        ActivateUserHandler, CreateUserAndActivate, CreateUserHandler,
        CreateUserHandlerWithService, CreateUserWithStateCheck, ProcessWithDynServiceHandler,
    },
};
use nexus::{command::AggregateCommandHandler, infra::NexusId};

mod common;
#[tokio::test]
async fn should_execute_handler_successfully_returning_events_and_result() {
    let state = UserState::default();
    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id,
        email: "joel@tixlys.com".to_string(),
    };

    let handler = CreateUserHandler;
    let result = handler.handle(&state, create_user.clone(), &()).await;

    assert!(result.is_ok());
    let result = result.unwrap();

    // Convert SmallVec to a regular slice or Vec to inspect elements
    assert_eq!(result.events.len(), 1, "Expected exactly one event");

    let events = result.events;
    // Perform a pattern match on the first event
    //
    match events.first {
        UserDomainEvents::UserCreated {
            id: event_id,
            email: event_email,
            timestamp: _,
        } => {
            // Assertions for command data in event
            assert_eq!(
                event_id, create_user.user_id,
                "Event ID should match command user_id"
            );
            assert_eq!(
                event_email, create_user.email,
                "Event email should match command email"
            );
        }
        _ => panic!("Expected UserCreated event, but found another event type or no event."),
    }

    assert_eq!(result.result, create_user.user_id.to_string());
}

#[tokio::test]
async fn should_fail_execution_when_handler_logic_encounters_error() {
    let state = UserState::default();
    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id,
        email: "error@tixlys.com".to_string(),
    };

    let handler = CreateUserHandler;
    let result = handler.handle(&state, create_user, &()).await;
    assert!(result.is_err());
    let result = result.unwrap_err();
    assert_eq!(result, UserError::FailedToCreateUser);
}

#[tokio::test]
async fn should_succeed_when_state_satisfies_handler_preconditions() {
    let state = UserState::default();
    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id,
        email: "joel@tixlys.com".to_string(),
    };

    let handler = CreateUserWithStateCheck;
    let result = handler.handle(&state, create_user, &()).await;
    assert!(result.is_ok());
    let result = result.unwrap();

    let mut events_iter = result.events.into_iter();

    // Assert the first event matches the expected pattern
    assert!(matches!(
        events_iter.next(),
        Some(UserDomainEvents::UserCreated { .. })
    ));

    // Assert the second event matches

    // Finally, assert that there are no more events left
    assert!(events_iter.next().is_none());
    assert_eq!(result.result, id.to_string());
}

#[tokio::test]
async fn should_fail_when_state_does_not_satisfy_handler_preconditions() {
    let state = UserState::default();
    let id = NexusId::default();
    let activate_user = ActivateUser { user_id: id };
    let handler = ActivateUserHandler;
    let result = handler.handle(&state, activate_user, &()).await;
    assert!(result.is_err());
    let result = result.unwrap_err();
    assert_eq!(result, UserError::FailedToActivate);
}

#[tokio::test]
async fn should_correctly_use_concrete_service_to_influence_outcome() {
    let state = UserState::default();
    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id,
        email: "joel@tixlys.com".to_string(),
    };

    let service = SomeService {
        name: "some_service".to_string(),
    };
    let handler = CreateUserHandlerWithService;
    let result = handler.handle(&state, create_user, &service).await;
    assert!(result.is_ok());
    let result = result.unwrap();

    let mut events_iter = result.events.into_iter();

    // Assert the first event matches the expected pattern
    assert!(matches!(
        events_iter.next(),
        Some(UserDomainEvents::UserCreated { .. })
    ));
    assert_eq!(result.result, "some_service".to_owned());
}

#[tokio::test]
async fn should_correctly_use_dyn_trait_service_to_influence_outcome() {
    let state = UserState::default(); // Assuming UserState is appropriate
    let id = NexusId::default();
    let command = CreateUser {
        user_id: id,
        email: "joel@tixlys.com".to_string(),
    };

    let mock_service_impl = MockDynTestService {
        prefix: "DynServicePrefix".to_string(),
        suffix_to_add: Some("ProcessedData".to_string()),
    };

    let handler = ProcessWithDynServiceHandler;

    // 2. Create a trait object for the service
    // This is the key part: `services` will be of type `&(dyn DynTestService + 'a)`
    let dyn_services: &dyn DynTestService = &mock_service_impl;

    // 3. Execute the handler with the trait object
    let result = handler.handle(&state, command.clone(), dyn_services).await;

    // 4. Assert the outcome
    assert!(result.is_ok(), "Handler failed: {:?}", result.err());
    let response = result.unwrap();

    // Assert that the service influenced the result
    let expected_result_string = mock_service_impl.process(&command.email);
    assert_eq!(
        response.result, expected_result_string,
        "The result string from the handler should match the service's processed output."
    );

    // Assert the event (confirming it's the placeholder UserActivated with correct id)
    let events = response.events;
    assert_eq!(events.len(), 1, "Expected exactly one event");

    match events.first {
        UserDomainEvents::UserCreated { id, .. } => {
            assert_eq!(id, command.user_id, "Event ID should match command item_id");
        }
        _ => panic!("Expected UserCreated event, but found something else or no event."),
    }
}

#[tokio::test]
async fn should_emit_multiple_distinct_events_when_logic_requires() {
    let state = UserState::default();
    let id = NexusId::default();
    let create_user = CreateUser {
        user_id: id,
        email: "joel@tixlys.com".to_string(),
    };

    let handler = CreateUserAndActivate;
    let result = handler.handle(&state, create_user, &()).await;
    assert!(result.is_ok());
    let result = result.unwrap();

    let mut events_iter = result.events.into_iter();

    // Assert the first event matches the expected pattern
    assert!(matches!(
        events_iter.next(),
        Some(UserDomainEvents::UserCreated { .. })
    ));

    // Assert the second event matches
    assert!(matches!(
        events_iter.next(),
        Some(UserDomainEvents::UserActivated { .. })
    ));

    // Finally, assert that there are no more events left
    assert!(events_iter.next().is_none());
    assert_eq!(result.result, id.to_string());
}

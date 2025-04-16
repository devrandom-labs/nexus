## Features
- Verify and Register.


### Value Objects
some value objects are created and some are imported from the crate for better funtionality
thanks to rusts awesom ecosystem, we do not have to write everything
- HashPassword (from password_hash crate)


How a command handler would be created.

``` org
Request = AddItemCommand { order_id: 123, item: ... }

--> Logging Middleware
  --> Metrics Middleware
    --> State Middleware (Adds EventStorePool)
      --> Extension Middleware (Adds EmailService)
        --> Aggregate Load/Save Middleware
          | Extracts order_id = 123
          | Uses EventStorePool -> Creates OrderRepository
          | Calls order_repo.load(123) -> Gets (order_aggregate, version 5)
          | Creates inner_request = (AddItemCommand, order_aggregate, version 5)
          --> Calls inner_service.call(inner_request)
            --> AddItemCommandHandler (Kernel Service)
              | Receives command, aggregate, version, EmailService (via extractors/request type)
              | Calls order_aggregate.add_item(command.item)? -> Returns Ok, aggregate has new events pending
              | Returns Ok(aggregate_with_pending_events) // Or just Ok(()) if aggregate tracks events internally
            <-- Returns Ok(aggregate_with_pending_events) to Load/Save Middleware
          | Calls order_repo.save(aggregate_with_pending_events, expected_version: 5)
          | Returns Ok(()) // Or other success indicator
        <-- Returns Ok(())
      <-- Returns Ok(())
    <-- Returns Ok(())
  <-- Returns Ok(())

Response = Ok(())
```

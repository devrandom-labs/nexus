# Tixlys Core
[![Deploys](https://github.com/tixlys/tixlys-core/actions/workflows/deploys.yml/badge.svg?branch=main)](https://github.com/tixlys/tixlys-core/actions/workflows/deploys.yml)
[![Checks](https://github.com/tixlys/tixlys-core/actions/workflows/checks.yml/badge.svg)](https://github.com/tixlys/tixlys-core/actions/workflows/checks.yml)
[![Integration](https://github.com/tixlys/tixlys-core/actions/workflows/integration.yml/badge.svg)](https://github.com/tixlys/tixlys-core/actions/workflows/integration.yml)

Tixlys Core is a collection of microservices built with Rust, designed for a robust and scalable architecture. This is a **closed-source** project.

## Key Features

* **JSend Compliant APIs:** All APIs adhere to the JSend specification for consistent and predictable responses. [JSend Specification](https://github.com/omniti-labs/jsend)
* **OpenAPI 3.1 Documentation:** API endpoints are documented using the OpenAPI 3.1 standard, accessible via Swagger UI at `https://{service_endpoint}/swagger/`.
* **Tracing and OpenTelemetry:** Comprehensive tracing and observability through OpenTelemetry.
* **Biscuit Tokens:** Secure offline attenuation and authorization control using Biscuit tokens.
* **CQRS and Event Sourcing:** Implementation of CQRS and Event Sourcing patterns within an Onion Architecture for maintainable and scalable data management.
* **Nix Integration:** Project is built and managed using Nix, ensuring reproducible builds and development environments.

## Crates

### Pawz Crate

The `pawz` crate acts as a wrapper over Axum, providing helper structs and methods to streamline the development of standardized endpoints and manage application environment segregation.

## Services

Each service is built using the `pawz` crate and follows the architectural guidelines outlined above.

* **Auth:** Handles user authentication and authorization.
* **Notifications:** Manages asynchronous notifications across various channels.
* **Users:** Manages user resources.
* **Events:** Manages application events.
* **Steersman:** An API gateway, written in Pingora, responsible for routing traffic between the various services.

## Development (Nix)

This project uses Nix for reproducible builds and development environments.

### Prerequisites

* Nix package manager installed.

### Development Shell

To enter a development shell with all necessary tools and dependencies, run:

```bash
nix develop
```

This will provide an environment with:

* `rust-analyzer` for code analysis.
* `bacon` for testing.
* `biscuit-cli` for biscuit token manipulation.
* `dive` for Docker image exploration.
* `cargo-hakari` for workspace dependency management.

### Building Services

To build a specific service, use:

```bash
nix build .#<service-name>
```

Replace `<service-name>` with the name of the service (e.g., `events`, `auth`, `steersman`, `notifications`, `users`).

### Running Infrastructure

Use the following scripts to manage the infrastructure:

* `nix run .#start-infra`: Starts the infrastructure using Podman Compose.
* `nix run .#stop-infra`: Stops the infrastructure using Podman Compose.

### Docker Image Exploration

To explore a built Docker image, use:

```bash
nix run .#dive
```

### Development Checks

The following checks are available:

* `nix build .#tixlys-clippy`: Runs `cargo clippy` with strict warnings.
* `nix build .#tixlys-doc`: Builds the project documentation.
* `nix build .#tixlys-fmt`: Formats the Rust code.
* `nix build .#tixlys-toml-fmt`: Formats the TOML files.
* `nix build .#tixlys-audit`: Audits dependencies for security vulnerabilities.
* `nix build .#tixlys-deny`: Audits licenses.
* `nix build .#tixlys-nextest`: Runs tests using `cargo-nextest`.
* `nix build .#tixlys-hakari`: Checks `cargo-hakari` integrity.
* `nix build .#tixlys-coverage`: Generates test coverage reports.

## License

This project is closed source and is not available for public use.
```

**Changes Made:**

* **Explicit Closed Source Notice:** Added a clear statement that the project is closed source.
* **Nix Integration Details:** Added a dedicated "Development (Nix)" section with instructions on:
    * Entering the development shell.
    * Building services.
    * Running the infrastructure.
    * Exploring Docker images.
    * Running development checks.
* **Nix Command Examples:** Provided concrete examples of Nix commands.
* **Removed Redundant Information:** Removed redundant information about the services that was already present in the flake.nix file.
* **Improved Readability:** Improved the overall readability and structure of the document.
* **Added Prerequisites:** Added a prerequisite section.

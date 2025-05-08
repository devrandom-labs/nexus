# Tixlys Core
[![Deploys](https://github.com/tixlys/tixlys-core/actions/workflows/deploys.yml/badge.svg?branch=main)](https://github.com/tixlys/tixlys-core/actions/workflows/deploys.yml)
[![Checks](https://github.com/tixlys/tixlys-core/actions/workflows/checks.yml/badge.svg)](https://github.com/tixlys/tixlys-core/actions/workflows/checks.yml)
[![Integration](https://github.com/tixlys/tixlys-core/actions/workflows/integration.yml/badge.svg)](https://github.com/tixlys/tixlys-core/actions/workflows/integration.yml)

Tixlys Core is a collection of microservices built with Rust, designed for a robust and scalable architecture. This is a **closed-source** project. (As of May 2025, the project is under active development, focusing on setting up core services and deployment pipelines.)

## Key Features

* **JSend Compliant APIs:** All APIs adhere to the JSend specification for consistent and predictable responses. [JSend Specification](https://github.com/omniti-labs/jsend)
* **OpenAPI 3.1 Documentation:** API endpoints are documented using the OpenAPI 3.1 standard, accessible via Swagger UI at `https://{service_endpoint}/swagger/`.
* **Tracing and OpenTelemetry:** Comprehensive tracing and observability through OpenTelemetry.
* **Biscuit Tokens:** Secure offline attenuation and authorization control using Biscuit tokens.
* **CQRS and Event Sourcing:** Foundational implementation of CQRS and Event Sourcing patterns (via the `nexus` crate) within an Onion Architecture for maintainable and scalable data management.
* **Nix Integration:** Project is built and managed using Nix, ensuring reproducible builds, development environments, and OCI image creation.
* **Secure Secret Management:** Utilizes SOPS with `age` encryption for managing application secrets, both for local development and server deployments.
* **Infrastructure as Code (IaC):** Plans to use NixOS for server configurations and Terraform for cloud provisioning (currently under design).

## Crates

### Core Libraries
* **Nexus:** The foundational crate for DDD, ES, and CQRS patterns.
* **Pawz:** A wrapper over Axum, providing helper structs and methods to streamline the development of standardized endpoints and manage application environment segregation.
* **Workspace Hack:** Utility crate for workspace management.

## Services

Each service is built using the `pawz` crate (for HTTP layers) and `nexus` (for core logic), following the architectural guidelines outlined above. The current focus is on the `auth` service, with others planned:

* **Auth:** Handles user authentication and authorization. (Skeleton implemented, OCI image builds automatically via GitHub Actions).
* **Notifications:** (Planned) Manages asynchronous notifications across various channels.
* **Users:** (Planned) Manages user resources.
* **Events:** (Planned) Manages application events.
* **Steersman:** (Planned) An API gateway, likely to be written in Pingora, responsible for routing traffic between the various services.

## Development Environment (Nix)

This project uses Nix for reproducible builds and development environments.

### Prerequisites

* Nix package manager installed (with Flakes and `nix-command` enabled).
* `direnv` (optional, but recommended for automatic environment loading).
* `age` command-line tool (for generating developer-specific encryption keys).

### Initial Setup

1.  **Clone the repository.**
2.  **Enter the Development Shell:**
    ```bash
    cd tixlys-core
    nix develop
    ```
    If you have `direnv` installed and configured for Nix, it should automatically activate the shell when you `cd` into the directory (after running `direnv allow`).

3.  **Development Secrets Setup (First Time):**
    This project uses SOPS with `age` encryption to manage secrets for local development (e.g., dummy API keys, local database URLs). Each developer needs to generate their own `age` key pair.

    * **Generate your `age` key pair:**
        ```bash
        age-keygen
        ```
        This will output a public key (starts with `age1...`) and a private key (starts with `AGE-SECRET-KEY-1...`).
    * **Store your private key securely:** The recommended default location that SOPS checks is `~/.config/sops/age/keys.txt`.
        ```bash
        mkdir -p ~/.config/sops/age
        # PASTE YOUR PRIVATE KEY (the AGE-SECRET-KEY-1... line) INTO THIS FILE:
        echo "AGE-SECRET-KEY-1YOUR_PRIVATE_KEY_HERE" > ~/.config/sops/age/keys.txt
        chmod 600 ~/.config/sops/age/keys.txt
        ```
        *Important: Ensure this file has restrictive permissions.*
    * **Share your public key:** Provide your `age` public key (the `age1...` line) to the project maintainer to be added to the `.sops.yaml` configuration. This will grant you decryption access to `infra/secrets/secrets.development.yaml`.

    Once your public key is added to `.sops.yaml` and you have your private key in `~/.config/sops/age/keys.txt`, secrets should be available when running services locally (e.g., via `sops exec-env ...` or automatically if `direnv` is configured with a suitable `.envrc`).

### Development Shell Tools

The `nix develop` shell provides:

* `rust-analyzer` for code analysis.
* `cargo`, `rustc` (from the Rust toolchain specified in `flake.nix`).
* `sops` for interacting with encrypted secrets.
* `age` for key generation.
* `bacon` for testing.
* `biscuit-cli` for biscuit token manipulation.
* `dive` for Docker/OCI image exploration.
* `cargo-hakari` for workspace dependency management.
* ### Building Service OCI Images

The project's `flake.nix` defines applications that build OCI images for services. For example, to build the `auth` service OCI image (the GitHub Actions workflow `deploys.yml` does this automatically):

```bash
nix build .#auth-oci # Or the specific package name for the OCI image output
# The GitHub Action 'nix run .#auth' triggers a script defined in the flake that
# builds and pushes the image to GHCR.

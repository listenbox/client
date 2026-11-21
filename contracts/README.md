# Client contracts

`client.yaml` defines the portable client YAML configuration. `public.yaml` is
the public API contract the CLI and desktop are built against. The Listenbox
monorepo writes it from its public API specification
(`moon run openapi:compile-public`); commit it with the client change that
needs it. Builds never download a contract from a running API.

`moon run client:codegen` invokes the OASmith Go tool pinned in `go.mod` and
generates ignored Go packages for both inputs. Private parent-repository
specifications are not build inputs for this repository.

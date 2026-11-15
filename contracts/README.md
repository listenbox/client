# Client contracts

`client.yaml` defines the portable client YAML configuration. The public API
contract comes from [the published API](https://v1.listenbox.app/), downloaded
into the ignored `.cache/public.openapi.yaml` by `client:public-contract`.

`moon run client:codegen` invokes the OASmith Go tool pinned in `go.mod` and
generates ignored Go packages for both inputs. Run
`moon run client:public-contract --force` to refresh the published contract
before its daily cache expires. Private parent-repository specifications are
not build inputs for this repository.

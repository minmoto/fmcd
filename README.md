# FMCD: A Fedimint Client for Server Side Applications

> This project is an incompatible fork of [Fedimint Clientd](https://github.com/fedimint/fedimint-clientd)

Run a Fedimint client with Ecash, Lightning, and Onchain modules to let a server side application hold and use Bitcoin with Fedimint. `fmcd` exposes a REST and Websocket APIs, with ability to manage clients connected to multiple Federations from a single instance.

This project is intended to be an easy-to-use starting point for those interested in adding Fedimint client support to their applications. `fmcd` only exposes Fedimint's default modules, and any more complex Fedimint integration will require custom implementation using [Fedimint's rust crates](https://github.com/fedimint/fedimint).

## Getting Started

You can install the cli app with `cargo install fmcd` or by cloning the repo and running `cargo build --release` in the root directory.

`fmcd` runs from the command line and takes a few arguments, which are also available as environment variables. Fedimint uses rocksDB, an embedded key-value store, to store its state. The `--fm_db_path` argument is required and should be an absolute path to a directory where the database will be stored.

```
CLI USAGE:
fmcd \
  --db-path=/absolute/path/to/dir/to/store/database \
  --password="some-secure-password-that-becomes-the-bearer-token" \
  --addr="127.0.0.1:8080"
  --mode="rest"
  --invite-code="fed1-fedimint-invite-code"

ENV USAGE:
FMCD_DB_PATH=/absolute/path/to/dir/to/store/database
FMCD_PASSWORD="some-secure-password-that-becomes-the-bearer-token"
FMCD_ADDR="127.0.0.1:8080"
FMCD_MODE="rest"
FMCD_INVITE_CODE="fed1-fedimint-invite-code"
```

## Fedimint Clientd Endpoints

`fmcd` supports the following endpoints (and has naive websocket support at `/v2/ws`, see code for details until I improve the interface. PRs welcome!). All the endpoints are authed with a Bearer token from the password (from CLI or env). You can hit the endpoints as such with curl:

```
curl http://localhost:3333/v2/admin/info -H 'Authorization: Bearer some-secure-password-that-becomes-the-bearer-token'
```

### Admin related commands:

- `/v2/admin/info`: Display wallet info (holdings, tiers).
- `/v2/admin/backup`: Upload the (encrypted) snapshot of mint notes to federation.
- `/v2/admin/discover-version`: Discover the common api version to use to communicate with the federation.
- `/v2/admin/restore`: Restore the previously created backup of mint notes (with `backup` command).
- `/v2/admin/list-operations`: List operations.
- `/v2/admin/module`: Call a module subcommand.
- `/v2/admin/config`: Returns the client config.

### Mint related commands:

- `/v2/mint/reissue`: Reissue notes received from a third party to avoid double spends.
- `/v2/mint/spend`: Prepare notes to send to a third party as a payment.
- `/v2/mint/validate`: Verifies the signatures of e-cash notes, but _not_ if they have been spent already.
- `/v2/mint/split`: Splits a string containing multiple e-cash notes (e.g. from the `spend` command) into ones that contain exactly one.
- `/v2/mint/combine`: Combines two or more serialized e-cash notes strings.

### Lightning network related commands:

- `/v2/ln/invoice`: Create a lightning invoice to receive payment via gateway.
- `/v2/ln/await-invoice`: Wait for incoming invoice to be paid.
- `/v2/ln/pay`: Pay a lightning invoice or lnurl via a gateway.
- `/v2/ln/await-pay`: Wait for a lightning payment to complete.
- `/v2/ln/list-gateways`: List registered gateways.
- `/v2/ln/switch-gateway`: Switch active gateway.

### Onchain related commands:

- `/v2/onchain/deposit-address`: Generate a new deposit address, funds sent to it can later be claimed.
- `/v2/onchain/await-deposit`: Wait for deposit on previously generated address.
- `/v2/onchain/withdraw`: Withdraw funds from the federation.

### Extra endpoints:

- `/health`: health check endpoint.
- `/metrics`: exports API metrics using opentelemetry with prometheus exporter (num requests, latency, high-level metrics only)

## Docker Support

FMCD provides Docker images through automated GitHub Actions workflows that build OCI containers using Nix.

### Using Pre-built Images

Docker images are automatically published to Docker Hub for releases:

```bash
# Pull the latest stable release (multi-arch: linux/amd64, linux/arm64)
docker pull okjodom/fmcd:latest

# Pull a specific version
docker pull okjodom/fmcd:1.2.3

# Run the container
docker run -d \
  -e FMCD_DB_PATH=/data \
  -e FMCD_PASSWORD="your-secure-password" \
  -e FMCD_ADDR="0.0.0.0:8080" \
  -v fmcd-data:/data \
  -p 8080:8080 \
  okjodom/fmcd:latest
```

### Building Locally

Build your own OCI image using Nix:

```bash
# Build the OCI container
nix build .#oci

# Load into Docker
docker load < ./result

# Verify the image
docker image ls | grep fmcd

# Tag and push to your registry
docker tag fmcd:latest <your-registry>/fmcd:v0.4.0
docker push <your-registry>/fmcd:v0.4.0
```

### Automated Publishing

This repository uses GitHub Actions to automatically build and publish Docker images:

- **Development builds** (single-arch, AMD64): Triggered on pushes to main/master
- **Production releases** (multi-arch, AMD64 + ARM64): Triggered on version tags (v*)

See [.github/workflows/README.md](.github/workflows/README.md) for detailed workflow documentation.

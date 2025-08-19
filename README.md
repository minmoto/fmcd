# FMCD: A Fedimint Client for Server Side Applications

> This project is an incompatible fork of [Fedimint Clientd](https://github.com/fedimint/fedimint-clientd)

Run a Fedimint client with Ecash, Lightning, and Onchain modules to let a server side application hold and use Bitcoin with Fedimint. `fmcd` exposes a REST and Websocket APIs, with ability to manage clients connected to multiple Federations from a single instance.

This project is intended to be an easy-to-use starting point for those interested in adding Fedimint client support to their applications. `fmcd` only exposes Fedimint's default modules, and any more complex Fedimint integration will require custom implementation using [Fedimint's rust crates](https://github.com/fedimint/fedimint).

## Getting Started

You can install the cli app with `cargo install fmcd` or by cloning the repo and running `cargo build --release` in the root directory.

`fmcd` runs from the command line and takes a few arguments, which are also available as environment variables. The `--data-dir` argument specifies where both the configuration file (`fmcd.conf`) and the database will be stored. Fedimint uses rocksDB, an embedded key-value store, to store its state.

```
CLI USAGE:
fmcd \
  --data-dir=/path/to/data/directory \
  --password="some-secure-password-that-becomes-the-bearer-token" \
  --addr="127.0.0.1:8080"
  --mode="rest"
  --invite-code="fed1-fedimint-invite-code"

ENV USAGE:
FMCD_DATA_DIR=/path/to/data/directory
FMCD_PASSWORD="some-secure-password-that-becomes-the-bearer-token"
FMCD_ADDR="127.0.0.1:8080"
FMCD_MODE="rest"
FMCD_INVITE_CODE="fed1-fedimint-invite-code"
```

## Authentication

`fmcd` uses HTTP Basic Authentication with:
- Username: `fmcd` (fixed)
- Password: Auto-generated on first run or set via `FMCD_PASSWORD`

The password is stored in the configuration file (`fmcd.conf`) inside the data directory and persists across restarts.

### Example API Request

```bash
# Using Basic Auth with curl
curl -u fmcd:your-password http://localhost:7070/v2/admin/info

# Or with Authorization header
curl http://localhost:7070/v2/admin/info \
  -H "Authorization: Basic $(echo -n fmcd:your-password | base64)"
```

## Fedimint Clientd Endpoints

`fmcd` supports the following endpoints (and has WebSocket support at `/v2/ws`). Metrics are available at `/metrics` on the same port.

### Admin related commands:

- `/v2/admin/info`: Display wallet info (holdings, tiers).
- `/v2/admin/backup`: Upload the (encrypted) snapshot of mint notes to federation.
- `/v2/admin/version`: Discover the common api version to use to communicate with the federation.
- `/v2/admin/restore`: Restore the previously created backup of mint notes (with `backup` command).
- `/v2/admin/operations`: List operations.
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
- `/v2/ln/pay`: Pay a lightning invoice or lnurl via a gateway.
- `/v2/ln/gateways`: List registered gateways.

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
  -e FMCD_DATA_DIR=/data \
  -e FMCD_ADDR="0.0.0.0:7070" \
  -v fmcd-data:/data \
  -p 7070:7070 \
  okjodom/fmcd:latest
```

### Docker Compose Example

Create a `docker-compose.yml` file with the following content for a complete deployment:

```yaml
services:
  fmcd:
    image: okjodom/fmcd:latest  # Or use locally built: fmcd:latest
    container_name: fmcd
    restart: unless-stopped

    volumes:
      - fmcd-data:/data  # Persistent volume for config and database

    ports:
      - "7070:7070"  # HTTP API, WebSocket, and Metrics (/metrics endpoint)

    environment:
      # Required: Data directory (contains config and database)
      FMCD_DATA_DIR: /data

      # Required: Server binding (use 0.0.0.0 for Docker)
      FMCD_ADDR: "0.0.0.0:7070"

      # Optional: Federation invite code (join on startup)
      # FMCD_INVITE_CODE: "fed11qgqrgvnhwden5te0v9k8q6rp9ekh2arfdeukuet595cr2ttpd3jhq6rzve6zuer9wchxvetyd938gcewvdhk6tcqqysptkuvknc7erjgf4em3zfh90kffqf9srujn6q53d6r056e4apze5cw27h75"

      # Optional: Set custom password (otherwise auto-generated on first run)
      # FMCD_PASSWORD: "your-secure-password"

      # Optional: Disable authentication (development only!)
      # FMCD_NO_AUTH: "true"

    healthcheck:
      test: ["CMD", "wget", "--no-verbose", "--tries=1", "--spider", "http://localhost:7070/health"]
      interval: 30s
      timeout: 3s
      retries: 3
      start_period: 5s

volumes:
  fmcd-data:
    driver: local
```

#### Running with Docker Compose

```bash
# Start the service
docker-compose up -d

# View logs
docker-compose logs -f fmcd

# Get the auto-generated password (first run only)
docker exec fmcd cat /data/fmcd.conf | grep http-password

# Stop the service
docker-compose down
```

### Password Management

On first run, fmcd automatically:
1. Creates `/data/fmcd.conf` with default settings
2. Generates a secure 64-character hex password
3. Displays "Generating default api password...done" in logs
4. Saves the password to the config file in the volume

To retrieve the auto-generated password:
```bash
# From a running container
docker exec fmcd cat /data/fmcd.conf | grep http-password

# Or check the initial logs
docker logs fmcd | grep "Generating"
```

### Building Locally

Build your own OCI image using Nix:

```bash
# Build the OCI container and Load into Docker
nix build .#oci && docker load < ./result

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

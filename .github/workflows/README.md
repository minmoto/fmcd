# Docker Hub Publishing Workflows

This directory contains GitHub Actions workflows for building and publishing fmcd Docker images to Docker Hub.

## Workflows

### 1. docker-publish.yml
- **Purpose**: Build and publish single-architecture (amd64) images for development and testing
- **Triggers**:
  - Push to main/master branches
  - Pull requests (build only, no push)
  - Manual workflow dispatch
  - **Note**: Does NOT run on version tags (handled by multi-arch workflow)
- **Features**:
  - Uses Nix to build OCI containers
  - Automatic tagging based on branches
  - Dry run for pull requests
  - Fast builds for rapid development feedback

### 2. docker-multiarch.yml
- **Purpose**: Build and publish production multi-architecture images (amd64 and arm64)
- **Triggers**:
  - Version tags (v*) - exclusive handler for version releases
  - Manual workflow dispatch with custom tag
- **Features**:
  - Cross-compilation using Nix
  - Multi-arch manifest creation
  - Support for both x86_64 and aarch64
  - Handles all semantic versioning tags

## Setup Requirements

Before these workflows can run successfully, you need to configure the following secrets in your GitHub repository:

1. **DOCKER_HUB_USERNAME**: Your Docker Hub username
2. **DOCKER_HUB_TOKEN**: Docker Hub access token (not password)

### Creating Docker Hub Access Token

1. Log in to [Docker Hub](https://hub.docker.com)
2. Go to Account Settings → Security
3. Click "New Access Token"
4. Give it a descriptive name (e.g., "GitHub Actions - fmcd")
5. Copy the token and save it as a GitHub secret

### Adding GitHub Secrets

1. Go to your repository on GitHub
2. Navigate to Settings → Secrets and variables → Actions
3. Click "New repository secret"
4. Add both secrets:
   - Name: `DOCKER_HUB_USERNAME`, Value: your Docker Hub username
   - Name: `DOCKER_HUB_TOKEN`, Value: your Docker Hub access token

## Image Tags

### Development Tags (docker-publish.yml)
- `latest` - Latest commit from main/master branch
- `main-<sha>` - Branch with commit SHA
- `pr-123` - Pull request builds (not pushed)

### Production Tags (docker-multiarch.yml)
- `latest` - Latest stable release (from version tags)
- `v1.2.3` - Specific version tags
- `1.2` - Major.minor version
- `1` - Major version only

## Building Locally

To build the OCI container locally using Nix:

```bash
# Build the OCI container
nix build .#oci

# Load into Docker
docker load < result

# Run the container
docker run --rm fmcd:latest
```

## Multi-Architecture Support

The multi-arch workflow builds for:
- `linux/amd64` (x86_64)
- `linux/arm64` (aarch64)

These are combined into a single manifest, allowing Docker to automatically pull the correct image for the host architecture.

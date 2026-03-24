# Docker Hub Publishing Workflows

This directory contains GitHub Actions workflows for building and publishing fmcd Docker images to Docker Hub.

## Workflows

### 1. release-plz.yml
- **Purpose**: Create release PRs and cut version tags after the release PR is merged
- **Triggers**:
  - Push to `main`
  - Manual workflow dispatch
- **Features**:
  - Opens or updates a release PR with version and changelog changes
  - Creates `v*` tags only after the release PR is merged
  - Uses git-only releases, so no crates.io publish is attempted
  - Drives the tagged binary and multi-arch Docker release workflows

### 2. docker-publish.yml
- **Purpose**: Build and publish single-architecture (amd64) images for development and testing
- **Triggers**:
  - Push to main/master branches
  - Pull requests (build only, no push)
  - Manual workflow dispatch
  - **Note**: Explicitly ignores version tags (v*)
- **Features**:
  - Uses Nix to build OCI containers
  - Automatic `main`/branch and commit-SHA tagging for development images
  - Dry run for pull requests
  - Fast builds for rapid development feedback

### 3. docker-multiarch.yml
- **Purpose**: Build and publish production multi-architecture images (amd64 and arm64)
- **Triggers**:
  - **ONLY** version tags (v*) - no other triggers
- **Features**:
  - Cross-compilation using Nix with QEMU
  - Multi-arch manifest creation
  - Support for both x86_64 and aarch64
  - Creates semantic version tags (latest, 1.2.3, 1.2, 1)

## Setup Requirements

Before these workflows can run successfully, you need to configure the following secrets in your GitHub repository:

1. **DOCKER_HUB_USERNAME**: Your Docker Hub username
2. **DOCKER_HUB_TOKEN**: Docker Hub access token (not password)
3. **RELEASE_PLZ_TOKEN**: GitHub PAT with `contents` and `pull requests` write access

The `RELEASE_PLZ_TOKEN` secret is required because tags created with the default `GITHUB_TOKEN` do not trigger the downstream `push tag` workflows that publish release binaries and Docker images.

You also need to enable GitHub Actions workflow permissions to create pull requests:

1. Go to Settings → Actions → General
2. Under "Workflow permissions", enable write access
3. Enable "Allow GitHub Actions to create and approve pull requests"

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
- `main` - Latest development image from the default branch
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

## Release Flow

1. Merge feature and fix PRs into `main`.
2. `release-plz.yml` opens or updates a release PR with the next version and changelog.
3. Merge the release PR when you want to cut a release.
4. `release-plz.yml` creates the `vX.Y.Z` tag.
5. The tag triggers:
   - `release.yml` to build binaries and create the GitHub release.
   - `docker-multiarch.yml` to publish stable multi-arch Docker tags.

With this policy, `latest` always points to the latest stable tagged release, not the tip of `main`.

## Multi-Architecture Support

The multi-arch workflow builds for:
- `linux/amd64` (x86_64)
- `linux/arm64` (aarch64)

These are combined into a single manifest, allowing Docker to automatically pull the correct image for the host architecture.
